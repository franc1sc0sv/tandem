//! Keyboard and mouse on the Windows secure desktop (T-027): UAC prompts, the lock and
//! sign-in screens and Ctrl+Alt+Del.
//!
//! The session agent cannot reach those desktops: they belong to `winlogon`, and UIPI keeps
//! a user process away from elevated windows. Three processes share the work:
//!
//! - **The service** (`tandemd --service`, LocalSystem, session 0): it only supervises. It
//!   launches the helper in the session that owns the console, with a copy of its own
//!   LocalSystem token moved to that session and `TokenUIAccess` set, follows session changes (WTS
//!   notifications: fast user switching, sign-out, remote sessions) and restarts the helper
//!   with a backoff if it dies ([`supervisor`]). It is also the one that calls `SendSAS`.
//! - **The helper** (`tandemd --input-helper`, SYSTEM in the console session, on `winsta0`):
//!   it injects with `SendInput` on whatever desktop receives input now, following the
//!   changes with `OpenInputDesktop`/`SetThreadDesktop` ([`desktop`], [`helper`]). It serves
//!   the session agent over a named pipe that only SYSTEM and elevated administrators can open
//!   ([`protocol`]); a Ctrl+Alt+Del request goes up to the service on its stdout.
//! - **The session agent** (`tandemd`): its injector sends each batch of `INPUT` records to
//!   the helper while it is reachable and falls back to its own `SendInput` when it is not
//!   ([`routed`]). It turns Ctrl+Alt+Del into a secure attention request.
//!
//! Installing the service (into Program Files, with recovery actions and the `SendSAS`
//! policy) is a plan of steps tested here ([`install`]); `windows` (only `cfg(windows)`)
//! carries them out and holds every Win32 call.

use std::sync::atomic::{AtomicU8, Ordering};

use tandem_proto::ModuleState;

pub mod desktop;
pub mod helper;
pub mod install;
pub mod protocol;
pub mod routed;
pub mod supervisor;
#[cfg(windows)]
pub mod windows;

/// Name of the service in the SCM.
pub const SERVICE_NAME: &str = "TandemSecureDesktop";
/// What `services.msc` shows.
pub const SERVICE_DISPLAY_NAME: &str = "Tandem secure desktop";
pub const SERVICE_DESCRIPTION: &str = "Lets the Tandem keyboard and mouse work on UAC prompts, the lock screen and the sign-in screen, and sends Ctrl+Alt+Del.";
/// Folder under `%ProgramFiles%` with the service's own copy of `tandemd.exe`. It is apart
/// from the app's folder so an app update never finds the running service's file locked,
/// and so the copy that runs as SYSTEM sits where only administrators write.
pub const SERVICE_DIR: &str = "Tandem Service";

/// The helper's pipe in session `session`. One per session: with fast user switching each
/// session's agent finds only its own helper (and only the console's exists).
pub fn helper_pipe_name(session: u32) -> String {
    format!(r"\\.\pipe\tandem-secure-input-{session}")
}

/// What the session agent knows about the secure desktop path, for the module's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SecureStatus {
    /// Nothing tried yet (no injector created, or not on Windows).
    Unknown = 0,
    /// The service is not installed.
    NotInstalled = 1,
    /// Installed but stopped, or its helper is not up in this session.
    NotRunning = 2,
    /// The helper refuses this agent: the pipe is for elevated administrators only.
    NoAccess = 3,
    /// The helper speaks another protocol version: the service is from another release.
    Outdated = 4,
    /// Injection goes through the helper.
    Connected = 5,
}

impl SecureStatus {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => SecureStatus::NotInstalled,
            2 => SecureStatus::NotRunning,
            3 => SecureStatus::NoAccess,
            4 => SecureStatus::Outdated,
            5 => SecureStatus::Connected,
            _ => SecureStatus::Unknown,
        }
    }
}

/// Last status the injector saw. A process-wide value because the module registry and the
/// injector are created in different places and the registry only reads it.
static STATUS: AtomicU8 = AtomicU8::new(SecureStatus::Unknown as u8);

pub fn report_status(status: SecureStatus) {
    let old = STATUS.swap(status as u8, Ordering::Relaxed);
    if old != status as u8 {
        tracing::info!(?status, "secure desktop");
    }
}

/// The status for the module: what the injector saw, or, before it tried, what the SCM says.
pub fn status() -> SecureStatus {
    match SecureStatus::from_u8(STATUS.load(Ordering::Relaxed)) {
        SecureStatus::Unknown => scm_status(),
        s => s,
    }
}

#[cfg(windows)]
fn scm_status() -> SecureStatus {
    match windows::service_state() {
        Ok(None) => SecureStatus::NotInstalled,
        // Running in the SCM is not enough: only reaching the helper proves the path works.
        Ok(Some(_)) => SecureStatus::NotRunning,
        Err(_) => SecureStatus::Unknown,
    }
}

#[cfg(not(windows))]
fn scm_status() -> SecureStatus {
    SecureStatus::NotInstalled
}

/// The module's state for a status.
pub fn module_state(status: SecureStatus) -> ModuleState {
    let reason = match status {
        SecureStatus::Connected => return ModuleState::Active,
        SecureStatus::Unknown | SecureStatus::NotInstalled => {
            "It needs Tandem's system service: install it from the setup (asks for administrator)"
        }
        SecureStatus::NotRunning => "Tandem's system service is installed but not running",
        SecureStatus::NoAccess => {
            "The agent runs without elevation: install Tandem under Program Files with administrator"
        }
        SecureStatus::Outdated => {
            "Tandem's system service is from another version: install it again from the setup"
        }
    };
    ModuleState::Unavailable {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_names_are_per_session() {
        assert_eq!(helper_pipe_name(1), r"\\.\pipe\tandem-secure-input-1");
        assert_ne!(helper_pipe_name(1), helper_pipe_name(2));
    }

    #[test]
    fn status_round_trips_through_its_byte() {
        for s in [
            SecureStatus::Unknown,
            SecureStatus::NotInstalled,
            SecureStatus::NotRunning,
            SecureStatus::NoAccess,
            SecureStatus::Outdated,
            SecureStatus::Connected,
        ] {
            assert_eq!(SecureStatus::from_u8(s as u8), s);
        }
        assert_eq!(SecureStatus::from_u8(200), SecureStatus::Unknown);
    }

    #[test]
    fn only_a_connected_helper_makes_the_module_active() {
        assert_eq!(module_state(SecureStatus::Connected), ModuleState::Active);
        for s in [
            SecureStatus::Unknown,
            SecureStatus::NotInstalled,
            SecureStatus::NotRunning,
            SecureStatus::NoAccess,
            SecureStatus::Outdated,
        ] {
            let ModuleState::Unavailable { reason } = module_state(s) else {
                panic!("{s:?} cannot be active");
            };
            assert!(!reason.is_empty());
        }
    }
}
