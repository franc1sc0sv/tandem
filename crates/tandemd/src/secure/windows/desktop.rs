//! `OpenInputDesktop` / `SetThreadDesktop` for the helper's thread.

use std::io;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS,
    GetUserObjectInformationW, HDESK, OpenInputDesktop, SetThreadDesktop, UOI_NAME,
};

use super::win_err;
use crate::secure::desktop::Desktops;

/// `GENERIC_ALL`: the thread only injects, but `SendInput` on the secure desktop wants full
/// rights on it.
const GENERIC_ALL: DESKTOP_ACCESS_FLAGS = DESKTOP_ACCESS_FLAGS(0x1000_0000);

struct Desk(HDESK);

impl Drop for Desk {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenInputDesktop and is closed only here. Closing the
        // desktop a thread is attached to is refused by Windows, and then it simply stays open.
        let _ = unsafe { CloseDesktop(self.0) };
    }
}

fn open_input(access: DESKTOP_ACCESS_FLAGS) -> io::Result<Desk> {
    // SAFETY: no pointers; the returned handle is owned by `Desk`.
    unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, access) }
        .map(Desk)
        .map_err(win_err)
}

fn name_of(desk: &Desk) -> io::Result<String> {
    let mut buf = [0u16; 128];
    let mut needed = 0u32;
    // SAFETY: `buf` is writable for its byte size, passed as the length.
    unsafe {
        GetUserObjectInformationW(
            HANDLE(desk.0.0),
            UOI_NAME,
            Some(buf.as_mut_ptr().cast()),
            size_of_val(&buf) as u32,
            Some(&mut needed),
        )
    }
    .map_err(win_err)?;
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Ok(String::from_utf16_lossy(&buf[..len]))
}

/// The input desktop of `winsta0`, attached to the calling thread.
#[derive(Default)]
pub struct WinDesktops {
    attached: Option<Desk>,
}

impl Desktops for WinDesktops {
    fn input_name(&mut self) -> io::Result<String> {
        name_of(&open_input(DESKTOP_READOBJECTS)?)
    }

    fn attach_input(&mut self) -> io::Result<String> {
        let desk = open_input(GENERIC_ALL)?;
        // SAFETY: `desk` is a live desktop handle; the helper's thread has no windows or hooks,
        // which is what SetThreadDesktop requires.
        unsafe { SetThreadDesktop(desk.0) }.map_err(win_err)?;
        let name = name_of(&desk)?;
        // The old one can only be closed once the thread left it.
        self.attached = Some(desk);
        Ok(name)
    }
}
