//! Instalación del agente en la sesión del usuario: `tandemd --install` / `--uninstall`.
//!
//! Lo llama el instalador de Windows (NSIS, `apps/tandem-ui/src-tauri/windows/hooks.nsh`) al
//! instalar y al desinstalar; ver `docs/dev/install-windows.md`. Instalar es idempotente:
//! deja el arranque automático apuntando al lanzador sin consola ([`LAUNCHER`]). No trae
//! configuración: el agente la genera al arrancar con lo que detecta ([`crate::autoconfig`],
//! T-082). Desinstalar quita el arranque y deja la configuración y los datos del usuario
//! (el desinstalador los borra solo si se le pide).
//!
//! El arranque va detrás de [`Autostart`]: en Windows es la clave `Run` de `HKCU`
//! ([`RunKey`]) y la lógica se prueba con un doble en memoria.

use std::io;
use std::path::{Path, PathBuf};

use crate::cli::RunArgs;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::RunKey;

/// Nombre del valor en `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. No es
/// `Tandem` para no chocar con un arranque propio de la app de Tauri.
pub const RUN_VALUE: &str = "tandemd";

/// Lanzador sin consola que arranca `tandemd` con la sesión (`src/bin/tandemw.rs`).
pub const LAUNCHER: &str = "tandemw";

/// El binario del agente, al lado del lanzador.
pub const DAEMON: &str = "tandemd";

/// Dónde se registra el arranque con la sesión.
pub trait Autostart {
    /// La orden registrada, si hay una.
    fn get(&self) -> io::Result<Option<String>>;
    fn set(&mut self, command: &str) -> io::Result<()>;
    /// Quita la orden. `true` si había una.
    fn remove(&mut self) -> io::Result<bool>;
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("no se pudo registrar el arranque automático: {0}")]
    Autostart(#[source] io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    /// La orden que queda registrada.
    pub command: String,
    /// La orden registrada cambió (o no había).
    pub autostart_changed: bool,
}

/// Registra `command` para el arranque con la sesión.
pub fn install(
    autostart: &mut dyn Autostart,
    command: &str,
) -> Result<InstallReport, InstallError> {
    let autostart_changed =
        autostart.get().map_err(InstallError::Autostart)?.as_deref() != Some(command);
    if autostart_changed {
        autostart.set(command).map_err(InstallError::Autostart)?;
    }
    Ok(InstallReport {
        command: command.to_owned(),
        autostart_changed,
    })
}

/// Quita el arranque automático. `true` si estaba registrado.
pub fn uninstall(autostart: &mut dyn Autostart) -> Result<bool, InstallError> {
    autostart.remove().map_err(InstallError::Autostart)
}

/// El lanzador al lado de `exe` si existe (`exists`); si no, el propio `exe`, que también
/// arranca el agente aunque con consola.
pub fn launcher_for(exe: &Path, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let launcher = sibling(exe, LAUNCHER);
    if exists(&launcher) {
        launcher
    } else {
        exe.to_owned()
    }
}

/// `tandemd` al lado del lanzador.
pub fn daemon_for(launcher: &Path) -> PathBuf {
    sibling(launcher, DAEMON)
}

