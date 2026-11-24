//! Real sending with `SendInput` and reading the desktop.

use ::windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_SUCCESS, LPARAM, POINT, RECT};
use ::windows::Win32::Graphics::Gdi::{
    DISPLAY_DEVICE_ACTIVE, DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors,
    GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use ::windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_BINARY, RegGetValueW};
use ::windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use ::windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, MOUSE_EVENT_FLAGS,
    MOUSEINPUT, SendInput, VIRTUAL_KEY,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    EDD_GET_DEVICE_INTERFACE_NAME, GetCursorPos, GetSystemMetrics, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};
use ::windows::core::{BOOL, PCWSTR};
use tandem_core::DisplayKey;

use super::{Desktop, EmulateError, InputSink, WinInput};
use crate::displays::{Display, windows_edid_key};
use crate::geometry::Rect;

fn os_error(op: &'static str) -> EmulateError {
    let e = ::windows::core::Error::from_thread();
    EmulateError::Os {
        op,
        code: e.code().0 as u32,
        message: e.message(),
    }
}

/// Declares the process Per-Monitor-V2 DPI aware, so that coordinates and metrics are
/// physical pixels on all monitors. If it was already declared (by the manifest or
/// by an earlier call), it is not an error.
pub fn set_per_monitor_v2_dpi_awareness() -> Result<(), EmulateError> {
    // SAFETY: it only changes a process attribute; the context is a system constant.
    match unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } {
        Ok(()) => Ok(()),
        Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => Ok(()),
        Err(e) => Err(EmulateError::Os {
            op: "SetProcessDpiAwarenessContext",
            code: e.code().0 as u32,
            message: e.message(),
        }),
    }
}

/// Real injection. Creating it declares the process Per-Monitor-V2.
pub struct SendInputSink {
    _private: (),
}

impl SendInputSink {
    pub fn new() -> Result<Self, EmulateError> {
        set_per_monitor_v2_dpi_awareness()?;
        Ok(Self { _private: () })
    }
}

fn to_win32(input: &WinInput) -> INPUT {
    match *input {
        WinInput::Keyboard {
            vk,
            scan,
            flags,
            extra_info,
        } => INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: scan,
                    dwFlags: KEYBD_EVENT_FLAGS(flags),
                    time: 0,
                    dwExtraInfo: extra_info,
                },
            },
        },
        WinInput::Mouse {
            dx,
            dy,
            data,
            flags,
            extra_info,
        } => INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data,
                    dwFlags: MOUSE_EVENT_FLAGS(flags),
                    time: 0,
                    dwExtraInfo: extra_info,
                },
            },
        },
    }
}

impl InputSink for SendInputSink {
    fn send(&mut self, inputs: &[WinInput]) -> Result<(), EmulateError> {
        let raw: Vec<INPUT> = inputs.iter().map(to_win32).collect();
        // SAFETY: `raw` lives during the call and the size is that of the INPUT structure.
        let sent = unsafe { SendInput(&raw, size_of::<INPUT>() as i32) } as usize;
        if sent == raw.len() {
            Ok(())
        } else if sent == 0 {
            // UIPI does not set an error of its own: GetLastError is usually left at 0 or ACCESS_DENIED.
            match os_error("SendInput") {
                EmulateError::Os { code: 0, .. } => Err(EmulateError::Blocked {
                    sent,
                    expected: raw.len(),
                }),
                e => Err(e),
            }
        } else {
            Err(EmulateError::Blocked {
                sent,
                expected: raw.len(),
            })
        }
    }

    fn desktop(&self) -> Result<Desktop, EmulateError> {
        // SAFETY: GetSystemMetrics has no preconditions.
        let (x, y, w, h) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        let virtual_rect = Rect::new(x, y, w.max(0) as u32, h.max(0) as u32);
        Ok(Desktop {
            virtual_rect,
            monitors: monitors()?,
        })
    }

