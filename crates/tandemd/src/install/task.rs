//! Startup with the highest privileges (A7, T-104): a Windows scheduled task that
//! launches the agent at sign-in with "the highest privileges" (`HighestAvailable`),
//! without asking for confirmation at each start. The tunnels' processes inherit those
//! privileges (D8).
//!
//! Registering an elevated task requires an administrator. If the installer does not have one, the
//! normal startup via the `Run` key remains and the agent runs without elevation ([`AgentInstall::RunKey`]).
//!
//! It is only registered if the executable and its folder are writable only by administrators
//! ([`PathAcl`]); otherwise, any user process would change the `.exe` and run it
//! elevated without UAC. It is registered through the Task Scheduler COM API, with the XML in
//! memory and without intermediate files.
//!
//! The task is defined with an XML and not with the options of `schtasks /Create`, because those
//! leave the Task Scheduler defaults: stopping the agent after three days
//! and not starting it on battery, which are no good for an agent that must always be up.

use std::io;
use std::path::Path;

use super::{Autostart, InstallError, InstallReport, command_line};
use crate::cli::RunArgs;

/// Name of the task, in its own Task Scheduler folder.
pub const TASK_NAME: &str = r"Tandem\tandemd";

/// The Task Scheduler. On Windows it is done by its COM API ([`super::HighestTask`]).
pub trait Scheduler {
    /// The current process has the elevated token (administrator with UAC accepted).
    fn is_elevated(&self) -> bool;
    fn exists(&self) -> io::Result<bool>;
    /// Creates the task or replaces the one there was.
    fn register(&mut self, xml: &str) -> io::Result<()>;
    /// Removes the task. `true` if it existed.
    fn remove(&mut self) -> io::Result<bool>;
}

/// Write permissions of a path. On Windows it reads the ACL ([`super::WinAcl`]).
pub trait PathAcl {
    /// `true` if, besides administrators and the system, nobody (`Users`,
    /// `Authenticated Users`, `Everyone`) can write, delete or change the permissions of
    /// `path`. An error counts as "unknown": it is treated as not secure.
    fn admin_only(&self, path: &Path, rigor: super::acl::Rigor) -> io::Result<bool>;
}

/// Why the elevated task was not registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotElevated {
    /// The process does not have the elevated token.
    NoAdmin,
    /// The executable or its folder can be written by a non-administrator: elevating it
    /// would hand a UAC jump to any user process.
    Writable(String),
    /// Without `DOMAIN\user` the trigger would apply to any user.
    NoUser,
}

/// What was registered to start with the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentInstall {
    /// Scheduled task with the highest privileges.
    Task { replaced: bool },
    /// The `Run` key, with the user's privileges, for the given reason.
    RunKey(InstallReport, NotElevated),
}

/// Registers the agent to start with the session with the highest privileges only if it
/// can (`scheduler.is_elevated()`), the user is known and `launcher` and its folder are only
/// writable by administrators (`acl`); otherwise, in the `Run` key without elevation. With the task
/// registered the `Run` value is removed (a second copy would start); when falling back to `Run` with
/// an administrator an old task pointing to another path is removed.
pub fn install_agent(
    scheduler: &mut dyn Scheduler,
    acl: &dyn PathAcl,
    autostart: &mut dyn Autostart,
    launcher: &Path,
    args: &RunArgs,
    user: Option<&str>,
) -> Result<AgentInstall, InstallError> {
    let elevated = scheduler.is_elevated();
    let user = user.filter(|u| !u.is_empty());
    let reason = match user {
        _ if !elevated => NotElevated::NoAdmin,
        None => NotElevated::NoUser,
        Some(_) => match not_admin_only(acl, launcher) {
            Some(why) => NotElevated::Writable(why),
            None => {
                let user = user.unwrap_or_default();
                let replaced = scheduler.exists().map_err(InstallError::Task)?;
                scheduler
                    .register(&task_xml(launcher, args, user))
                    .map_err(InstallError::Task)?;
                super::uninstall(autostart)?;
                return Ok(AgentInstall::Task { replaced });
            }
        },
    };
    if elevated {
        tracing::warn!(
            ?reason,
            "the elevated task is not registered: the agent starts with the Run key, without elevation"
        );
        scheduler.remove().map_err(InstallError::Task)?;
    }
    let report = super::install(autostart, &command_line(launcher, args))?;
    Ok(AgentInstall::RunKey(report, reason))
}

