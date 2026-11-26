//! Installing and removing the service: `tandemd --install-service` / `--uninstall-service`
//! (and `--install --with-service`, which the app runs with UAC).
//!
//! What to do is a list of [`Step`]s computed from what is there now ([`Current`]); the
//! Windows side (`windows::manager`) carries each step out against the SCM, the file system
//! and the registry. Computing it apart keeps the decisions (what to stop, copy, create or
//! leave alone, and which policy to touch) tested on Linux.
//!
//! - The service runs its own copy of `tandemd.exe` in `%ProgramFiles%\Tandem Service`
//!   ([`super::SERVICE_DIR`]). The copy is made only from an executable that only
//!   administrators can write: otherwise any process of the user could swap it right before
//!   the copy and become SYSTEM. After copying, the copy itself is checked again.
//! - It runs as LocalSystem, starts with Windows (it has to be up for the sign-in screen)
//!   and the SCM restarts it if it dies (recovery actions: 5 s, 10 s, 60 s; the count resets
//!   after a day).
//! - `SendSAS` from a service only works if the `SoftwareSASGeneration` policy allows
//!   services. Installing turns that bit on if it is off and remembers the value it found;
//!   uninstalling puts that value back.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{SERVICE_DESCRIPTION, SERVICE_DISPLAY_NAME, SERVICE_NAME};
use crate::install::PathAcl;

/// `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System`, value
/// `SoftwareSASGeneration`: 0 nobody, 1 services, 2 Ease of Access apps, 3 both.
pub const SAS_POLICY_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System";
pub const SAS_POLICY_VALUE: &str = "SoftwareSASGeneration";
/// Where install remembers the policy it found (`HKLM`).
pub const STATE_KEY: &str = r"SOFTWARE\Tandem\SecureDesktop";
/// Remembered policy value; [`POLICY_ABSENT`] if there was none.
pub const STATE_PREVIOUS_POLICY: &str = "PreviousSoftwareSASGeneration";
pub const POLICY_ABSENT: u32 = u32::MAX;
const SAS_SERVICES: u32 = 1;

/// How the SCM registers the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSpec {
    pub name: String,
    pub display_name: String,
    pub description: String,
    /// The full command line (`"<exe>" --service`).
    pub command_line: String,
    /// Restart delays after the first, second and later failures.
    pub restart_delays: Vec<Duration>,
    /// Failures counted within this period (`dwResetPeriod`).
    pub reset_after: Duration,
}

impl ServiceSpec {
    pub fn for_exe(exe: &Path) -> Self {
        ServiceSpec {
            name: SERVICE_NAME.into(),
            display_name: SERVICE_DISPLAY_NAME.into(),
            description: SERVICE_DESCRIPTION.into(),
            command_line: format!("\"{}\" --service", exe.display()),
            restart_delays: vec![
                Duration::from_secs(5),
                Duration::from_secs(10),
                Duration::from_secs(60),
            ],
            reset_after: Duration::from_secs(24 * 60 * 60),
        }
    }
}

/// The installed service, as the SCM reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub command_line: String,
    pub running: bool,
}

/// What there is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Current {
    /// The process has the elevated token.
    pub elevated: bool,
    /// `None` if the service is not installed.
    pub service: Option<Installed>,
    /// The `tandemd.exe` that runs this install.
    pub source: PathBuf,
    /// `%ProgramFiles%` as the system reports it (not the environment variable, which the user
    /// can change).
    pub program_files: PathBuf,
    /// `SoftwareSASGeneration` now (`None`: not set).
    pub sas_policy: Option<u32>,
    /// What install remembered (`None`: nothing remembered).
    pub remembered_policy: Option<u32>,
}

impl Current {
    pub fn dir(&self) -> PathBuf {
        self.program_files.join(super::SERVICE_DIR)
    }

    pub fn exe(&self) -> PathBuf {
        self.dir()
            .join(format!("tandemd{}", std::env::consts::EXE_SUFFIX))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Stop,
    CreateDir(PathBuf),
    Copy {
        from: PathBuf,
        to: PathBuf,
    },
    /// Fails if `path` or a parent can be written by a non-administrator.
    CheckAdminOnly(PathBuf),
    Create(ServiceSpec),
    /// The service exists: only its configuration is rewritten.
    Update(ServiceSpec),
    /// Description and recovery actions.
    Configure(ServiceSpec),
    /// Turns on the services bit of `SoftwareSASGeneration` and remembers `previous`.
    SetSasPolicy {
        value: u32,
        previous: Option<u32>,
    },
    /// Puts back the remembered value (`None`: deletes it) and forgets it.
    RestoreSasPolicy(Option<u32>),
    Start,
    Delete,
    RemoveFile(PathBuf),
    /// With everything in it (the service's logs): `remove_dir_all`.
    RemoveDir(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("installing the system service requires an administrator (run it with UAC)")]
    NotElevated,
    #[error(
        "{0}: the service would run as SYSTEM a file that a normal user can change. Install Tandem under Program Files"
    )]
    UnsafeSource(String),
}

