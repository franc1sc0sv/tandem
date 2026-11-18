//! El servicio de administrador que el asistente instala (T-131, `F-Asis1`).
//!
//! - macOS: un LaunchDaemon (`/Library/LaunchDaemons`) con el `tandemd` del `.app`. Escribir
//!   ahí pide credenciales: se piden con el diálogo del sistema (`osascript ... with
//!   administrator privileges`). El daemon usa su propio directorio de estado, para no
//!   pelear con el agente de la sesión.
//! - Windows: la tarea elevada de T-104 (`tandemd --install` con UAC). Antes de pedir UAC se
//!   hacen las mismas comprobaciones de ACL que T-104: elevar un ejecutable que escribe un
//!   usuario normal sería regalar un salto de UAC. El `--install` elevado las repite.
//!
//! Todo lo que decide es portable. Lo que eleva privilegios va detrás de [`Elevator`] y el
//! estado detrás de [`Probe`]; las pruebas usan dobles y nunca elevan.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tandemd::install::PathAcl;

/// Etiqueta del LaunchDaemon (distinta de la del LaunchAgent de la sesión).
pub const DAEMON_LABEL: &str = "com.franc1sc0sv.tandem.admin";
pub const DAEMON_PLIST: &str = "/Library/LaunchDaemons/com.franc1sc0sv.tandem.admin.plist";
/// Carpeta de root donde vive la copia que corre como root: el `tandemd` del `.app` lo puede
/// reemplazar quien escriba en la app, y eso sería root para ese usuario.
pub const DAEMON_BIN_DIR: &str = "/Library/PrivilegedHelperTools";
pub const DAEMON_BIN: &str = "/Library/PrivilegedHelperTools/com.franc1sc0sv.tandem.admin";
pub const DAEMON_DATA: &str = "/Library/Application Support/Tandem";

/// Lo que reporta la fila del asistente. `pending`: hay que instalarlo (o quedó a medias).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AdminState {
    Off,
    Pending,
    Ok,
}

/// Lo que la UI dibuja: estado, si hay botón «Instalar» y, si no lo hay, por qué.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminView {
    pub state: AdminState,
    pub can_install: bool,
    pub note: String,
}

/// Dónde corre la app y con qué `tandemd` se instalaría.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Platform {
    Mac {
        daemon: PathBuf,
    },
    Windows {
        /// `tandemd.exe`: lo que se lanza elevado con `--install`.
        daemon: PathBuf,
        /// Lo que la tarea registra para arrancar: `tandemw.exe` si existe, si no `tandemd.exe`.
        /// Es el mismo que elige el `--install` elevado (`launcher_for`).
        launcher: PathBuf,
    },
    /// Otro sistema, o una compilación sin `.app` (desarrollo): no hay nada que instalar.
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdminError {
    #[error("{0}")]
    NotApplicable(String),
    #[error("se canceló la petición de administrador")]
    Cancelled,
    #[error("no se pudo elevar: {0}")]
    Failed(String),
}

/// Una orden con privilegios.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Elevation {
    /// Un script de shell como administrador (macOS: diálogo del sistema).
    MacShell { script: String, prompt: String },
    /// `<program> <arg>` con UAC (Windows).
    WinUac { program: PathBuf, arg: String },
}

pub trait Elevator {
    fn run(&self, what: &Elevation) -> Result<(), AdminError>;
}

