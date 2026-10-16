//! El LaunchAgent de macOS que arranca `tandemd` al iniciar sesión.
//!
//! La app lo instala o lo pone al día cada vez que arranca: el plist apunta al `tandemd`
//! que viaja dentro de `Tandem.app` (`Contents/MacOS/tandemd`) y lleva la versión, así que
//! una app nueva cambia el plist y eso reinicia el agente con el binario nuevo.
//!
//! Todo lo que decide (el plist, si la app está en un lugar estable, qué hacer con lo que ya
//! hay) es portable y se prueba en cualquier sistema; `launchctl` queda detrás de
//! [`Launchd`].

use std::io;
use std::path::{Component, Path, PathBuf};

/// Etiqueta del servicio en launchd y nombre del plist.
pub const LABEL: &str = "com.franc1sc0sv.tandem.tandemd";
/// Nombre del sidecar dentro de `Contents/MacOS` (Tauri le quita el sufijo del target).
pub const SIDECAR: &str = "tandemd";
/// Si `tandemd` sale con error (por ejemplo, todavía no hay `tandem.toml`), launchd espera
/// esto antes de volver a lanzarlo: sin espera, un error fijo llenaría el log.
pub const THROTTLE_SECS: u32 = 30;

/// Lo que va en el plist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    /// `tandemd` dentro de la app.
    pub program: PathBuf,
    /// Salida y errores de `tandemd` antes de que abra su propio log.
    pub log: PathBuf,
    /// Versión de la app: si cambia, el plist cambia y el agente se reinicia.
    pub version: String,
}

/// `~/Library/LaunchAgents/<LABEL>.plist`.
pub fn plist_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// `~/Library/Logs/Tandem/tandemd.log`: `~/Library/Logs` existe siempre y launchd no crea
/// directorios.
pub fn log_path(home: &Path) -> PathBuf {
    home.join("Library/Logs/Tandem/tandemd.log")
}

/// Por qué la app no instala el agente desde donde está.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Placement {
    #[error("la app no corre desde un Tandem.app (compilación de desarrollo)")]
    NotBundled,
    #[error(
        "macOS abrió la app desde una copia temporal (App Translocation); \
         muévela a Aplicaciones y vuelve a abrirla"
    )]
    Translocated,
    #[error("la app corre desde la imagen de disco; cópiala a Aplicaciones y ábrela desde ahí")]
    DiskImage,
}

/// El `tandemd` que acompaña al ejecutable de la app, si la app está en un lugar del que el
/// agente puede depender: dentro de un `.app` y no en una copia temporal ni en el `.dmg`.
pub fn sidecar_for(exe: &Path) -> Result<PathBuf, Placement> {
    let dir = exe.parent().ok_or(Placement::NotBundled)?;
    let names: Vec<&str> = dir
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    let bundled = matches!(
        names.as_slice(),
        [.., app, "Contents", "MacOS"] if app.ends_with(".app")
    );
    if !bundled {
        return Err(Placement::NotBundled);
    }
    if names.contains(&"AppTranslocation") {
        return Err(Placement::Translocated);
    }
    if names.first() == Some(&"Volumes") {
        return Err(Placement::DiskImage);
    }
    Ok(dir.join(SIDECAR))
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// El plist del LaunchAgent.
///
/// - `RunAtLoad`: arranca al iniciar sesión (y al instalarlo).
/// - `KeepAlive.SuccessfulExit = false`: si cae con error, launchd lo levanta de nuevo (tras
///   [`THROTTLE_SECS`]); si sale bien (por ejemplo, al apagar) no.
/// - `LimitLoadToSessionType = Aqua` y `ProcessType = Interactive`: solo en una sesión
///   gráfica (la captura de teclado y mouse la necesita) y sin que macOS lo frene como a un
///   proceso de fondo.
pub fn plist(spec: &AgentSpec) -> String {
    let program = escape(&spec.program.to_string_lossy());
    let log = escape(&spec.log.to_string_lossy());
    let version = escape(&spec.version);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{program}</string>
	</array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>TANDEM_APP_VERSION</key>
		<string>{version}</string>
	</dict>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ThrottleInterval</key>
	<integer>{THROTTLE_SECS}</integer>
	<key>LimitLoadToSessionType</key>
	<string>Aqua</string>
	<key>ProcessType</key>
	<string>Interactive</string>
	<key>StandardOutPath</key>
	<string>{log}</string>
	<key>StandardErrorPath</key>
	<string>{log}</string>
</dict>
</plist>
"#
    )
}