/// `SoftwareSASGeneration` with services allowed, if it does not allow them yet.
pub fn sas_policy_with_services(current: Option<u32>) -> Option<u32> {
    let value = current.unwrap_or(0);
    if value & SAS_SERVICES != 0 && value <= 3 {
        return None;
    }
    Some((value & 3) | SAS_SERVICES)
}

/// The steps that leave the service installed, up to date and running.
pub fn install_plan(current: &Current, acl: &dyn PathAcl) -> Result<Vec<Step>, PlanError> {
    if !current.elevated {
        return Err(PlanError::NotElevated);
    }
    let exe = current.exe();
    let copying = current.source != exe;
    if copying && let Some(why) = crate::install::not_admin_only(acl, &current.source) {
        return Err(PlanError::UnsafeSource(why));
    }
    let spec = ServiceSpec::for_exe(&exe);
    let mut steps = Vec::new();
    // The running service holds its .exe: it has to stop before the copy.
    if current.service.as_ref().is_some_and(|s| s.running) {
        steps.push(Step::Stop);
    }
    if copying {
        steps.push(Step::CreateDir(current.dir()));
        steps.push(Step::Copy {
            from: current.source.clone(),
            to: exe.clone(),
        });
    }
    steps.push(Step::CheckAdminOnly(exe));
    steps.push(match current.service {
        None => Step::Create(spec.clone()),
        Some(_) => Step::Update(spec.clone()),
    });
    steps.push(Step::Configure(spec));
    if let Some(value) = sas_policy_with_services(current.sas_policy) {
        // A reinstall keeps the first value remembered, not the one it set itself.
        let previous = current
            .remembered_policy
            .map(|p| (p != POLICY_ABSENT).then_some(p))
            .unwrap_or(current.sas_policy);
        steps.push(Step::SetSasPolicy { value, previous });
    }
    steps.push(Step::Start);
    Ok(steps)
}

/// The steps that remove the service, its copy and the policy change. Without a service
/// installed only the leftovers are cleaned.
pub fn uninstall_plan(current: &Current) -> Result<Vec<Step>, PlanError> {
    if !current.elevated {
        return Err(PlanError::NotElevated);
    }
    let mut steps = Vec::new();
    if let Some(service) = &current.service {
        if service.running {
            steps.push(Step::Stop);
        }
        steps.push(Step::Delete);
    }
    // Only the copy this install makes is removed: a source in that folder is left alone
    // when someone ran the uninstall from there (it cannot delete itself anyway).
    let exe = current.exe();
    if current.source != exe {
        steps.push(Step::RemoveFile(exe));
        steps.push(Step::RemoveDir(current.dir()));
    }
    if let Some(remembered) = current.remembered_policy {
        steps.push(Step::RestoreSasPolicy(
            (remembered != POLICY_ABSENT).then_some(remembered),
        ));
    }
    Ok(steps)
}

/// How many times the service start is tried right after installing it (T-363).
pub const START_ATTEMPTS: u32 = 5;
/// The wait between those tries.
pub const START_RETRY_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Runs `start` until it works or `attempts` run out, waiting with `sleep` in between, but only
/// while the failure is one an antivirus scanning the freshly copied file causes (access
/// denied, or the service not answering in time). Seen on the real PC on 2026-11-26: the first
/// start failed with "Access is denied" and a manual start a minute later worked.
pub fn start_with_retries(
    attempts: u32,
    mut start: impl FnMut() -> std::io::Result<()>,
    mut sleep: impl FnMut(),
) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        tries += 1;
        match start() {
            Ok(()) => return Ok(()),
            Err(e) if tries < attempts && transient_start_error(&e) => {
                tracing::info!(attempt = tries, error = %e, "the service did not start yet; trying again");
                sleep();
            }
            Err(e) => return Err(e),
        }
    }
}