    fn cursor_pos(&self) -> Option<(i32, i32)> {
        let mut p = POINT::default();
        // SAFETY: `p` is a valid, writable POINT.
        unsafe { GetCursorPos(&mut p) }.ok()?;
        Some((p.x, p.y))
    }
}

/// Each monitor of the virtual desktop with the adapter output it hangs from (`\\.\DISPLAY1`).
fn enumerate() -> Result<Vec<(Rect, [u16; 32])>, EmulateError> {
    type Out = Vec<(Rect, [u16; 32])>;
    // SAFETY: the signature matches `MONITORENUMPROC`; it is only invoked by `EnumDisplayMonitors`
    // synchronously inside `enumerate`.
    unsafe extern "system" fn collect(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `&mut Out` that `enumerate` passes, alive during the enumeration.
        let out = unsafe { &mut *(data.0 as *mut Out) };
        let mut info = MONITORINFOEXW {
            monitorInfo: MONITORINFO {
                cbSize: size_of::<MONITORINFOEXW>() as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: `info` is a MONITORINFOEXW and its `cbSize` says so, so Windows also fills
        // `szDevice`.
        if unsafe { GetMonitorInfoW(hmon, &mut info.monitorInfo) }.as_bool() {
            let r = info.monitorInfo.rcMonitor;
            out.push((
                Rect::new(
                    r.left,
                    r.top,
                    (r.right - r.left).max(0) as u32,
                    (r.bottom - r.top).max(0) as u32,
                ),
                info.szDevice,
            ));
        }
        BOOL(1)
    }

    let mut out: Out = Vec::new();
    // SAFETY: the callback only uses `out` for the duration of the call.
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut out as *mut Out as isize),
        )
    };
    if !ok.as_bool() {
        return Err(os_error("EnumDisplayMonitors"));
    }
    Ok(out)
}

fn monitors() -> Result<Vec<Rect>, EmulateError> {
    Ok(enumerate()?.into_iter().map(|(r, _)| r).collect())
}