/// `launchctl` en el dominio de la sesión del usuario (`gui/<uid>`).
pub trait Launchd {
    fn is_loaded(&self, label: &str) -> bool;
    /// Lo saca de launchd (y lo detiene). Que no estuviera cargado no es un error.
    fn bootout(&self, label: &str) -> Result<(), String>;
    /// Lo carga desde el plist; con `RunAtLoad` arranca enseguida.
    fn bootstrap(&self, plist: &Path) -> Result<(), String>;
}

/// Qué hizo [`ensure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// El plist ya era este y launchd lo tenía cargado.
    Unchanged,
    /// El plist ya era este pero no estaba cargado (lo habían sacado a mano o falló antes).
    Loaded,
    /// No había plist.
    Created,
    /// Había otro (otra versión u otra ubicación de la app): se reemplazó y se reinició.
    Updated,
}

impl Outcome {
    pub fn describe(self) -> &'static str {
        match self {
            Outcome::Unchanged => "tandemd ya estaba instalado como agente de inicio de sesión",
            Outcome::Loaded => "tandemd se volvió a cargar en launchd",
            Outcome::Created => "tandemd quedó instalado: arranca solo al iniciar sesión",
            Outcome::Updated => "tandemd se puso al día y se reinició",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("no está {0}; la app parece incompleta")]
    MissingSidecar(PathBuf),
    #[error("no se pudo escribir {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
    #[error("launchctl no cargó el agente: {0}")]
    Launchctl(String),
}

/// Deja instalado y cargado el LaunchAgent de `spec` en `plist_file`.
///
/// Solo toca launchd si hace falta: abrir la app con el agente ya al día no lo reinicia.
pub fn ensure(
    plist_file: &Path,
    spec: &AgentSpec,
    launchd: &dyn Launchd,
) -> Result<Outcome, InstallError> {
    if !spec.program.is_file() {
        return Err(InstallError::MissingSidecar(spec.program.clone()));
    }
    let want = plist(spec);
    let have = std::fs::read_to_string(plist_file).ok();
    if have.as_deref() == Some(want.as_str()) {
        if launchd.is_loaded(LABEL) {
            return Ok(Outcome::Unchanged);
        }
        launchd
            .bootstrap(plist_file)
            .map_err(InstallError::Launchctl)?;
        return Ok(Outcome::Loaded);
    }

    if let Some(dir) = spec.log.parent() {
        create_dir(dir)?;
    }
    write_atomic(plist_file, &want)?;
    // El viejo sigue corriendo con el binario anterior hasta que se lo saque.
    let outcome = if have.is_some() {
        Outcome::Updated
    } else {
        Outcome::Created
    };
    if launchd.is_loaded(LABEL) {
        launchd.bootout(LABEL).map_err(InstallError::Launchctl)?;
    }
    launchd
        .bootstrap(plist_file)
        .map_err(InstallError::Launchctl)?;
    Ok(outcome)
}

/// Cómo quedó el agente de inicio de sesión, para el primer arranque.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Report {
    /// `tandemd` quedó instalado y cargado.
    pub ok: bool,
    pub text: String,
}

/// Instala o pone al día el agente de la app que corre en `exe`, para el usuario de `home`.
pub fn install_from(exe: &Path, home: &Path, version: &str, launchd: &dyn Launchd) -> Report {
    let program = match sidecar_for(exe) {
        Ok(p) => p,
        Err(why) => {
            return Report {
                ok: false,
                text: format!("tandemd no se instaló: {why}"),
            };
        }
    };
    let spec = AgentSpec {
        program,
        log: log_path(home),
        version: version.to_owned(),
    };
    match ensure(&plist_path(home), &spec, launchd) {
        Ok(outcome) => Report {
            ok: true,
            text: outcome.describe().to_owned(),
        },
        Err(e) => Report {
            ok: false,
            text: format!("tandemd no se instaló: {e}"),
        },
    }
}

fn create_dir(dir: &Path) -> Result<(), InstallError> {
    std::fs::create_dir_all(dir).map_err(|source| InstallError::Write {
        path: dir.to_owned(),
        source,
    })
}

