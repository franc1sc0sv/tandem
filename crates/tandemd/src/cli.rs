//! Command-line arguments. There are few: a crate is not needed for this.

use std::ffi::OsString;
use std::path::PathBuf;

pub const USAGE: &str = "\
Usage: tandemd [options]
       tandemd links [--config <file>] [--data-dir <dir>]

Options:
  --config <file>           tandem.toml to use (by default, the one in the app directory;
                            if it does not exist, the agent generates it with an empty desk)
  --data-dir <dir>          directory for state, logs and socket (by default, the app's)
  --check-config <file>     validates the configuration and exits (code != 0 if invalid)
  --install                 registers the startup with the session (Windows; used by the
                            installer). With administrator it also refreshes the system
                            service if it is installed
  --with-service            with --install: also installs the system service for the
                            secure desktop (UAC, sign-in screen; needs administrator)
  --uninstall               removes the startup with the session and, with administrator,
                            the system service (leaves the configuration)
  --install-service         installs or refreshes only the system service (administrator)
  --uninstall-service       removes only the system service (administrator)
  links                     lists the network interfaces, their detected type, the address the
                            wired link would use and the state of each path (spike S5)
  -h, --help                shows this help
  -V, --version             shows the version";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run(RunArgs),
    CheckConfig(PathBuf),
    /// The `RunArgs` paths are the ones the agent will use when starting with the session.
    /// `with_service`: also install the secure desktop service (T-027).
    Install {
        args: RunArgs,
        with_service: bool,
    },
    Uninstall,
    /// Only the secure desktop service (T-027).
    InstallService,
    UninstallService,
    /// `--service`: run as the Windows service (started by the SCM, not by people).
    Service,
    /// `--input-helper`: the helper the service launches in the console session.
    InputHelper,
    /// `tandemd links`: diagnostics of the wired links (T-094). It uses the same paths
    /// as when running the agent.
    Links(RunArgs),
    Help,
    Version,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunArgs {
    pub config: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    /// Simulated backends (`--sim-backends`). It only exists with the
    /// `sim-backends` feature, which the end-to-end tests with the binary use.
    pub sim_backends: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CliError {
    #[error("missing value for `{0}`")]
    MissingValue(&'static str),
    #[error("unknown option: `{0}`")]
    Unknown(String),
    #[error("`{0}` appears twice")]
    Repeated(&'static str),
    #[error("`{0}` and `{1}` do not go together")]
    Conflict(&'static str, &'static str),
}

/// It does not appear in [`USAGE`]: it is not for users.
const SIM_FLAG: &str = "--sim-backends";
/// Neither do these: the SCM and the service start them.
const SERVICE_FLAG: &str = "--service";
const HELPER_FLAG: &str = "--input-helper";
const WITH_SERVICE: &str = "--with-service";

/// What to do instead of starting the agent; there can be only one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Check,
    Install,
    Uninstall,
    InstallService,
    UninstallService,
    Service,
    InputHelper,
}

impl Action {
    fn flag(self) -> &'static str {
        match self {
            Action::Check => "--check-config",
            Action::Install => "--install",
            Action::Uninstall => "--uninstall",
            Action::InstallService => "--install-service",
            Action::UninstallService => "--uninstall-service",
            Action::Service => SERVICE_FLAG,
            Action::InputHelper => HELPER_FLAG,
        }
    }
}

/// `args` without the program name.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, CliError> {
    let mut args = args.into_iter().peekable();
    let links = args.next_if(|a| a.to_str() == Some("links")).is_some();
    let mut run = RunArgs::default();
    let mut check = None;
    let mut with_service = false;
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
            Some("--install-service") => set_action(Action::InstallService)?,
            Some("--uninstall-service") => set_action(Action::UninstallService)?,
            Some(SERVICE_FLAG) => set_action(Action::Service)?,
            Some(HELPER_FLAG) => set_action(Action::InputHelper)?,
            Some(WITH_SERVICE) => {
                if with_service {
                    return Err(CliError::Repeated(WITH_SERVICE));
                }
                with_service = true;
            }
            Some(SIM_FLAG) if cfg!(feature = "sim-backends") => {
                if run.sim_backends {
                    return Err(CliError::Repeated(SIM_FLAG));
                }
                run.sim_backends = true;
            }
            _ => return Err(CliError::Unknown(arg.to_string_lossy().into_owned())),
        }
    }
    // Simulating only makes sense when running the agent.
    if let (true, Some(action)) = (run.sim_backends, action) {
        return Err(CliError::Conflict(action.flag(), SIM_FLAG));
    }
    // `--with-service` only modifies `--install`.
    if with_service && action != Some(Action::Install) {
        return Err(CliError::Conflict(
            action.map_or("running the agent", Action::flag),
            WITH_SERVICE,
        ));
    }
    if links {
        return match action {
            Some(action) => Err(CliError::Conflict("links", action.flag())),
            None if run.sim_backends => Err(CliError::Conflict("links", SIM_FLAG)),
            None => Ok(Command::Links(run)),
        };
    }
    Ok(match (action, check) {
        (Some(Action::Check), Some(path)) => Command::CheckConfig(path),
        (Some(Action::Install), _) => Command::Install {
            args: run,
            with_service,
        },
        (Some(Action::Uninstall), _) => Command::Uninstall,
        (Some(Action::InstallService), _) => Command::InstallService,
        (Some(Action::UninstallService), _) => Command::UninstallService,
        (Some(Action::Service), _) => Command::Service,
        (Some(Action::InputHelper), _) => Command::InputHelper,
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
        assert_eq!(
            p(&["--install"]),
            Ok(Command::Install {
                args: RunArgs::default(),
                with_service: false
            })
        );
        assert_eq!(
            p(&["--config", "a.toml", "--install"]),
            Ok(Command::Install {
                args: RunArgs {
                    config: Some("a.toml".into()),
                    ..RunArgs::default()
                },
                with_service: false
            })
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

    #[test]
    fn service_commands() {
        assert_eq!(
            p(&["--install", "--with-service"]),
            Ok(Command::Install {
                args: RunArgs::default(),
                with_service: true
            })
        );
        assert_eq!(
            p(&["--with-service", "--install"]),
            Ok(Command::Install {
                args: RunArgs::default(),
                with_service: true
            })
        );
        assert_eq!(p(&["--install-service"]), Ok(Command::InstallService));
        assert_eq!(p(&["--uninstall-service"]), Ok(Command::UninstallService));
        assert_eq!(p(&["--service"]), Ok(Command::Service));
        assert_eq!(p(&["--input-helper"]), Ok(Command::InputHelper));
        assert_eq!(
            p(&["--with-service"]),
            Err(CliError::Conflict("running the agent", "--with-service"))
        );
        assert_eq!(
            p(&["--uninstall", "--with-service"]),
            Err(CliError::Conflict("--uninstall", "--with-service"))
        );
        assert_eq!(
            p(&["--install", "--with-service", "--with-service"]),
            Err(CliError::Repeated("--with-service"))
        );
        assert_eq!(
            p(&["--service", "--input-helper"]),
            Err(CliError::Conflict("--service", "--input-helper"))
        );
        assert_eq!(
            p(&["links", "--service"]),
            Err(CliError::Conflict("links", "--service"))
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
    fn links_carries_the_paths_but_not_the_actions() {
        assert_eq!(p(&["links"]), Ok(Command::Links(RunArgs::default())));
        assert_eq!(
            p(&["links", "--config", "a.toml"]),
            Ok(Command::Links(RunArgs {
                config: Some("a.toml".into()),
                ..RunArgs::default()
            }))
        );
        assert_eq!(
            p(&["links", "--install"]),
            Err(CliError::Conflict("links", "--install"))
        );
    }

    #[test]
    fn help_and_version() {
        assert_eq!(p(&["--help"]), Ok(Command::Help));
        assert_eq!(p(&["-h", "--nothing"]), Ok(Command::Help));
        assert_eq!(p(&["-V"]), Ok(Command::Version));
    }

    #[test]
    fn errors() {
        assert_eq!(
            p(&["--check-config"]),
            Err(CliError::MissingValue("--check-config"))
        );
        assert_eq!(
            p(&["--nothing"]),
            Err(CliError::Unknown("--nothing".into()))
        );
        assert_eq!(p(&["loose"]), Err(CliError::Unknown("loose".into())));
        assert_eq!(
            p(&["--config", "a", "--config", "b"]),
            Err(CliError::Repeated("--config"))
        );
    }
}