/// `ERROR_ACCESS_DENIED` (5) and `ERROR_SERVICE_REQUEST_TIMEOUT` (1053), or a wait that ran out.
fn transient_start_error(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5 | 1053)) || e.kind() == std::io::ErrorKind::TimedOut
}

#[cfg(test)]
mod tests {
    use std::io;

    use proptest::prelude::*;

    use super::*;
    use crate::install::acl::Rigor;

    struct Acl(bool);
    impl PathAcl for Acl {
        fn admin_only(&self, _: &Path, _: Rigor) -> io::Result<bool> {
            Ok(self.0)
        }
    }

    fn current() -> Current {
        Current {
            elevated: true,
            service: None,
            source: PathBuf::from(r"C:\Program Files\Tandem\tandemd.exe"),
            program_files: PathBuf::from(r"C:\Program Files"),
            sas_policy: None,
            remembered_policy: None,
        }
    }

    fn exe(c: &Current) -> PathBuf {
        c.exe()
    }

    #[test]
    fn fresh_install_copies_creates_configures_sets_the_policy_and_starts() {
        let c = current();
        let steps = install_plan(&c, &Acl(true)).unwrap();
        let spec = ServiceSpec::for_exe(&exe(&c));
        assert_eq!(
            steps,
            [
                Step::CreateDir(c.dir()),
                Step::Copy {
                    from: c.source.clone(),
                    to: exe(&c)
                },
                Step::CheckAdminOnly(exe(&c)),
                Step::Create(spec.clone()),
                Step::Configure(spec),
                Step::SetSasPolicy {
                    value: 1,
                    previous: None
                },
                Step::Start,
            ]
        );
    }

    #[test]
    fn the_service_runs_its_own_copy_with_dash_service() {
        let c = current();
        let spec = ServiceSpec::for_exe(&exe(&c));
        assert!(spec.command_line.ends_with("\" --service"));
        assert!(spec.command_line.contains("Tandem Service"));
        assert_eq!(spec.name, SERVICE_NAME);
        assert_eq!(spec.restart_delays.len(), 3);
        assert!(spec.restart_delays.iter().all(|d| !d.is_zero()));
    }

    #[test]
    fn without_elevation_nothing_is_planned() {
        let c = Current {
            elevated: false,
            ..current()
        };
        assert_eq!(install_plan(&c, &Acl(true)), Err(PlanError::NotElevated));
        assert_eq!(uninstall_plan(&c), Err(PlanError::NotElevated));
    }

    #[test]
    fn a_user_writable_source_is_refused() {
        let c = Current {
            source: PathBuf::from(r"C:\Users\ana\AppData\Local\Tandem\tandemd.exe"),
            ..current()
        };
        let err = install_plan(&c, &Acl(false)).unwrap_err();
        assert!(matches!(err, PlanError::UnsafeSource(_)), "{err}");
    }

    #[test]
    fn reinstall_stops_the_running_service_before_copying_and_updates_it() {
        let c = Current {
            service: Some(Installed {
                command_line: "old".into(),
                running: true,
            }),
            sas_policy: Some(1),
            remembered_policy: Some(POLICY_ABSENT),
            ..current()
        };
        let steps = install_plan(&c, &Acl(true)).unwrap();
        assert_eq!(steps[0], Step::Stop);
        let copy = steps
            .iter()
            .position(|s| matches!(s, Step::Copy { .. }))
            .unwrap();
        assert!(copy > 0);
        assert!(steps.iter().any(|s| matches!(s, Step::Update(_))));
        assert!(!steps.iter().any(|s| matches!(s, Step::Create(_))));
        // The policy already allows services: it is not touched again.
        assert!(!steps.iter().any(|s| matches!(s, Step::SetSasPolicy { .. })));
        assert_eq!(steps.last(), Some(&Step::Start));
    }

    #[test]
    fn running_from_the_service_folder_does_not_copy_onto_itself() {
        let mut c = current();
        c.source = c.exe();
        // No ACL check of a source that is the destination: the copy check covers it.
        let steps = install_plan(&c, &Acl(false)).unwrap();
        assert!(!steps.iter().any(|s| matches!(s, Step::Copy { .. })));
        assert!(steps.contains(&Step::CheckAdminOnly(c.exe())));
    }

    #[test]
    fn sas_policy_only_gains_the_services_bit() {
        assert_eq!(sas_policy_with_services(None), Some(1));
        assert_eq!(sas_policy_with_services(Some(0)), Some(1));
        assert_eq!(sas_policy_with_services(Some(1)), None);
        assert_eq!(sas_policy_with_services(Some(2)), Some(3));
        assert_eq!(sas_policy_with_services(Some(3)), None);
        assert_eq!(sas_policy_with_services(Some(7)), Some(3));
    }