fn sibling(exe: &Path, name: &str) -> PathBuf {
    exe.with_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// La orden de la clave `Run`: el programa y las rutas de `args`, cada uno entre comillas
/// (las rutas de Windows no pueden contener `"`).
pub fn command_line(program: &Path, args: &RunArgs) -> String {
    let mut parts = vec![quote(program)];
    if let Some(config) = &args.config {
        parts.push("--config".into());
        parts.push(quote(config));
    }
    if let Some(data_dir) = &args.data_dir {
        parts.push("--data-dir".into());
        parts.push(quote(data_dir));
    }
    parts.join(" ")
}

fn quote(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

/// Doble de prueba de [`Autostart`].
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct MemAutostart {
    pub command: Option<String>,
    pub writes: usize,
    pub fail: bool,
}

#[cfg(test)]
impl Autostart for MemAutostart {
    fn get(&self) -> io::Result<Option<String>> {
        if self.fail {
            return Err(io::Error::other("registro no disponible"));
        }
        Ok(self.command.clone())
    }

    fn set(&mut self, command: &str) -> io::Result<()> {
        self.writes += 1;
        self.command = Some(command.to_owned());
        Ok(())
    }

    fn remove(&mut self) -> io::Result<bool> {
        if self.fail {
            return Err(io::Error::other("registro no disponible"));
        }
        Ok(self.command.take().is_some())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn install_registers_and_is_idempotent() {
        let mut run = MemAutostart::default();
        let report = install(&mut run, "\"C:\\T\\tandemw.exe\"").unwrap();
        assert_eq!(
            report,
            InstallReport {
                command: "\"C:\\T\\tandemw.exe\"".into(),
                autostart_changed: true,
            }
        );
        assert_eq!(run.command.as_deref(), Some("\"C:\\T\\tandemw.exe\""));
        let again = install(&mut run, "\"C:\\T\\tandemw.exe\"").unwrap();
        assert!(!again.autostart_changed);
        assert_eq!(run.writes, 1, "la misma orden no se reescribe");
    }

    #[test]
    fn reinstall_updates_a_stale_command() {
        let mut run = MemAutostart {
            command: Some("\"C:\\viejo\\tandemd.exe\"".into()),
            ..Default::default()
        };
        let report = install(&mut run, "nuevo").unwrap();
        assert!(report.autostart_changed);
        assert_eq!(run.command.as_deref(), Some("nuevo"));
    }

    #[test]
    fn uninstall_removes_only_autostart() {
        let mut run = MemAutostart::default();
        install(&mut run, "a").unwrap();
        assert!(uninstall(&mut run).unwrap());
        assert_eq!(run.command, None);
        // Desinstalar dos veces no es un error.
        assert!(!uninstall(&mut run).unwrap());
    }

    #[test]
    fn autostart_errors_are_reported() {
        let mut run = MemAutostart {
            fail: true,
            ..Default::default()
        };
        let err = install(&mut run, "a").unwrap_err();
        assert!(matches!(err, InstallError::Autostart(_)), "{err}");
        assert!(matches!(
            uninstall(&mut run),
            Err(InstallError::Autostart(_))
        ));
    }

    #[test]
    fn launcher_is_used_when_present() {
        let exe = Path::new("dir").join(format!("tandemd{}", std::env::consts::EXE_SUFFIX));
        let launcher = Path::new("dir").join(format!("tandemw{}", std::env::consts::EXE_SUFFIX));
        assert_eq!(launcher_for(&exe, |p| p == launcher), launcher);
        assert_eq!(launcher_for(&exe, |_| false), exe);
        assert_eq!(daemon_for(&launcher), exe);
    }

    #[test]
    fn command_line_quotes_paths() {
        let program = Path::new(r"C:\Users\Ana María\AppData\Local\Tandem\tandemw.exe");
        assert_eq!(
            command_line(program, &RunArgs::default()),
            r#""C:\Users\Ana María\AppData\Local\Tandem\tandemw.exe""#
        );
        let args = RunArgs {
            config: Some(r"D:\cfg\tandem.toml".into()),
            data_dir: Some(r"D:\datos".into()),
            ..RunArgs::default()
        };
        assert_eq!(
            command_line(Path::new("t.exe"), &args),
            r#""t.exe" --config "D:\cfg\tandem.toml" --data-dir "D:\datos""#
        );
    }

    proptest! {
        /// Instalar n veces deja lo mismo que instalar una, y desinstalar lo quita todo.
        #[test]
        fn install_converges(n in 1usize..5, previous in proptest::option::of("[a-z]{1,8}")) {
            let mut run = MemAutostart { command: previous, ..Default::default() };
            for _ in 0..n {
                install(&mut run, "orden").unwrap();
            }
            prop_assert_eq!(run.command.as_deref(), Some("orden"));
            prop_assert!(run.writes <= 1);
            prop_assert!(uninstall(&mut run).unwrap());
            prop_assert_eq!(run.command, None);
        }
    }
}