pub trait Probe {
    /// macOS: si el plist existe y si launchd lo tiene cargado. Windows: si la tarea existe.
    fn state(&self, platform: &Platform) -> AdminState;
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// El plist del LaunchDaemon: arranca con el sistema y lo levanta si cae con error.
pub fn daemon_plist(program: &Path) -> String {
    let program = escape_xml(&program.to_string_lossy());
    let data = escape_xml(DAEMON_DATA);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{DAEMON_LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{program}</string>
		<string>--data-dir</string>
		<string>{data}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ThrottleInterval</key>
	<integer>30</integer>
</dict>
</plist>
"#
    )
}

/// El script que corre como administrador: copia `source` (el `tandemd` del `.app`) a
/// [`DAEMON_BIN`] con dueño root y el plist apunta a esa copia. El plist ya viene escapado como
/// XML (sin `'`), así que cabe entre comillas simples de shell sin más. La ruta de `source` va
/// escapada para shell: una `'` la cerraría.
pub fn mac_install_script(source: &Path) -> String {
    let plist = daemon_plist(Path::new(DAEMON_BIN));
    let source = source.to_string_lossy().replace('\'', r"'\''");
    // `-L` rechaza un enlace en la última pieza justo antes de copiar; los de tramos anteriores
    // los descarta `mac_platform` al resolver la ruta.
    format!(
        "[ ! -L '{source}' ] && /usr/bin/install -d -o root -g wheel -m 755 '{DAEMON_BIN_DIR}' \
         && /usr/bin/install -o root -g wheel -m 755 '{source}' '{DAEMON_BIN}' \
         && /bin/mkdir -p '{DAEMON_DATA}' && /usr/bin/printf '%s' '{plist}' > '{DAEMON_PLIST}' \
         && /usr/sbin/chown root:wheel '{DAEMON_PLIST}' && /bin/chmod 644 '{DAEMON_PLIST}' \
         && {{ /bin/launchctl bootout system/{DAEMON_LABEL} 2>/dev/null; \
         /bin/launchctl bootstrap system '{DAEMON_PLIST}'; }}"
    )
}

pub fn mac_uninstall_script() -> String {
    format!(
        "{{ /bin/launchctl bootout system/{DAEMON_LABEL} 2>/dev/null; \
         /bin/rm -f '{DAEMON_PLIST}' '{DAEMON_BIN}'; }}"
    )
}

/// Los argumentos de `osascript` que muestran el diálogo de administrador del sistema.
pub fn osascript_args(script: &str, prompt: &str) -> Vec<String> {
    let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    vec![
        "-e".to_owned(),
        format!(
            "do shell script {} with prompt {} with administrator privileges",
            q(script),
            q(prompt)
        ),
    ]
}

/// Los argumentos de PowerShell que lanzan `<program> <arg>` con UAC y esperan a que acabe.
pub fn powershell_args(program: &Path, arg: &str) -> Vec<String> {
    let lit = |s: &str| format!("'{}'", s.replace('\'', "''"));
    vec![
        "-NoProfile".to_owned(),
        "-NonInteractive".to_owned(),
        "-Command".to_owned(),
        format!(
            "Start-Process -FilePath {} -ArgumentList {} -Verb RunAs -Wait -WindowStyle Hidden",
            lit(&program.to_string_lossy()),
            lit(arg)
        ),
    ]
}

/// El estado de macOS a partir de lo que se ve: plist puesto y cargado en launchd.
pub fn mac_state(plist_exists: bool, loaded: bool) -> AdminState {
    match (plist_exists, loaded) {
        (_, true) => AdminState::Ok,
        (true, false) => AdminState::Pending,
        (false, false) => AdminState::Off,
    }
}

/// Qué muestra la fila ahora mismo, con la posibilidad de instalar.
pub fn view(platform: &Platform, probe: &dyn Probe, acl: &dyn PathAcl) -> AdminView {
    let state = probe.state(platform);
    let blocked = check(platform, acl).err().map(|e| e.to_string());
    match (state, blocked) {
        (AdminState::Ok, _) => AdminView {
            state,
            can_install: false,
            note: String::new(),
        },
        (_, Some(note)) => AdminView {
            state,
            can_install: false,
            note,
        },
        (_, None) => AdminView {
            state,
            can_install: true,
            note: String::new(),
        },
    }
}

/// Las comprobaciones previas: si no pasan, no se pide elevación.
pub fn check(platform: &Platform, acl: &dyn PathAcl) -> Result<(), AdminError> {
    match platform {
        Platform::Unsupported(why) => Err(AdminError::NotApplicable(why.clone())),
        // Root copia `source` y la ejecuta: ni ella ni sus ancestros los puede escribir un
        // usuario (si no, quien escriba en el `.app` obtiene root). Igual que en Windows. Tampoco
        // el destino de la copia. El release va sin firmar (D19): no hay `codesign` que valga.
        Platform::Mac { daemon } => {
            if let Some(why) = tandemd::install::not_admin_only(acl, daemon) {
                return Err(AdminError::NotApplicable(format!(
                    "la instalación por usuario no admite el servicio de administrador: {why}. \
                     Mueve Tandem a Aplicaciones (propiedad de root) para instalarlo"
                )));
            }
            match tandemd::install::not_admin_only(acl, Path::new(DAEMON_BIN)) {
                None => Ok(()),
                Some(why) => Err(AdminError::NotApplicable(format!(
                    "no se instala el servicio: {why}"
                ))),
            }
        }
        // Se comprueba lo que se eleva (`tandemd.exe --install`) y lo que la tarea registra.
        Platform::Windows { daemon, launcher } => [daemon, launcher]
            .into_iter()
            .find_map(|p| tandemd::install::not_admin_only(acl, p))
            .map_or(Ok(()), |why| {
                Err(AdminError::NotApplicable(format!(
                    "instalación por usuario: {why}. Instala Tandem en Archivos de programa para el servicio"
                )))
            }),
    }
}

pub fn install(
    platform: &Platform,
    elevator: &dyn Elevator,
    probe: &dyn Probe,
    acl: &dyn PathAcl,
) -> Result<(), AdminError> {
    check(platform, acl)?;
    let what = match platform {
        Platform::Mac { daemon } => Elevation::MacShell {
            script: mac_install_script(daemon),
            prompt: "Tandem instala el servicio de administrador.".to_owned(),
        },
        Platform::Windows { daemon, .. } => Elevation::WinUac {
            program: daemon.clone(),
            arg: "--install".to_owned(),
        },
        Platform::Unsupported(why) => return Err(AdminError::NotApplicable(why.clone())),
    };
    elevator.run(&what)?;
    // El `--install` elevado cae a la clave `Run` si sus comprobaciones fallan: eso no es éxito.
    match probe.state(platform) {
        AdminState::Ok => Ok(()),
        _ => Err(AdminError::Failed("el servicio no quedó activo".to_owned())),
    }
}

pub fn uninstall(platform: &Platform, elevator: &dyn Elevator) -> Result<(), AdminError> {
    let what = match platform {
        Platform::Mac { .. } => Elevation::MacShell {
            script: mac_uninstall_script(),
            prompt: "Tandem quita el servicio de administrador.".to_owned(),
        },
        Platform::Windows { daemon, .. } => Elevation::WinUac {
            program: daemon.clone(),
            arg: "--uninstall".to_owned(),
        },
        Platform::Unsupported(why) => return Err(AdminError::NotApplicable(why.clone())),
    };
    elevator.run(&what)
}

/// La plataforma según el ejecutable de la app.
pub fn platform_for(exe: &Path) -> Platform {
    if cfg!(target_os = "macos") {
        mac_platform(exe, |p| std::fs::canonicalize(p))
    } else if cfg!(windows) {
        let daemon = exe.with_file_name("tandemd.exe");
        let launcher = tandemd::install::launcher_for(&daemon, |p| p.is_file());
        Platform::Windows { daemon, launcher }
    } else {
        Platform::Unsupported(
            "el servicio de administrador solo existe en macOS y Windows".to_owned(),
        )
    }
}

/// El `tandemd` del `.app` como lo verá root: la ruta ya resuelta (`resolve` = `realpath`) y
/// sin enlaces. Si resolverla cambia algo es que hay un enlace en el camino: se descarta.
pub fn mac_platform(exe: &Path, resolve: impl Fn(&Path) -> std::io::Result<PathBuf>) -> Platform {
    let daemon = match crate::launch_agent::sidecar_for(exe) {
        Ok(d) => d,
        Err(why) => return Platform::Unsupported(why.to_string()),
    };
    match resolve(&daemon) {
        Ok(real) if real == daemon => Platform::Mac { daemon: real },
        Ok(_) => Platform::Unsupported(
            "la ruta de tandemd pasa por un enlace simbólico: el servicio de administrador no la admite"
                .to_owned(),
        ),
        Err(e) => Platform::Unsupported(format!("no se pudo resolver {}: {e}", daemon.display())),
    }
}

/// Permisos de Unix: de administradores es lo que tiene dueño root y nadie más escribe, y no
/// es un enlace. Lo que no existe aún (la carpeta la crea el script como root) no cuenta.
#[cfg(unix)]
pub struct UnixAcl;

/// El grupo `admin` de macOS.
#[cfg(unix)]
const ADMIN_GID: u32 = 80;

#[cfg(unix)]
impl PathAcl for UnixAcl {
    fn admin_only(&self, path: &Path, _: tandemd::install::acl::Rigor) -> std::io::Result<bool> {
        use std::os::unix::fs::MetadataExt;
        match std::fs::symlink_metadata(path) {
            // Dueño root, sin escritura de «otros» y la del grupo solo si es `admin` (80, como
            // `/Applications`: `root:admin 775`).
            Ok(m) => {
                let group_ok = m.mode() & 0o020 == 0 || m.gid() == ADMIN_GID;
                Ok(
                    !m.file_type().is_symlink()
                        && m.uid() == 0
                        && m.mode() & 0o002 == 0
                        && group_ok,
                )
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(e) => Err(e),
        }
    }
}

/// El sistema de verdad: `osascript` / PowerShell y `launchctl` / el Programador de tareas.
pub struct System;

impl Elevator for System {
    fn run(&self, what: &Elevation) -> Result<(), AdminError> {
        let (bin, args) = match what {
            Elevation::MacShell { script, prompt } => ("osascript", osascript_args(script, prompt)),
            Elevation::WinUac { program, arg } => ("powershell.exe", powershell_args(program, arg)),
        };
        let out = std::process::Command::new(bin)
            .args(args)
            .output()
            .map_err(|e| AdminError::Failed(format!("{bin}: {e}")))?;
        if out.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&out.stderr);
        // osascript: -128 es «Cancelar»; PowerShell: UAC rechazado.
        if err.contains("-128") || err.contains("canceled by the user") {
            return Err(AdminError::Cancelled);
        }
        Err(AdminError::Failed(err.trim().to_owned()))
    }
}

impl Probe for System {
    fn state(&self, platform: &Platform) -> AdminState {
        match platform {
            Platform::Mac { .. } => {
                let exists = Path::new(DAEMON_PLIST).exists();
                let loaded = std::process::Command::new("/bin/launchctl")
                    .args(["print", &format!("system/{DAEMON_LABEL}")])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                mac_state(exists, loaded)
            }
            #[cfg(windows)]
            Platform::Windows { .. } => {
                use tandemd::install::{HighestTask, Scheduler};
                if HighestTask.exists().unwrap_or(false) {
                    AdminState::Ok
                } else {
                    AdminState::Off
                }
            }
            _ => AdminState::Off,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io;

    use proptest::prelude::*;
    use tandemd::install::acl::Rigor;

    use super::*;

    struct Acl(bool);
    impl PathAcl for Acl {
        fn admin_only(&self, _: &Path, _: Rigor) -> io::Result<bool> {
            Ok(self.0)
        }
    }

    /// Registra lo que se le pide elevar; nunca eleva. Antes de elevar el servicio está `off`.
    struct Fake {
        asked: RefCell<Vec<Elevation>>,
        result: Result<(), AdminError>,
        after: AdminState,
    }
    impl Fake {
        fn new(result: Result<(), AdminError>, after: AdminState) -> Self {
            Self {
                asked: RefCell::new(vec![]),
                result,
                after,
            }
        }
        fn elevate(&self) {
            self.asked.borrow_mut().push(Elevation::MacShell {
                script: String::new(),
                prompt: String::new(),
            });
        }
    }
    impl Elevator for Fake {
        fn run(&self, what: &Elevation) -> Result<(), AdminError> {
            self.asked.borrow_mut().push(what.clone());
            self.result.clone()
        }
    }
    impl Probe for Fake {
        fn state(&self, _: &Platform) -> AdminState {
            if self.asked.borrow().is_empty() {
                AdminState::Off
            } else {
                self.after
            }
        }
    }

    fn mac() -> Platform {
        Platform::Mac {
            daemon: "/Applications/Tandem.app/Contents/MacOS/tandemd".into(),
        }
    }
    fn win() -> Platform {
        Platform::Windows {
            daemon: r"C:\Program Files\Tandem\tandemd.exe".into(),
            launcher: r"C:\Program Files\Tandem\tandemw.exe".into(),
        }
    }

    /// Solo dice «escribible por el usuario» para las rutas que se le indican.
    struct AclExcept(&'static str);
    impl PathAcl for AclExcept {
        fn admin_only(&self, p: &Path, _: Rigor) -> io::Result<bool> {
            Ok(!p.to_string_lossy().contains(self.0))
        }
    }

    #[test]
    fn mac_copies_the_binary_as_root_and_the_plist_points_at_the_copy() {
        let src = "/Users/ana/Applications/Tandem.app/Contents/MacOS/tandemd";
        let script = mac_install_script(Path::new(src));
        // El binario que corre como root es una copia de root, no el del `.app`.
        assert!(script.contains(&format!(
            "/usr/bin/install -o root -g wheel -m 755 '{src}' '{DAEMON_BIN}'"
        )));
        assert!(script.contains(
            "/usr/bin/install -d -o root -g wheel -m 755 '/Library/PrivilegedHelperTools'"
        ));
        let plist = daemon_plist(Path::new(DAEMON_BIN));
        assert!(plist.contains(&format!("<string>{DAEMON_BIN}</string>")));
        assert!(!plist.contains("Tandem.app"));
        assert!(!script.contains(&format!("<string>{src}</string>")));
        // La copia va antes de cargar el daemon.
        assert!(
            script.find("install -o root").unwrap() < script.find("launchctl bootstrap").unwrap()
        );
    }

    /// Como un Mac: `/Applications` es de root; `/Users/<u>/...` lo escribe el usuario.
    #[test]
    fn mac_source_in_a_user_writable_app_is_rejected_and_one_owned_by_root_is_accepted() {
        let acl = AclExcept("/Users/");
        let user_app = Platform::Mac {
            daemon: "/Users/ana/Applications/Tandem.app/Contents/MacOS/tandemd".into(),
        };
        let f = Fake::new(Ok(()), AdminState::Ok);
        let err = install(&user_app, &f, &f, &acl).unwrap_err();
        let AdminError::NotApplicable(why) = err else {
            panic!("debía ser NotApplicable")
        };
        assert!(why.contains("instalación por usuario no admite el servicio de administrador"));
        // No se pidió la contraseña y la fila no ofrece «Instalar».
        assert!(f.asked.borrow().is_empty());
        let v = view(&user_app, &f, &acl);
        assert!(!v.can_install);
        assert!(v.note.contains("instalación por usuario"));
        // `/Applications/Tandem.app` con dueño root sí.
        install(&mac(), &f, &f, &acl).unwrap();
        assert_eq!(f.asked.borrow().len(), 1);
    }

    #[test]
    fn mac_script_refuses_symlinks_before_copying() {
        let script =
            mac_install_script(Path::new("/Applications/Tandem.app/Contents/MacOS/tandemd"));
        let guard = script
            .find("! -L '/Applications/Tandem.app/Contents/MacOS/tandemd'")
            .unwrap();
        assert!(guard < script.find("install -o root").unwrap());
    }

    #[test]
    fn mac_platform_resolves_the_source_and_rejects_symlinks() {
        let exe = Path::new("/Applications/Tandem.app/Contents/MacOS/tandem-ui");
        let same = |p: &Path| Ok(p.to_path_buf());
        assert!(matches!(mac_platform(exe, same), Platform::Mac { .. }));
        // La ruta real es otra (un enlace en algún tramo): no vale.
        let linked = |_: &Path| Ok(PathBuf::from("/Users/ana/evil/tandemd"));
        let Platform::Unsupported(why) = mac_platform(exe, linked) else {
            panic!("debía rechazarse")
        };
        assert!(why.contains("enlace"));
        let missing = |_: &Path| Err(io::Error::from(io::ErrorKind::NotFound));
        assert!(matches!(
            mac_platform(exe, missing),
            Platform::Unsupported(_)
        ));
    }

    #[test]
    fn mac_with_a_user_writable_destination_never_elevates() {
        let f = Fake::new(Ok(()), AdminState::Ok);
        let acl = AclExcept("PrivilegedHelperTools");
        let err = install(&mac(), &f, &f, &acl).unwrap_err();
        assert!(matches!(err, AdminError::NotApplicable(_)));
        assert!(f.asked.borrow().is_empty());
        assert!(!view(&mac(), &f, &acl).can_install);
        // Un ancestro escribible (por ejemplo /Library cambiado) también la descarta.
        assert!(check(&mac(), &AclExcept("/Library")).is_err());
    }

    #[test]
    fn windows_validates_the_executables_it_elevates_and_registers() {
        // Se eleva `tandemd.exe --install` y la tarea apunta al lanzador: los dos se comprueban.
        assert!(check(&win(), &AclExcept("tandemw.exe")).is_err());
        assert!(check(&win(), &AclExcept("tandemd.exe")).is_err());
        assert!(check(&win(), &Acl(true)).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn unix_acl_accepts_only_root_owned_paths_without_user_write() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        // Si las pruebas corren como root, su carpeta sí es de root.
        if std::fs::metadata(dir.path()).unwrap().uid() == 0 {
            return;
        }
        // Una carpeta de pruebas es del usuario (no de root): no es solo de administradores.
        assert!(!UnixAcl.admin_only(dir.path(), Rigor::Full).unwrap());
        // Lo que no existe aún lo crea root con dueño root: no cuenta contra la ruta.
        assert!(
            UnixAcl
                .admin_only(&dir.path().join("nuevo"), Rigor::Full)
                .unwrap()
        );
    }

    #[test]
    fn mac_install_asks_admin_with_the_daemon_script() {
        let f = Fake::new(Ok(()), AdminState::Ok);
        install(&mac(), &f, &f, &Acl(true)).unwrap();
        let asked = f.asked.borrow();
        let Elevation::MacShell { script, .. } = &asked[0] else {
            panic!("no es shell")
        };
        assert!(script.contains("/Applications/Tandem.app/Contents/MacOS/tandemd"));
        assert!(script.contains("launchctl bootstrap system"));
        assert!(script.contains("--data-dir"));
    }

    #[test]
    fn windows_install_runs_dash_install_with_uac_after_the_acl_check() {
        let f = Fake::new(Ok(()), AdminState::Ok);
        install(&win(), &f, &f, &Acl(true)).unwrap();
        assert_eq!(
            f.asked.borrow()[0],
            Elevation::WinUac {
                program: r"C:\Program Files\Tandem\tandemd.exe".into(),
                arg: "--install".into()
            }
        );
    }

    #[test]
    fn windows_with_a_writable_launcher_never_elevates() {
        let f = Fake::new(Ok(()), AdminState::Ok);
        let err = install(&win(), &f, &f, &Acl(false)).unwrap_err();
        assert!(matches!(err, AdminError::NotApplicable(_)));
        assert!(f.asked.borrow().is_empty());
        let v = view(&win(), &f, &Acl(false));
        assert!(!v.can_install);
        assert!(v.note.contains("instalación por usuario"));
    }

    #[test]
    fn unsupported_platform_explains_and_offers_no_button() {
        let p = Platform::Unsupported("la app no corre desde un Tandem.app".into());
        let f = Fake::new(Ok(()), AdminState::Ok);
        assert!(install(&p, &f, &f, &Acl(true)).is_err());
        let v = view(&p, &f, &Acl(true));
        assert!(!v.can_install);
        assert_eq!(v.note, "la app no corre desde un Tandem.app");
    }

    #[test]
    fn cancelled_elevation_propagates() {
        let f = Fake::new(Err(AdminError::Cancelled), AdminState::Ok);
        assert_eq!(
            install(&mac(), &f, &f, &Acl(true)),
            Err(AdminError::Cancelled)
        );
    }

    #[test]
    fn a_fallback_to_the_run_key_is_not_success() {
        // El `--install` elevado salió bien pero no dejó la tarea: la fila no puede quedar en «ok».
        let f = Fake::new(Ok(()), AdminState::Off);
        assert!(matches!(
            install(&win(), &f, &f, &Acl(true)),
            Err(AdminError::Failed(_))
        ));
    }

    #[test]
    fn view_reports_the_three_states() {
        let f = Fake::new(Ok(()), AdminState::Pending);
        let v = view(&mac(), &f, &Acl(true));
        assert_eq!((v.state, v.can_install), (AdminState::Off, true));
        f.elevate();
        assert_eq!(view(&mac(), &f, &Acl(true)).state, AdminState::Pending);
        let ok = Fake::new(Ok(()), AdminState::Ok);
        ok.elevate();
        let v = view(&mac(), &ok, &Acl(true));
        assert_eq!((v.state, v.can_install), (AdminState::Ok, false));
    }

    #[test]
    fn mac_state_table() {
        assert_eq!(mac_state(false, false), AdminState::Off);
        assert_eq!(mac_state(true, false), AdminState::Pending);
        assert_eq!(mac_state(true, true), AdminState::Ok);
    }

    #[test]
    fn uninstall_asks_for_the_matching_command() {
        let f = Fake::new(Ok(()), AdminState::Off);
        uninstall(&mac(), &f).unwrap();
        uninstall(&win(), &f).unwrap();
        let asked = f.asked.borrow();
        assert!(
            matches!(&asked[0], Elevation::MacShell { script, .. } if script.contains("rm -f"))
        );
        assert!(matches!(&asked[1], Elevation::WinUac { arg, .. } if arg == "--uninstall"));
    }

    #[test]
    fn osascript_shows_the_system_dialog() {
        let a = osascript_args("echo \"x\"", "Pide");
        assert_eq!(a[0], "-e");
        assert!(a[1].ends_with("with administrator privileges"));
        assert!(a[1].contains(r#"\"x\""#));
    }

    #[test]
    fn powershell_runs_as_with_the_path_quoted() {
        let a = powershell_args(Path::new(r"C:\Users\O'Neil\tandemd.exe"), "--install");
        assert!(a[3].contains("-Verb RunAs"));
        assert!(a[3].contains(r"'C:\Users\O''Neil\tandemd.exe'"));
    }

    proptest! {
        /// Sea cual sea la ruta, el plist no deja comillas simples (cabe en el script de shell).
        #[test]
        fn plist_is_safe_for_the_shell_script(path in "/[ -~]{0,40}") {
            let plist = daemon_plist(Path::new(&path));
            prop_assert!(!plist.contains('\''));
            prop_assert!(mac_install_script(Path::new(&path)).contains(DAEMON_PLIST));
        }
    }
}