    #[test]
    fn the_first_remembered_policy_survives_a_reinstall() {
        // Install found 2 and set 3; someone reset it to 0; the reinstall still remembers 2.
        let c = Current {
            sas_policy: Some(0),
            remembered_policy: Some(2),
            ..current()
        };
        let steps = install_plan(&c, &Acl(true)).unwrap();
        assert!(steps.contains(&Step::SetSasPolicy {
            value: 1,
            previous: Some(2)
        }));
    }

    #[test]
    fn uninstall_stops_deletes_cleans_and_restores_the_policy() {
        let c = Current {
            service: Some(Installed {
                command_line: "x".into(),
                running: true,
            }),
            remembered_policy: Some(POLICY_ABSENT),
            ..current()
        };
        assert_eq!(
            uninstall_plan(&c).unwrap(),
            [
                Step::Stop,
                Step::Delete,
                Step::RemoveFile(c.exe()),
                Step::RemoveDir(c.dir()),
                Step::RestoreSasPolicy(None),
            ]
        );
        let stopped = Current {
            service: Some(Installed {
                command_line: "x".into(),
                running: false,
            }),
            remembered_policy: Some(2),
            ..current()
        };
        let steps = uninstall_plan(&stopped).unwrap();
        assert_eq!(steps[0], Step::Delete);
        assert_eq!(steps.last(), Some(&Step::RestoreSasPolicy(Some(2))));
    }

    #[test]
    fn uninstall_without_a_service_only_cleans_leftovers() {
        let steps = uninstall_plan(&current()).unwrap();
        assert!(!steps.contains(&Step::Delete));
        assert!(!steps.contains(&Step::Stop));
        assert!(!steps.iter().any(|s| matches!(s, Step::RestoreSasPolicy(_))));
    }

    proptest! {
        /// Whatever there is: install always ends starting the service, never copies without
        /// checking the copy afterwards, and stops before copying if it was running.
        #[test]
        fn install_plans_are_well_ordered(
            installed in proptest::option::of(proptest::bool::ANY),
            policy in proptest::option::of(0u32..5),
            remembered in proptest::option::of(prop_oneof![Just(POLICY_ABSENT), 0u32..4]),
        ) {
            let c = Current {
                service: installed.map(|running| Installed { command_line: "x".into(), running }),
                sas_policy: policy,
                remembered_policy: remembered,
                ..current()
            };
            let steps = install_plan(&c, &Acl(true)).unwrap();
            prop_assert_eq!(steps.last(), Some(&Step::Start));
            let copy = steps.iter().position(|s| matches!(s, Step::Copy { .. }));
            let check = steps.iter().position(|s| matches!(s, Step::CheckAdminOnly(_)));
            prop_assert!(copy < check);
            if installed == Some(true) {
                prop_assert_eq!(&steps[0], &Step::Stop);
            }
            for s in &steps {
                if let Step::SetSasPolicy { value, .. } = s {
                    prop_assert!(*value & 1 == 1 && *value <= 3);
                }
            }
        }
    }

    #[test]
    fn a_start_refused_by_a_scan_is_tried_again() {
        let mut calls = 0;
        let mut sleeps = 0;
        let r = start_with_retries(
            START_ATTEMPTS,
            || {
                calls += 1;
                if calls < 3 {
                    Err(std::io::Error::from_raw_os_error(5))
                } else {
                    Ok(())
                }
            },
            || sleeps += 1,
        );
        assert!(r.is_ok());
        assert_eq!((calls, sleeps), (3, 2));
    }

    #[test]
    fn other_start_errors_and_the_last_attempt_are_reported() {
        let mut calls = 0;
        let r = start_with_retries(
            START_ATTEMPTS,
            || {
                calls += 1;
                Err(std::io::Error::from_raw_os_error(2))
            },
            || {},
        );
        assert_eq!(r.unwrap_err().raw_os_error(), Some(2));
        assert_eq!(calls, 1, "a missing file is not retried");
        let mut calls = 0;
        let r = start_with_retries(
            START_ATTEMPTS,
            || {
                calls += 1;
                Err(std::io::Error::from_raw_os_error(5))
            },
            || {},
        );
        assert!(r.is_err());
        assert_eq!(calls, START_ATTEMPTS);
    }
}
