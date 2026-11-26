//! Who has the foreground and whether it outranks the agent (T-363): the Win32 side of
//! [`ForegroundProbe`]. A process whose token the agent cannot even open counts as elevated:
//! that refusal is what UIPI looks like from a medium-integrity process.

use std::path::Path;

use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use windows::core::PWSTR;

use super::Owned;
use crate::secure::uipi::{Foreground, ForegroundProbe};

#[derive(Debug, Default, Clone, Copy)]
pub struct WinForeground;

impl ForegroundProbe for WinForeground {
    fn foreground(&self) -> Option<Foreground> {
        let pid = foreground_pid()?;
        let Some(process) = open_process(pid) else {
            // Not even limited information: a protected or higher process.
            return Some(Foreground {
                process: format!("process {pid}"),
                elevated: true,
            });
        };
        let name = image_name(&process).unwrap_or_else(|| format!("process {pid}"));
        let theirs = token_elevated(process.0);
        // SAFETY: the pseudo-handle of this process needs no closing and is always valid.
        let ours = token_elevated(unsafe { GetCurrentProcess() }).unwrap_or(false);
        Some(Foreground {
            process: name,
            elevated: theirs.is_none_or(|t| t && !ours),
        })
    }
}

fn foreground_pid() -> Option<u32> {
    // SAFETY: no arguments; a null window means nobody has the foreground.
    let hwnd: HWND = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return None;
    }
    let mut pid = 0u32;
    // SAFETY: `hwnd` came from the system a moment ago; `pid` is a valid out pointer. A window
    // that closed in between gives 0.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (pid != 0).then_some(pid)
}

fn open_process(pid: u32) -> Option<Owned> {
    // SAFETY: plain call; the handle, if any, is owned and closed by `Owned`.
    unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .ok()
        .map(Owned)
}

fn image_name(process: &Owned) -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` u16s and the call writes at most that many; the handle is open.
    unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    }
    .ok()?;
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

/// Whether the process's token is elevated; `None` when the token cannot be read.
fn token_elevated(process: HANDLE) -> Option<bool> {
    let mut token = HANDLE::default();
    // SAFETY: `process` is open (or the current-process pseudo-handle); `token` is a valid out
    // pointer and is owned below.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
    let token = Owned(token);
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: `elevation` is the TOKEN_ELEVATION this class returns, with its size.
    unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            Some((&raw mut elevation).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    }
    .ok()?;
    Some(elevation.TokenIsElevated != 0)
}
