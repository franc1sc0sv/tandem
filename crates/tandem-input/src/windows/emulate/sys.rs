//! Envío real con `SendInput` y lectura del escritorio.

use ::windows::Win32::Foundation::{ERROR_ACCESS_DENIED, LPARAM, POINT, RECT};
use ::windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use ::windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use ::windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, MOUSE_EVENT_FLAGS,
    MOUSEINPUT, SendInput, VIRTUAL_KEY,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};
use ::windows::core::BOOL;

use super::{Desktop, EmulateError, InputSink, WinInput};
use crate::geometry::Rect;

fn os_error(op: &'static str) -> EmulateError {
    let e = ::windows::core::Error::from_thread();
    EmulateError::Os {
        op,
        code: e.code().0 as u32,
        message: e.message(),
    }
}

/// Declara el proceso Per-Monitor-V2 DPI aware, para que coordenadas y métricas sean
/// píxeles físicos en todos los monitores. Si ya estaba declarado (por el manifiesto o
/// por una llamada anterior), no es un error.
pub fn set_per_monitor_v2_dpi_awareness() -> Result<(), EmulateError> {
    // SAFETY: solo cambia un atributo del proceso; el contexto es una constante del sistema.
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

/// Inyección real. Crearla declara el proceso Per-Monitor-V2.
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
        // SAFETY: `raw` vive durante la llamada y el tamaño es el de la estructura INPUT.
        let sent = unsafe { SendInput(&raw, size_of::<INPUT>() as i32) } as usize;
        if sent == raw.len() {
            Ok(())
        } else if sent == 0 {
            // UIPI no pone un error propio: GetLastError suele quedar en 0 o ACCESS_DENIED.
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
        // SAFETY: GetSystemMetrics no tiene precondiciones.
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
        // SAFETY: `p` es un POINT válido y escribible.
        unsafe { GetCursorPos(&mut p) }.ok()?;
        Some((p.x, p.y))
    }
}

fn monitors() -> Result<Vec<Rect>, EmulateError> {
    unsafe extern "system" fn collect(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` es el `&mut Vec<Rect>` que pasa `monitors`, vivo durante la enumeración.
        let out = unsafe { &mut *(data.0 as *mut Vec<Rect>) };
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `info` está inicializada con su tamaño.
        if unsafe { GetMonitorInfoW(hmon, &mut info) }.as_bool() {
            let r = info.rcMonitor;
            out.push(Rect::new(
                r.left,
                r.top,
                (r.right - r.left).max(0) as u32,
                (r.bottom - r.top).max(0) as u32,
            ));
        }
        BOOL(1)
    }

    let mut out: Vec<Rect> = Vec::new();
    // SAFETY: el callback solo usa `out` mientras dura la llamada.
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut out as *mut Vec<Rect> as isize),
        )
    };
    if !ok.as_bool() {
        return Err(os_error("EnumDisplayMonitors"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use ::windows::Win32::UI::Input::KeyboardAndMouse as km;

    use super::super::*;
    use super::*;

    #[test]
    fn constantes_portables_coinciden_con_win32() {
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

    /// En la PC: `cargo test -p tandem-input --lib -- --ignored --test-threads=1 sendinput`.
    /// Con el cursor lejos de los bordes: lo mueve en un cuadrado de 100 px y lo devuelve a
    /// donde estaba; presiona Shift y lo suelta con `release_all`.
    #[test]
    #[ignore = "hardware"]
    fn sendinput_mueve_el_cursor_y_lo_devuelve() {
        let sink = SendInputSink::new().unwrap();
        let start = sink.cursor_pos().unwrap();
        let mut emu = Emulator::new(sink).unwrap();
        for (dx, dy) in [(100.0, 0.0), (0.0, 100.0), (-100.0, 0.0), (0.0, -100.0)] {
            // En cuartos de píxel: comprueba que las fracciones se acumulan.
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