fn write_atomic(path: &Path, text: &str) -> Result<(), InstallError> {
    let err = |source| InstallError::Write {
        path: path.to_owned(),
        source,
    };
    if let Some(dir) = path.parent() {
        create_dir(dir)?;
    }
    // Escribir aparte y renombrar: launchd nunca ve un plist a medias.
    let tmp = path.with_extension("plist.tmp");
    std::fs::write(&tmp, text).map_err(err)?;
    std::fs::rename(&tmp, path).map_err(err)
}

/// `launchctl` de verdad, en la sesión gráfica del usuario.
#[cfg(target_os = "macos")]
pub mod system {
    use std::path::Path;
    use std::process::Command;
    use std::time::Duration;

    use super::Launchd;

    const LAUNCHCTL: &str = "/bin/launchctl";

    pub struct SystemLaunchd {
        domain: String,
    }

    impl SystemLaunchd {
        /// El dominio `gui/<uid>` del usuario que corre la app.
        pub fn current_user() -> Result<Self, String> {
            let out = Command::new("/usr/bin/id")
                .arg("-u")
                .output()
                .map_err(|e| format!("no se pudo correr id -u: {e}"))?;
            let uid = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if !out.status.success() || uid.is_empty() || !uid.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("id -u devolvió «{uid}»"));
            }
            Ok(SystemLaunchd {
                domain: format!("gui/{uid}"),
            })
        }

        fn run(&self, args: &[&str]) -> Result<(), String> {
            let out = Command::new(LAUNCHCTL)
                .args(args)
                .output()
                .map_err(|e| format!("no se pudo correr launchctl: {e}"))?;
            if out.status.success() {
                Ok(())
            } else {
                Err(format!(
                    "launchctl {}: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&out.stderr).trim()
                ))
            }
        }
    }

    impl Launchd for SystemLaunchd {
        fn is_loaded(&self, label: &str) -> bool {
            Command::new(LAUNCHCTL)
                .args(["print", format!("{}/{label}", self.domain).as_str()])
                .output()
                .is_ok_and(|o| o.status.success())
        }

        fn bootout(&self, label: &str) -> Result<(), String> {
            let target = format!("{}/{label}", self.domain);
            if self.run(&["bootout", target.as_str()]).is_err() && self.is_loaded(label) {
                return Err(format!("no se pudo detener {target}"));
            }
            Ok(())
        }

        fn bootstrap(&self, plist: &Path) -> Result<(), String> {
            let plist = plist.to_string_lossy();
            // Justo después de un `bootout`, launchd puede tardar en soltar el servicio
            // («Bootstrap failed: 5»): se reintenta un par de veces.
            let mut last = String::new();
            for attempt in 0..4 {
                if attempt > 0 {
                    std::thread::sleep(Duration::from_millis(500));
                }
                match self.run(&["bootstrap", self.domain.as_str(), &*plist]) {
                    Ok(()) => return Ok(()),
                    Err(e) => last = e,
                }
            }
            Err(last)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn spec(dir: &Path) -> AgentSpec {
        let program = dir.join("Tandem.app/Contents/MacOS/tandemd");
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, b"").unwrap();
        AgentSpec {
            program,
            log: dir.join("Logs/Tandem/tandemd.log"),
            version: "0.1.0".into(),
        }
    }

    #[derive(Default)]
    struct FakeLaunchd {
        loaded: RefCell<bool>,
        calls: RefCell<Vec<String>>,
        fail_bootstrap: bool,
    }

    impl Launchd for FakeLaunchd {
        fn is_loaded(&self, _: &str) -> bool {
            *self.loaded.borrow()
        }
        fn bootout(&self, label: &str) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("bootout {label}"));
            *self.loaded.borrow_mut() = false;
            Ok(())
        }
        fn bootstrap(&self, plist: &Path) -> Result<(), String> {
            self.calls.borrow_mut().push(format!(
                "bootstrap {}",
                plist.file_name().unwrap().display()
            ));
            if self.fail_bootstrap {
                return Err("Bootstrap failed: 5".into());
            }
            *self.loaded.borrow_mut() = true;
            Ok(())
        }
    }

    impl FakeLaunchd {
        fn calls(&self) -> Vec<String> {
            self.calls.borrow_mut().drain(..).collect()
        }
    }

    #[test]
    fn plist_content() {
        let s = AgentSpec {
            program: "/Applications/Tandem.app/Contents/MacOS/tandemd".into(),
            log: "/Users/fran/Library/Logs/Tandem/tandemd.log".into(),
            version: "0.1.0".into(),
        };
        let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>com.franc1sc0sv.tandem.tandemd</string>
	<key>ProgramArguments</key>
	<array>
		<string>/Applications/Tandem.app/Contents/MacOS/tandemd</string>
	</array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>TANDEM_APP_VERSION</key>
		<string>0.1.0</string>
	</dict>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ThrottleInterval</key>
	<integer>30</integer>
	<key>LimitLoadToSessionType</key>
	<string>Aqua</string>
	<key>ProcessType</key>
	<string>Interactive</string>
	<key>StandardOutPath</key>
	<string>/Users/fran/Library/Logs/Tandem/tandemd.log</string>
	<key>StandardErrorPath</key>
	<string>/Users/fran/Library/Logs/Tandem/tandemd.log</string>
</dict>
</plist>
"#;
        assert_eq!(plist(&s), expected);
    }

    #[test]
    fn plist_escapes_paths() {
        let s = AgentSpec {
            program: "/Users/a&b/Apps/<T>.app/Contents/MacOS/tandemd".into(),
            log: "/l".into(),
            version: "1\"2".into(),
        };
        let p = plist(&s);
        assert!(
            p.contains("<string>/Users/a&amp;b/Apps/&lt;T&gt;.app/Contents/MacOS/tandemd</string>")
        );
        assert!(p.contains("<string>1&quot;2</string>"));
        assert!(!p.contains("a&b"));
    }

    #[test]
    fn standard_locations() {
        let home = Path::new("/Users/fran");
        assert_eq!(
            plist_path(home),
            Path::new("/Users/fran/Library/LaunchAgents/com.franc1sc0sv.tandem.tandemd.plist")
        );
        assert_eq!(
            log_path(home),
            Path::new("/Users/fran/Library/Logs/Tandem/tandemd.log")
        );
    }

    #[test]
    fn sidecar_next_to_the_app_binary() {
        assert_eq!(
            sidecar_for(Path::new(
                "/Applications/Tandem.app/Contents/MacOS/tandem-ui"
            )),
            Ok("/Applications/Tandem.app/Contents/MacOS/tandemd".into())
        );
        assert_eq!(
            sidecar_for(Path::new(
                "/Users/f/Apps/Tandem.app/Contents/MacOS/tandem-ui"
            )),
            Ok("/Users/f/Apps/Tandem.app/Contents/MacOS/tandemd".into())
        );
    }

    #[test]
    fn no_agent_from_unstable_places() {
        assert_eq!(
            sidecar_for(Path::new("/repo/target/debug/tandem-ui")),
            Err(Placement::NotBundled)
        );
        assert_eq!(
            sidecar_for(Path::new("tandem-ui")),
            Err(Placement::NotBundled)
        );
        assert_eq!(
            sidecar_for(Path::new("/x/Tandem/Contents/MacOS/tandem-ui")),
            Err(Placement::NotBundled)
        );
        assert_eq!(
            sidecar_for(Path::new(
                "/private/var/folders/xy/T/AppTranslocation/1234/d/Tandem.app/Contents/MacOS/tandem-ui"
            )),
            Err(Placement::Translocated)
        );
        assert_eq!(
            sidecar_for(Path::new(
                "/Volumes/Tandem/Tandem.app/Contents/MacOS/tandem-ui"
            )),
            Err(Placement::DiskImage)
        );
    }

    #[test]
    fn first_install_writes_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(dir.path());
        let file = plist_path(&dir.path().join("home"));
        let ld = FakeLaunchd::default();
        assert_eq!(ensure(&file, &s, &ld).unwrap(), Outcome::Created);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), plist(&s));
        assert!(s.log.parent().unwrap().is_dir());
        assert!(!file.with_extension("plist.tmp").exists());
        assert_eq!(
            ld.calls(),
            ["bootstrap com.franc1sc0sv.tandem.tandemd.plist"]
        );
    }

    #[test]
    fn up_to_date_and_loaded_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(dir.path());
        let file = dir.path().join("a.plist");
        let ld = FakeLaunchd::default();
        ensure(&file, &s, &ld).unwrap();
        ld.calls();
        assert_eq!(ensure(&file, &s, &ld).unwrap(), Outcome::Unchanged);
        assert!(ld.calls().is_empty());
    }

    #[test]
    fn up_to_date_but_unloaded_is_loaded_again() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(dir.path());
        let file = dir.path().join("a.plist");
        let ld = FakeLaunchd::default();
        ensure(&file, &s, &ld).unwrap();
        ld.calls();
        *ld.loaded.borrow_mut() = false;
        assert_eq!(ensure(&file, &s, &ld).unwrap(), Outcome::Loaded);
        assert_eq!(ld.calls(), ["bootstrap a.plist"]);
    }

    #[test]
    fn new_version_replaces_and_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = spec(dir.path());
        let file = dir.path().join("a.plist");
        let ld = FakeLaunchd::default();
        ensure(&file, &s, &ld).unwrap();
        ld.calls();
        s.version = "0.2.0".into();
        assert_eq!(ensure(&file, &s, &ld).unwrap(), Outcome::Updated);
        assert!(std::fs::read_to_string(&file).unwrap().contains("0.2.0"));
        assert_eq!(
            ld.calls(),
            [
                "bootout com.franc1sc0sv.tandem.tandemd",
                "bootstrap a.plist"
            ]
        );
    }

    #[test]
    fn foreign_plist_not_loaded_is_replaced_without_bootout() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(dir.path());
        let file = dir.path().join("a.plist");
        std::fs::write(&file, "viejo").unwrap();
        let ld = FakeLaunchd::default();
        assert_eq!(ensure(&file, &s, &ld).unwrap(), Outcome::Updated);
        assert_eq!(ld.calls(), ["bootstrap a.plist"]);
    }

    #[test]
    fn missing_sidecar_touches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let s = AgentSpec {
            program: dir.path().join("no-existe"),
            log: dir.path().join("l/t.log"),
            version: "0.1.0".into(),
        };
        let file = dir.path().join("a.plist");
        let ld = FakeLaunchd::default();
        assert!(matches!(
            ensure(&file, &s, &ld),
            Err(InstallError::MissingSidecar(_))
        ));
        assert!(!file.exists());
        assert!(ld.calls().is_empty());
    }

    #[test]
    fn launchctl_failure_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(dir.path());
        let file = dir.path().join("a.plist");
        let ld = FakeLaunchd {
            fail_bootstrap: true,
            ..FakeLaunchd::default()
        };
        let e = ensure(&file, &s, &ld).unwrap_err();
        assert!(e.to_string().contains("Bootstrap failed: 5"), "{e}");
        // El plist queda escrito: el próximo arranque de la app solo intenta cargarlo.
        assert_eq!(std::fs::read_to_string(&file).unwrap(), plist(&s));
    }
    #[test]
    fn install_from_a_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(dir.path());
        let exe = s.program.with_file_name("tandem-ui");
        let home = dir.path().join("home");
        let ld = FakeLaunchd::default();
        let r = install_from(&exe, &home, "0.1.0", &ld);
        assert!(r.ok, "{r:?}");
        assert_eq!(r.text, Outcome::Created.describe());
        let text = std::fs::read_to_string(plist_path(&home)).unwrap();
        // En Windows, `s.program` mezcla separadores: se compara con el que calcula la app.
        let program = sidecar_for(&exe).unwrap();
        assert!(program.is_file());
        assert!(text.contains(&format!(
            "<string>{}</string>",
            escape(&program.to_string_lossy())
        )));
        assert!(log_path(&home).parent().unwrap().is_dir());
        // La segunda vez no hay nada que hacer.
        let r = install_from(&exe, &home, "0.1.0", &ld);
        assert_eq!(r.text, Outcome::Unchanged.describe());
    }

    #[test]
    fn install_from_a_dev_build_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let ld = FakeLaunchd::default();
        let r = install_from(
            Path::new("/repo/target/debug/tandem-ui"),
            dir.path(),
            "0.1.0",
            &ld,
        );
        assert!(!r.ok);
        assert!(r.text.contains("desarrollo"), "{}", r.text);
        assert!(!plist_path(dir.path()).exists());
        assert!(ld.calls().is_empty());
    }
}
