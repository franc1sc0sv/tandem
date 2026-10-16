//! Argumentos de la línea de órdenes. Son pocos: no hace falta un crate para esto.

use std::ffi::OsString;
use std::path::PathBuf;

pub const USAGE: &str = "\
Uso: tandemd [opciones]

Opciones:
  --config <archivo>        tandem.toml a usar (por omisión, el del directorio de la app;
                            si no existe, el agente lo genera con los monitores que detecta)
  --data-dir <dir>          directorio de estado, logs y socket (por omisión, el de la app)
  --check-config <archivo>  valida la configuración y sale (código ≠ 0 si es inválida)
  --install                 registra el arranque con la sesión (Windows; lo usa el
                            instalador)
  --uninstall               quita el arranque con la sesión (deja la configuración)
  -h, --help                muestra esta ayuda
  -V, --version             muestra la versión";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run(RunArgs),
    CheckConfig(PathBuf),
    /// Las rutas de `RunArgs` son las que usará el agente al arrancar con la sesión.
    Install(RunArgs),
    Uninstall,
    Help,
    Version,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunArgs {
    pub config: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    /// Backends simulados (`--sim-backends`). Solo existe con la feature
    /// `sim-backends`, que usan las pruebas de punta a punta con el binario.
    pub sim_backends: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CliError {
    #[error("falta el valor de `{0}`")]
    MissingValue(&'static str),
    #[error("opción desconocida: `{0}`")]
    Unknown(String),
    #[error("`{0}` aparece dos veces")]
    Repeated(&'static str),
    #[error("`{0}` y `{1}` no van juntas")]
    Conflict(&'static str, &'static str),
}

/// No aparece en [`USAGE`]: no es para usuarios.
const SIM_FLAG: &str = "--sim-backends";

/// Qué hacer en vez de arrancar el agente; solo puede haber una.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Check,
    Install,
    Uninstall,
}

impl Action {
    fn flag(self) -> &'static str {
        match self {
            Action::Check => "--check-config",
            Action::Install => "--install",
            Action::Uninstall => "--uninstall",
        }
    }
}

/// `args` sin el nombre del programa.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, CliError> {
    let mut args = args.into_iter();
    let mut run = RunArgs::default();
    let mut check = None;
    let mut action: Option<Action> = None;
    let mut set_action = |new: Action| match action {
        Some(old) if old == new => Err(CliError::Repeated(new.flag())),
        Some(old) => Err(CliError::Conflict(old.flag(), new.flag())),
        None => {
            action = Some(new);
            Ok(())
        }
    };
    while let Some(arg) = args.next() {
        let mut value = |name: &'static str, slot: &mut Option<PathBuf>| {
            if slot.is_some() {
                return Err(CliError::Repeated(name));
            }
            let v = args.next().ok_or(CliError::MissingValue(name))?;
            *slot = Some(PathBuf::from(v));
            Ok(())
        };
        match arg.to_str() {
            Some("-h" | "--help") => return Ok(Command::Help),
            Some("-V" | "--version") => return Ok(Command::Version),
            Some("--config") => value("--config", &mut run.config)?,
            Some("--data-dir") => value("--data-dir", &mut run.data_dir)?,
            Some("--check-config") => {
                value("--check-config", &mut check)?;
                set_action(Action::Check)?;
            }
            Some("--install") => set_action(Action::Install)?,
            Some("--uninstall") => set_action(Action::Uninstall)?,
            Some(SIM_FLAG) if cfg!(feature = "sim-backends") => {
                if run.sim_backends {
                    return Err(CliError::Repeated(SIM_FLAG));
                }
                run.sim_backends = true;
            }
            _ => return Err(CliError::Unknown(arg.to_string_lossy().into_owned())),
        }
    }
    // Simular solo tiene sentido al correr el agente.
    if let (true, Some(action)) = (run.sim_backends, action) {
        return Err(CliError::Conflict(action.flag(), SIM_FLAG));
    }
    Ok(match (action, check) {
        (Some(Action::Check), Some(path)) => Command::CheckConfig(path),
        (Some(Action::Install), _) => Command::Install(run),
        (Some(Action::Uninstall), _) => Command::Uninstall,
        _ => Command::Run(run),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Command, CliError> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn no_args_runs_with_defaults() {
        assert_eq!(p(&[]), Ok(Command::Run(RunArgs::default())));
    }

    #[test]
    fn run_with_paths() {
        assert_eq!(
            p(&["--config", "a.toml", "--data-dir", "d"]),
            Ok(Command::Run(RunArgs {
                config: Some("a.toml".into()),
                data_dir: Some("d".into()),
                ..RunArgs::default()
            }))
        );
    }

    #[test]
    fn check_config() {
        assert_eq!(
            p(&["--check-config", "t.toml"]),
            Ok(Command::CheckConfig("t.toml".into()))
        );
    }

    #[test]
    fn install_and_uninstall() {
        assert_eq!(p(&["--install"]), Ok(Command::Install(RunArgs::default())));
        assert_eq!(
            p(&["--config", "a.toml", "--install"]),
            Ok(Command::Install(RunArgs {
                config: Some("a.toml".into()),
                ..RunArgs::default()
            }))
        );
        assert_eq!(p(&["--uninstall"]), Ok(Command::Uninstall));
        assert_eq!(
            p(&["--install", "--uninstall"]),
            Err(CliError::Conflict("--install", "--uninstall"))
        );
        assert_eq!(
            p(&["--check-config", "t.toml", "--install"]),
            Err(CliError::Conflict("--check-config", "--install"))
        );
        assert_eq!(
            p(&["--uninstall", "--uninstall"]),
            Err(CliError::Repeated("--uninstall"))
        );
    }

    #[cfg(feature = "sim-backends")]
    #[test]
    fn sim_backends_only_when_running() {
        assert_eq!(
            p(&["--sim-backends", "--data-dir", "d"]),
            Ok(Command::Run(RunArgs {
                data_dir: Some("d".into()),
                sim_backends: true,
                ..RunArgs::default()
            }))
        );
        assert_eq!(
            p(&["--sim-backends", "--sim-backends"]),
            Err(CliError::Repeated("--sim-backends"))
        );
        assert_eq!(
            p(&["--sim-backends", "--install"]),
            Err(CliError::Conflict("--install", "--sim-backends"))
        );
        assert_eq!(
            p(&["--check-config", "t.toml", "--sim-backends"]),
            Err(CliError::Conflict("--check-config", "--sim-backends"))
        );
    }

    #[cfg(not(feature = "sim-backends"))]
    #[test]
    fn sim_backends_is_unknown_without_the_feature() {
        assert_eq!(
            p(&["--sim-backends"]),
            Err(CliError::Unknown("--sim-backends".into()))
        );
    }

    #[test]
    fn help_and_version() {
        assert_eq!(p(&["--help"]), Ok(Command::Help));
        assert_eq!(p(&["-h", "--nada"]), Ok(Command::Help));
        assert_eq!(p(&["-V"]), Ok(Command::Version));
    }

    #[test]
    fn errors() {
        assert_eq!(
            p(&["--check-config"]),
            Err(CliError::MissingValue("--check-config"))
        );
        assert_eq!(p(&["--nada"]), Err(CliError::Unknown("--nada".into())));
        assert_eq!(p(&["suelto"]), Err(CliError::Unknown("suelto".into())));
        assert_eq!(
            p(&["--config", "a", "--config", "b"]),
            Err(CliError::Repeated("--config"))
        );
    }
}