/// The reason the executable or some folder of its path is not administrators-only,
/// if there is one. It walks up to the root: whoever can delete or rename a
/// parent folder replaces everything hanging from it.
pub fn not_admin_only(acl: &dyn PathAcl, launcher: &Path) -> Option<String> {
    use super::acl::Rigor;
    let paths = launcher.ancestors().filter(|p| !p.as_os_str().is_empty());
    // The executable and its folder, with the full mask (DLL planting); the rest, with the
    // ancestors' one.
    for (i, path) in paths.enumerate() {
        let rigor = if i < 2 { Rigor::Full } else { Rigor::Ancestor };
        match acl.admin_only(path, rigor) {
            Ok(true) => {}
            Ok(false) => {
                return Some(format!(
                    "{} is not administrators-only (writable, owned by someone else or a link)",
                    path.display()
                ));
            }
            Err(e) => return Some(format!("could not read the ACL of {}: {e}", path.display())),
        }
    }
    None
}

/// Removes both startups. Returns `(task, Run key)`: which ones were registered.
/// Removing the task requires an administrator, just like registering it.
pub fn uninstall_agent(
    scheduler: &mut dyn Scheduler,
    autostart: &mut dyn Autostart,
) -> Result<(bool, bool), InstallError> {
    // Both are attempted even if one fails: the elevated task cannot be left pointing to a
    // path that is no longer controlled.
    let task = scheduler.remove().map_err(InstallError::Task);
    let run = super::uninstall(autostart);
    Ok((task?, run?))
}

/// The task definition. `user` (`DOMAIN\user`) is who signs in and under whose
/// account it runs; it is required: without it the trigger would apply to any user.
pub fn task_xml(launcher: &Path, args: &RunArgs, user: &str) -> String {
    let full = command_line(launcher, args);
    // `Command` is only the program and `Arguments` the rest: the line without the program.
    let program = launcher.display().to_string();
    let arguments = full
        .strip_prefix(&format!("\"{program}\""))
        .unwrap_or_default()
        .trim();
    let user_id = format!("<UserId>{}</UserId>", escape(user));
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Tandem agent: Mac and PC as a single desk.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      {user_id}
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Tandem">
      {user_id}
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Tandem">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{arguments}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        command = escape(&program),
        arguments = escape(arguments),
    )
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Test double of [`PathAcl`]: `writable` are the paths any user can write.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct MemAcl {
    pub writable: Vec<std::path::PathBuf>,
    pub fail: bool,
}

#[cfg(test)]
impl PathAcl for MemAcl {
    fn admin_only(&self, path: &Path, _rigor: super::acl::Rigor) -> io::Result<bool> {
        if self.fail {
            return Err(io::Error::other("access denied"));
        }
        Ok(!self.writable.iter().any(|w| w == path))
    }
}

/// Test double of [`Scheduler`].
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct MemScheduler {
    pub elevated: bool,
    pub task: Option<String>,
    pub fail: bool,
}

#[cfg(test)]
impl Scheduler for MemScheduler {
    fn is_elevated(&self) -> bool {
        self.elevated
    }

    fn exists(&self) -> io::Result<bool> {
        Ok(self.task.is_some())
    }

    fn register(&mut self, xml: &str) -> io::Result<()> {
        if self.fail {
            return Err(io::Error::other("Task Scheduler unavailable"));
        }
        self.task = Some(xml.to_owned());
        Ok(())
    }

