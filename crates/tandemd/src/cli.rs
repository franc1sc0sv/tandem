//! Argumentos de la línea de órdenes. Son pocos: no hace falta un crate para esto.

use std::ffi::OsString;
use std::path::PathBuf;

pub const USAGE: &str = "\
Uso: tandemd [opciones]

Opciones:
  --config <archivo>        tandem.toml a usar (por omisión, el del directorio de la app)
  --data-dir <dir>          directorio de estado, logs y socket (por omisión, el de la app)
  --check-config <archivo>  valida la configuración y sale (código ≠ 0 si es inválida)
  -h, --help                muestra esta ayuda
  -V, --version             muestra la versión";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run(RunArgs),
    CheckConfig(PathBuf),
    Help,
    Version,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunArgs {
    pub config: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CliError {
    #[error("falta el valor de `{0}`")]
    MissingValue(&'static str),
    #[error("opción desconocida: `{0}`")]
    Unknown(String),
    #[error("`{0}` aparece dos veces")]
    Repeated(&'static str),
}

/// `args` sin el nombre del programa.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, CliError> {
    let mut args = args.into_iter();
    let mut run = RunArgs::default();
    let mut check = None;
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
            Some("--check-config") => value("--check-config", &mut check)?,
            _ => return Err(CliError::Unknown(arg.to_string_lossy().into_owned())),
        }
    }
    Ok(match check {
        Some(path) => Command::CheckConfig(path),
        None => Command::Run(run),
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
