//! The Win32 side of the window rescue (T-360, [`crate::rescue`]): lists the user's top-level
//! windows and places them with `SetWindowPlacement`, which also moves a minimized window's
//! restore rectangle and re-maximizes a maximized one on the screen its rectangle is on.

use std::ffi::c_void;

use ::windows::Win32::Foundation::{CloseHandle, E_ACCESSDENIED, HWND, LPARAM, RECT};
use ::windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use ::windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetWindow, GetWindowLongPtrW,
    GetWindowPlacement, GetWindowRect, GetWindowThreadProcessId, IsHungAppWindow, IsWindow,
    IsWindowVisible, SW_SHOWMAXIMIZED, SW_SHOWMINIMIZED, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE,
    SetWindowPlacement, WINDOWPLACEMENT, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use ::windows::core::{BOOL, PWSTR};

use super::emulate::rescue_screens;
use crate::geometry::Rect;
use crate::rescue::{
    MoveError, Placement, RescueScreen, ShowState, WindowId, WindowInfo, WindowMover,
};

/// `HRESULT_FROM_WIN32(ERROR_INVALID_WINDOW_HANDLE)`.
const INVALID_WINDOW: i32 = 0x8007_0578_u32 as i32;

#[derive(Debug, Clone, Copy)]
pub struct Win32WindowMover;

fn hwnd(id: WindowId) -> HWND {
    HWND(id as usize as *mut c_void)
}

fn id_of(h: HWND) -> WindowId {
    h.0 as usize as u64
}

fn rect_of(r: RECT) -> Rect {
    Rect::new(
        r.left,
        r.top,
        (r.right - r.left).max(0) as u32,
        (r.bottom - r.top).max(0) as u32,
    )
}

/// `rcNormalPosition` is in workspace coordinates: the origin is the work area's corner of the
/// primary screen (a taskbar on the top or the left shifts it), not the screen's.
fn workspace_offset(screens: &[RescueScreen]) -> (i32, i32) {
    screens
        .iter()
        .find(|s| s.primary)
        .map_or((0, 0), |s| (s.work.x - s.rect.x, s.work.y - s.rect.y))
}

fn is_shell(class: &str) -> bool {
    matches!(
        class,
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    )
}

fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 128];
    // SAFETY: the buffer is a valid, writable slice for the length the call reports.
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Tandem's own windows: this process and the UI's (`tandem*.exe`).
fn is_tandem(h: HWND) -> bool {
    let mut pid = 0u32;
    // SAFETY: `pid` outlives the call.
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    // SAFETY: no arguments.
    if pid == unsafe { GetCurrentProcessId() } {
        return true;
    }
    // SAFETY: a query-only handle, closed below.
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
    else {
        return false;
    };
    let mut buf = [0u16; 520];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` UTF-16 units and the call updates `len` with what it wrote.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    // SAFETY: the handle was opened above and is not used again.
    let _ = unsafe { CloseHandle(process) };
    if ok.is_err() {
        return false;
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]).to_lowercase();
    let exe = path.rsplit(['\\', '/']).next().unwrap_or_default();
    exe.starts_with("tandem")
}

/// A window the user sees as an application window.
fn is_user_window(h: HWND) -> bool {
    // SAFETY: plain queries on a window handle; a stale one just fails.
    unsafe {
        if !IsWindowVisible(h).as_bool() {
            return false;
        }
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        if ex & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) != 0 {
            return false;
        }
        if GetWindow(h, GW_OWNER).is_ok_and(|o| !o.0.is_null()) {
            return false;
        }
        let mut cloaked = 0u32;
        // SAFETY: `cloaked` is a DWORD and the size passed is its size.
        if DwmGetWindowAttribute(
            h,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
        {
            return false;
        }
    }
    !is_shell(&class_name(h)) && !is_tandem(h)
}

fn info(h: HWND, offset: (i32, i32)) -> Result<WindowInfo, MoveError> {
    let mut p = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    let mut r = RECT::default();
    // SAFETY: `p` carries its `length` and `r` is a RECT; both live through the calls.
    unsafe {
        GetWindowPlacement(h, &mut p).map_err(|e| map_error(&e))?;
        GetWindowRect(h, &mut r).map_err(|e| map_error(&e))?;
    }
    let state = match p.showCmd {
        c if c == SW_SHOWMAXIMIZED.0 as u32 => ShowState::Maximized,
        c if c == SW_SHOWMINIMIZED.0 as u32 => ShowState::Minimized,
        _ => ShowState::Normal,
    };
    let mut normal = rect_of(p.rcNormalPosition);
    normal.x += offset.0;
    normal.y += offset.1;
    Ok(WindowInfo {
        id: id_of(h),
        rect: rect_of(r),
        normal,
        state,
    })
}

fn map_error(e: &::windows::core::Error) -> MoveError {
    if e.code() == E_ACCESSDENIED {
        MoveError::Denied
    } else if e.code().0 == INVALID_WINDOW {
        MoveError::Gone
    } else {
        MoveError::Other(e.to_string())
    }
}

impl WindowMover for Win32WindowMover {
    fn screens(&self) -> Result<Vec<RescueScreen>, String> {
        rescue_screens().map_err(|e| e.to_string())
    }

    fn windows(&self) -> Result<Vec<WindowInfo>, String> {
        let offset = workspace_offset(&self.screens()?);
        struct Collect {
            offset: (i32, i32),
            out: Vec<WindowInfo>,
        }
        // SAFETY: the signature matches `WNDENUMPROC`; it only runs inside `EnumWindows`.
        unsafe extern "system" fn each(h: HWND, data: LPARAM) -> BOOL {
            // SAFETY: `data` is the `&mut Collect` below, alive during the enumeration.
            let c = unsafe { &mut *(data.0 as *mut Collect) };
            if is_user_window(h)
                && let Ok(w) = info(h, c.offset)
                && w.rect.width > 0
                && w.rect.height > 0
            {
                c.out.push(w);
            }
            BOOL(1)
        }
        let mut c = Collect {
            offset,
            out: Vec::new(),
        };
        // SAFETY: the callback uses `c` only for the duration of the call.
        unsafe { EnumWindows(Some(each), LPARAM(&mut c as *mut Collect as isize)) }
            .map_err(|e| e.to_string())?;
        Ok(c.out)
    }

    fn place(&self, id: WindowId, to: Placement) -> Result<WindowInfo, MoveError> {
        let h = hwnd(id);
        // SAFETY: a plain query.
        if !unsafe { IsWindow(Some(h)) }.as_bool() {
            return Err(MoveError::Gone);
        }
        // SAFETY: a plain query. SetWindowPlacement waits for the window's thread, so a hung
        // app would block the agent's display loop.
        if unsafe { IsHungAppWindow(h) }.as_bool() {
            return Err(MoveError::Busy);
        }
        let offset = workspace_offset(&self.screens().map_err(MoveError::Other)?);
        let mut p = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        // SAFETY: `p` carries its `length`.
        unsafe { GetWindowPlacement(h, &mut p) }.map_err(|e| map_error(&e))?;
        let n = to.normal;
        p.rcNormalPosition = RECT {
            left: n.x - offset.0,
            top: n.y - offset.1,
            right: n.x - offset.0 + n.width as i32,
            bottom: n.y - offset.1 + n.height as i32,
        };
        p.showCmd = match to.state {
            ShowState::Normal => SW_SHOWNOACTIVATE.0,
            ShowState::Maximized => SW_SHOWMAXIMIZED.0,
            ShowState::Minimized => SW_SHOWMINNOACTIVE.0,
        } as u32;
        // SAFETY: `p` is the placement just read, changed in place.
        unsafe { SetWindowPlacement(h, &p) }.map_err(|e| map_error(&e))?;
        info(h, offset)
    }
}