/// Each monitor of the virtual desktop, in the order of [`SendInputSink`]'s desktop, with the
/// EDID identity Windows saved for it (T-340).
pub fn displays() -> Result<Vec<Display>, EmulateError> {
    Ok(enumerate()?
        .into_iter()
        .map(|(r, output)| (r, edid_key(&output)))
        .collect())
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The EDID identity of the active monitor on an adapter output (`szDevice`).
fn edid_key(output: &[u16; 32]) -> Option<DisplayKey> {
    // A mirrored output has several monitors; the first active one is the one on screen.
    for i in 0..8 {
        let mut dd = DISPLAY_DEVICEW {
            cb: size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: `output` is the NUL-terminated `szDevice` Windows filled and `dd` carries its
        // size in `cb`.
        let found = unsafe {
            EnumDisplayDevicesW(
                PCWSTR(output.as_ptr()),
                i,
                &mut dd,
                EDD_GET_DEVICE_INTERFACE_NAME,
            )
        };
        if !found.as_bool() {
            return None;
        }
        if dd.StateFlags.0 & DISPLAY_DEVICE_ACTIVE.0 == 0 {
            continue;
        }
        let len = dd.DeviceID.iter().position(|c| *c == 0).unwrap_or(128);
        let key = windows_edid_key(&String::from_utf16_lossy(&dd.DeviceID[..len]))?;
        return read_edid(&key).and_then(|raw| DisplayKey::from_edid(&raw));
    }
    None
}

/// The `EDID` value under a key of `HKEY_LOCAL_MACHINE`.
fn read_edid(key: &str) -> Option<Vec<u8>> {
    let (key, value) = (utf16(key), utf16("EDID"));
    let mut buf = vec![0u8; 512];
    let mut len = buf.len() as u32;
    // SAFETY: both names are NUL-terminated and live during the call; `buf` has room for `len`
    // bytes and Windows writes no more than that.
    let err = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if err != ERROR_SUCCESS {
        return None;
    }
    buf.truncate(len as usize);
    Some(buf)
}

#[cfg(test)]
mod tests {
    use ::windows::Win32::UI::Input::KeyboardAndMouse as km;

    use super::super::*;
    use super::*;

    #[test]
    fn portable_constants_match_win32() {
        assert_eq!(KEYEVENTF_EXTENDEDKEY, km::KEYEVENTF_EXTENDEDKEY.0);
        assert_eq!(KEYEVENTF_KEYUP, km::KEYEVENTF_KEYUP.0);
        assert_eq!(KEYEVENTF_SCANCODE, km::KEYEVENTF_SCANCODE.0);
        assert_eq!(MOUSEEVENTF_MOVE, km::MOUSEEVENTF_MOVE.0);
        assert_eq!(MOUSEEVENTF_LEFTDOWN, km::MOUSEEVENTF_LEFTDOWN.0);
        assert_eq!(MOUSEEVENTF_LEFTUP, km::MOUSEEVENTF_LEFTUP.0);
        assert_eq!(MOUSEEVENTF_RIGHTDOWN, km::MOUSEEVENTF_RIGHTDOWN.0);
        assert_eq!(MOUSEEVENTF_RIGHTUP, km::MOUSEEVENTF_RIGHTUP.0);
        assert_eq!(MOUSEEVENTF_MIDDLEDOWN, km::MOUSEEVENTF_MIDDLEDOWN.0);
        assert_eq!(MOUSEEVENTF_MIDDLEUP, km::MOUSEEVENTF_MIDDLEUP.0);
        assert_eq!(MOUSEEVENTF_XDOWN, km::MOUSEEVENTF_XDOWN.0);
        assert_eq!(MOUSEEVENTF_XUP, km::MOUSEEVENTF_XUP.0);
        assert_eq!(MOUSEEVENTF_WHEEL, km::MOUSEEVENTF_WHEEL.0);
        assert_eq!(MOUSEEVENTF_HWHEEL, km::MOUSEEVENTF_HWHEEL.0);
        assert_eq!(MOUSEEVENTF_VIRTUALDESK, km::MOUSEEVENTF_VIRTUALDESK.0);
        assert_eq!(MOUSEEVENTF_ABSOLUTE, km::MOUSEEVENTF_ABSOLUTE.0);
        assert_eq!(
            XBUTTON1,
            u32::from(::windows::Win32::UI::WindowsAndMessaging::XBUTTON1)
        );
        assert_eq!(
            XBUTTON2,
            u32::from(::windows::Win32::UI::WindowsAndMessaging::XBUTTON2)
        );
        assert_eq!(VK_PAUSE, km::VK_PAUSE.0);
    }

    /// On the PC: `cargo test -p tandem-input --lib -- --ignored --test-threads=1 sendinput`.
    /// With the cursor far from the edges: moves it in a 100 px square and puts it back
    /// where it was; presses Shift and releases it with `release_all`.
    #[test]
    #[ignore = "hardware"]
    fn sendinput_moves_the_cursor_and_puts_it_back() {
        let sink = SendInputSink::new().unwrap();
        let start = sink.cursor_pos().unwrap();
        let mut emu = Emulator::new(sink).unwrap();
        for (dx, dy) in [(100.0, 0.0), (0.0, 100.0), (-100.0, 0.0), (0.0, -100.0)] {
            // In quarter pixels: checks that fractions accumulate.
            for _ in 0..400 {
                emu.emulate(&InputEvent::MotionRelative {
                    dx: dx / 400.0,
                    dy: dy / 400.0,
                })
                .unwrap();
            }
        }
        assert_eq!(emu.sink().cursor_pos(), Some(start));
        emu.emulate(&InputEvent::Key {
            usage: crate::event::ModifierKey::ShiftLeft.usage(),
            state: KeyState::Pressed,
        })
        .unwrap();
        emu.release_all().unwrap();
        assert!(emu.pressed().is_empty());
    }
}
