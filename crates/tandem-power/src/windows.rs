//! Clean shutdown on Windows: the same as `shutdown /s /t 0`, through the API.

use windows::Win32::Foundation::{CloseHandle, ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
    SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Shutdown::{
    InitiateShutdownW, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OTHER,
    SHTDN_REASON_MINOR_OTHER, SHUTDOWN_POWEROFF,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PCWSTR;

use crate::{PowerError, Shutdown};

/// Shuts the PC down with `InitiateShutdownW`: no grace period and no forcing (programs that
/// ask to save changes can stop it, as with `shutdown /s /t 0`). Without
/// `SHUTDOWN_HYBRID`: a Fast Startup boot would not be clean (DESIGN §6).
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsShutdown;

impl Shutdown for WindowsShutdown {
    fn power_off(&self) -> Result<(), PowerError> {
        enable_shutdown_privilege()?;
        // SAFETY: no pointers of our own; null machine and message mean "this one" and "none".
        let code = unsafe {
            InitiateShutdownW(
                PCWSTR::null(),
                PCWSTR::null(),
                0,
                SHUTDOWN_POWEROFF,
                SHTDN_REASON_FLAG_PLANNED | SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_MINOR_OTHER,
            )
        };
        if code != 0 {
            return Err(PowerError::Shutdown(format!(
                "InitiateShutdownW: {}",
                windows::core::Error::from(windows::Win32::Foundation::WIN32_ERROR(code))
            )));
        }
        Ok(())
    }
}

/// `SeShutdownPrivilege` exists in the token (SYSTEM and administrators have it) but
/// comes disabled: it has to be enabled before asking for the shutdown.
fn enable_shutdown_privilege() -> Result<(), PowerError> {
    let err = |what: &str, e: windows::core::Error| PowerError::Shutdown(format!("{what}: {e}"));
    let mut token = HANDLE::default();
    // SAFETY: `token` is a writable HANDLE; the process pseudo-handle is not closed.
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
    }
    .map_err(|e| err("OpenProcessToken", e))?;
    let result = adjust(token);
    // SAFETY: `token` was opened above and is not used afterwards.
    let _ = unsafe { CloseHandle(token) };
    result.map_err(|e| err("SeShutdownPrivilege", e))
}

fn adjust(token: HANDLE) -> windows::core::Result<()> {
    let mut luid = LUID::default();
    // SAFETY: `luid` is writable; the name is a system constant.
    unsafe { LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid) }?;
    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    // SAFETY: `token` is valid and has TOKEN_ADJUST_PRIVILEGES; `privileges` lives for the duration of the call.
    unsafe { AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None) }?;
    // Returns success even if the token lacks the privilege: that is only visible in the last error.
    // SAFETY: GetLastError has no preconditions.
    if unsafe { GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
        return Err(windows::core::Error::from(ERROR_NOT_ALL_ASSIGNED));
    }
    Ok(())
}
