//! The Win32 side of the secure desktop: SCM, sessions, desktops, the helper's pipe and the
//! installer's steps. The decisions are in the portable modules next door; here there are
//! only system calls, each `unsafe` block in a small function with its `SAFETY` note.

use std::io;
use std::path::PathBuf;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::GetCurrentProcessId;

mod desktop;
mod host;
mod manager;
mod pipe;
mod scm;

pub use desktop::WinDesktops;
pub use manager::{install_service, service_state, uninstall_service};
pub use pipe::{PipeConnector, run_helper};
pub use scm::run_service;

/// A kernel handle closed on drop.
pub(crate) struct Owned(pub HANDLE);

// SAFETY: a kernel handle is a process-wide value; nothing here relies on thread affinity.
unsafe impl Send for Owned {}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: the handle is owned by this value and closed only here.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn win_err(e: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(e.code().0 & 0xFFFF)
}

/// The session this process runs in.
pub fn current_session() -> io::Result<u32> {
    let mut session = 0u32;
    // SAFETY: `session` is a valid out pointer; the pid is this process's.
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) }.map_err(win_err)?;
    Ok(session)
}

/// Where the service writes its logs: next to its executable, in `%ProgramFiles%`, where only
/// administrators write. `%ProgramData%` would let a user plant a link that SYSTEM then
/// appends to.
pub fn service_log_dir(name: &str) -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| io::Error::other("the executable has no folder"))?;
    Ok(dir.join("logs").join(name))
}

/// Run on the Windows CI: what can be checked without SYSTEM or a real console session.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::secure::routed::{ConnectError, HelperConnector};

    #[test]
    fn the_session_and_the_scm_can_be_read() {
        current_session().unwrap();
        // On CI the service is not installed; on a dev PC it may be. Either way, no error.
        service_state().unwrap();
    }

    /// Without a helper the connector says why; a pipe with the helper's name served by a
    /// process that is not SYSTEM (this test) is never trusted: keystrokes do not go to a
    /// squatter. One test, so the squatter cannot race the first check.
    #[test]
    fn the_connector_explains_and_refuses_squatters() {
        let Err(e) = PipeConnector::new().connect() else {
            // A real helper on this machine: nothing to check here.
            return;
        };
        assert!(
            matches!(e, ConnectError::NotInstalled | ConnectError::NotRunning),
            "{e}"
        );
        let name = crate::secure::helper_pipe_name(current_session().unwrap());
        let Ok(_server) = pipe::squat_for_tests(&name) else {
            // Creating it needs high integrity (the label); without it there is nothing to test.
            return;
        };
        let Err(e) = PipeConnector::new().connect() else {
            panic!("a pipe served by a non-SYSTEM process was accepted");
        };
        assert!(
            matches!(e, ConnectError::Untrusted(_) | ConnectError::NoAccess),
            "{e}"
        );
    }
}
