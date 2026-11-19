//! Installing the agent in the user's session: `tandemd --install` / `--uninstall`.
//!
//! Called by the Windows installer (NSIS, `apps/tandem-ui/src-tauri/windows/hooks.nsh`) on
//! install and uninstall; see `docs/dev/install-windows.md`. Installing is idempotent:
//! it leaves the automatic startup pointing to the console-less launcher ([`LAUNCHER`]). It brings no
//! configuration: the agent generates it on start with what it detects ([`crate::autoconfig`],
//! T-082). Uninstalling removes the startup and leaves the user's configuration and data
//! (the uninstaller deletes them only if asked).
//!
//! Startup goes behind [`Autostart`]: on Windows it is the `HKCU` `Run` key
//! ([`RunKey`]) and the logic is tested with an in-memory double.
//!
//! With an administrator, the agent is also registered as a scheduled task with the highest
//! privileges ([`task`], A7, T-104): what runs in the tunnels inherits those privileges.
//! Without an administrator the `Run` key remains, with the user's.

use std::io;
use std::path::{Path, PathBuf};

use crate::cli::RunArgs;

pub mod acl;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{HighestTask, RunKey, WinAcl};

pub mod task;
pub use task::{
    AgentInstall, NotElevated, PathAcl, Scheduler, TASK_NAME, install_agent, not_admin_only,
    uninstall_agent,
};

/// Name of the value in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. It is not
/// `Tandem` so as not to clash with the Tauri app's own startup.
pub const RUN_VALUE: &str = "tandemd";

/// Console-less launcher that starts `tandemd` with the session (`src/bin/tandemw.rs`).
pub const LAUNCHER: &str = "tandemw";

/// The agent's binary, next to the launcher.
pub const DAEMON: &str = "tandemd";

/// Where the startup with the session is registered.
pub trait Autostart {
    /// The registered command, if there is one.
    fn get(&self) -> io::Result<Option<String>>;
    fn set(&mut self, command: &str) -> io::Result<()>;
    /// Removes the command. `true` if there was one.
    fn remove(&mut self) -> io::Result<bool>;
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("could not register the automatic startup: {0}")]
    Autostart(#[source] io::Error),
    #[error("could not register or remove the scheduled task (requires an administrator): {0}")]
    Task(#[source] io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    /// The command that is left registered.
    pub command: String,
    /// The registered command changed (or there was none).
    pub autostart_changed: bool,
}

/// Registers `command` for startup with the session.
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

/// Removes the automatic startup. `true` if it was registered.
pub fn uninstall(autostart: &mut dyn Autostart) -> Result<bool, InstallError> {
    autostart.remove().map_err(InstallError::Autostart)
}

/// The launcher next to `exe` if it exists (`exists`); otherwise, `exe` itself, which also
/// starts the agent although with a console.
pub fn launcher_for(exe: &Path, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let launcher = sibling(exe, LAUNCHER);
    if exists(&launcher) {
        launcher
    } else {
        exe.to_owned()
    }
}

/// `tandemd` next to the launcher.
pub fn daemon_for(launcher: &Path) -> PathBuf {
    sibling(launcher, DAEMON)
}

fn sibling(exe: &Path, name: &str) -> PathBuf {
    exe.with_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// The `Run` key's command: the program and the `args` paths, each in quotes
/// (Windows paths cannot contain `"`).
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

/// Test double of [`Autostart`].
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
            return Err(io::Error::other("registry unavailable"));
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
            return Err(io::Error::other("registry unavailable"));
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
        assert_eq!(run.writes, 1, "the same command is not rewritten");
    }

    #[test]
    fn reinstall_updates_a_stale_command() {
        let mut run = MemAutostart {
            command: Some("\"C:\\old\\tandemd.exe\"".into()),
            ..Default::default()
        };
        let report = install(&mut run, "new").unwrap();
        assert!(report.autostart_changed);
        assert_eq!(run.command.as_deref(), Some("new"));
    }

    #[test]
    fn uninstall_removes_only_autostart() {
        let mut run = MemAutostart::default();
        install(&mut run, "a").unwrap();
        assert!(uninstall(&mut run).unwrap());
        assert_eq!(run.command, None);
        // Uninstalling twice is not an error.
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
        let program = Path::new(r"C:\Users\Ana Zoë\AppData\Local\Tandem\tandemw.exe");
        assert_eq!(
            command_line(program, &RunArgs::default()),
            r#""C:\Users\Ana Zoë\AppData\Local\Tandem\tandemw.exe""#
        );
        let args = RunArgs {
            config: Some(r"D:\cfg\tandem.toml".into()),
            data_dir: Some(r"D:\data".into()),
            ..RunArgs::default()
        };
        assert_eq!(
            command_line(Path::new("t.exe"), &args),
            r#""t.exe" --config "D:\cfg\tandem.toml" --data-dir "D:\data""#
        );
    }

    proptest! {
        /// Installing n times leaves the same as installing once, and uninstalling removes everything.
        #[test]
        fn install_converges(n in 1usize..5, previous in proptest::option::of("[a-z]{1,8}")) {
            let mut run = MemAutostart { command: previous, ..Default::default() };
            for _ in 0..n {
                install(&mut run, "command").unwrap();
            }
            prop_assert_eq!(run.command.as_deref(), Some("command"));
            prop_assert!(run.writes <= 1);
            prop_assert!(uninstall(&mut run).unwrap());
            prop_assert_eq!(run.command, None);
        }
    }
}