    fn remove(&mut self) -> io::Result<bool> {
        if self.fail {
            return Err(io::Error::other("access denied"));
        }
        Ok(self.task.take().is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::MemAutostart;

    const LAUNCHER: &str = "C:/Program Files/Tandem/tandemw.exe";
    const DIR: &str = "C:/Program Files/Tandem";

    fn install(
        scheduler: &mut MemScheduler,
        run: &mut MemAutostart,
        args: &RunArgs,
    ) -> Result<AgentInstall, InstallError> {
        install_with(scheduler, &MemAcl::default(), run, args, Some(r"PC\ana"))
    }

    fn install_with(
        scheduler: &mut MemScheduler,
        acl: &MemAcl,
        run: &mut MemAutostart,
        args: &RunArgs,
        user: Option<&str>,
    ) -> Result<AgentInstall, InstallError> {
        install_agent(scheduler, acl, run, Path::new(LAUNCHER), args, user)
    }

    fn elevated() -> MemScheduler {
        MemScheduler {
            elevated: true,
            ..Default::default()
        }
    }

    #[test]
    fn writable_executable_or_folder_do_not_register_the_task_and_use_run() {
        for writable in [LAUNCHER, DIR] {
            let mut task = elevated();
            let acl = MemAcl {
                writable: vec![writable.into()],
                ..Default::default()
            };
            let mut run = MemAutostart::default();
            let got = install_with(
                &mut task,
                &acl,
                &mut run,
                &RunArgs::default(),
                Some("PC\\ana"),
            )
            .unwrap();
            let AgentInstall::RunKey(_, NotElevated::Writable(why)) = got else {
                panic!("the Run key was expected for a writable path: {got:?}");
            };
            assert!(why.contains(writable), "{why}");
            assert_eq!(
                task.task, None,
                "it is not registered with the writable path"
            );
            assert!(run.command.is_some());
        }
    }

    #[test]
    fn a_writable_ancestor_even_the_grandparent_does_not_register_the_task() {
        // The root comes out of `ancestors()` as is: `C:` on Linux, `C:/` on Windows.
        let root = Path::new(LAUNCHER)
            .ancestors()
            .filter(|p| !p.as_os_str().is_empty())
            .last()
            .unwrap()
            .to_path_buf();
        for writable in [Path::new("C:/Program Files").to_path_buf(), root] {
            let writable = writable.display().to_string();
            let mut task = elevated();
            let acl = MemAcl {
                writable: vec![writable.clone().into()],
                ..Default::default()
            };
            let mut run = MemAutostart::default();
            let got = install_with(
                &mut task,
                &acl,
                &mut run,
                &RunArgs::default(),
                Some("PC\\ana"),
            )
            .unwrap();
            let AgentInstall::RunKey(_, NotElevated::Writable(why)) = got else {
                panic!("the Run key was expected for a writable ancestor: {got:?}");
            };
            assert!(why.contains(&writable), "{why}");
            assert_eq!(task.task, None);
        }
    }

    #[test]
    fn failing_to_read_the_acl_counts_as_insecure() {
        let mut task = elevated();
        let acl = MemAcl {
            fail: true,
            ..Default::default()
        };
        let mut run = MemAutostart::default();
        let got = install_with(
            &mut task,
            &acl,
            &mut run,
            &RunArgs::default(),
            Some("PC\\ana"),
        )
        .unwrap();
        assert!(matches!(
            got,
            AgentInstall::RunKey(_, NotElevated::Writable(_))
        ));
        assert_eq!(task.task, None);
    }

    #[test]
    fn without_a_user_the_task_is_not_registered() {
        for user in [None, Some("")] {
            let mut task = elevated();
            let mut run = MemAutostart::default();
            let got = install_with(
                &mut task,
                &MemAcl::default(),
                &mut run,
                &RunArgs::default(),
                user,
            )
            .unwrap();
            assert!(matches!(got, AgentInstall::RunKey(_, NotElevated::NoUser)));
            assert_eq!(task.task, None);
        }
    }

    #[test]
    fn when_falling_back_to_run_with_admin_the_old_task_is_removed() {
        let mut task = MemScheduler {
            task: Some("old".into()),
            ..elevated()
        };
        let acl = MemAcl {
            writable: vec![LAUNCHER.into()],
            ..Default::default()
        };
        let mut run = MemAutostart::default();
        install_with(
            &mut task,
            &acl,
            &mut run,
            &RunArgs::default(),
            Some("PC\\ana"),
        )
        .unwrap();
        assert_eq!(task.task, None);
    }

    #[test]
    fn uninstall_removes_the_key_even_if_the_task_fails() {
        let mut task = MemScheduler {
            fail: true,
            task: Some("x".into()),
            ..elevated()
        };
        let mut run = MemAutostart {
            command: Some("y".into()),
            ..Default::default()
        };
        assert!(uninstall_agent(&mut task, &mut run).is_err());
        assert_eq!(run.command, None);
    }

    #[test]
    fn elevated_registers_the_task_and_removes_the_run_key() {
        let mut task = MemScheduler {
            elevated: true,
            ..Default::default()
        };
        let mut run = MemAutostart {
            command: Some("old".into()),
            ..Default::default()
        };
        let got = install(&mut task, &mut run, &RunArgs::default()).unwrap();
        assert_eq!(got, AgentInstall::Task { replaced: false });
        assert!(task.task.is_some());
        assert_eq!(run.command, None, "it cannot start twice");
        // Reinstalling replaces the task.
        let got = install(&mut task, &mut run, &RunArgs::default()).unwrap();
        assert_eq!(got, AgentInstall::Task { replaced: true });
    }

    #[test]
    fn without_admin_the_run_key_remains_and_the_task_is_untouched() {
        let mut task = MemScheduler::default();
        let mut run = MemAutostart::default();
        let got = install(&mut task, &mut run, &RunArgs::default()).unwrap();
        let AgentInstall::RunKey(report, NotElevated::NoAdmin) = got else {
            panic!("the Run key was expected");
        };
        assert!(report.autostart_changed);
        assert_eq!(
            run.command.as_deref(),
            Some(format!("\"{LAUNCHER}\"").as_str())
        );
        assert_eq!(task.task, None);
    }

    #[test]
    fn a_task_error_is_reported_and_does_not_remove_the_existing_startup() {
        let mut task = MemScheduler {
            elevated: true,
            fail: true,
            ..Default::default()
        };
        let mut run = MemAutostart {
            command: Some("old".into()),
            ..Default::default()
        };
        let err = install(&mut task, &mut run, &RunArgs::default()).unwrap_err();
        assert!(matches!(err, InstallError::Task(_)), "{err}");
        assert_eq!(run.command.as_deref(), Some("old"));
    }

    #[test]
    fn uninstall_removes_both() {
        let mut task = MemScheduler {
            elevated: true,
            task: Some("x".into()),
            ..Default::default()
        };
        let mut run = MemAutostart {
            command: Some("y".into()),
            ..Default::default()
        };
        assert_eq!(uninstall_agent(&mut task, &mut run).unwrap(), (true, true));
        assert_eq!(
            uninstall_agent(&mut task, &mut run).unwrap(),
            (false, false)
        );
        task.fail = true;
        assert!(uninstall_agent(&mut task, &mut run).is_err());
    }

    #[test]
    fn the_xml_asks_for_the_highest_privileges_and_does_not_limit_the_agent() {
        let args = RunArgs {
            config: Some(r"D:\cfg\mi & tandem.toml".into()),
            ..RunArgs::default()
        };
        let xml = task_xml(Path::new(LAUNCHER), &args, r"PC\ana");
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(xml.contains("<LogonTrigger>"));
        assert!(xml.contains(r"<UserId>PC\ana</UserId>"));
        assert!(xml.contains(&format!("<Command>{LAUNCHER}</Command>")));
        // The arguments go apart from the program and escaped.
        assert!(
            xml.contains(
                "<Arguments>--config &quot;D:\\cfg\\mi &amp; tandem.toml&quot;</Arguments>"
            ),
            "{xml}"
        );
        // The user is in the trigger and in the principal.
        assert_eq!(xml.matches("<UserId>").count(), 2);
    }
}
