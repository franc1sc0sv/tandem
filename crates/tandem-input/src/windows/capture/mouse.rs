//! Mouse: hook buttons and wheel, Raw Input deltas.

use super::consts::*;
use super::{MouseHook, RawMouse};
use crate::event::{Button, InputEvent, KeyState, Scroll};

/// Button of a hook message: index (left, right, middle, X1, X2) and whether
/// it is a press.
pub(super) fn button_bit(m: &MouseHook) -> Option<(u8, bool)> {
    let x = || match m.high_word() {
        XBUTTON1 => Some(3),
        XBUTTON2 => Some(4),
        _ => None,
    };
    match m.message {
        WM_LBUTTONDOWN => Some((0, true)),
        WM_LBUTTONUP => Some((0, false)),
        WM_RBUTTONDOWN => Some((1, true)),
        WM_RBUTTONUP => Some((1, false)),
        WM_MBUTTONDOWN => Some((2, true)),
        WM_MBUTTONUP => Some((2, false)),
        WM_XBUTTONDOWN => Some((x()?, true)),
        WM_XBUTTONUP => Some((x()?, false)),
        _ => None,
    }
}

/// Event from a mouse hook message. Movement only comes out as an absolute
/// position and only if it was not swallowed: with input swallowed the cursor does not move and
/// the delta comes from Raw Input.
pub fn mouse_event(m: &MouseHook, swallowed: bool) -> Option<InputEvent> {
    if let Some((index, down)) = button_bit(m) {
        let button = match index {
            0 => Button::Left,
            1 => Button::Right,
            2 => Button::Middle,
            3 => Button::Back,
            _ => Button::Forward,
        };
        let state = if down {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        return Some(InputEvent::Button { button, state });
    }
    // The high word is a signed delta in units of 120 (or fractions,
    // with high-resolution wheels and precision touchpads).
    let delta = i32::from(m.high_word() as i16);
    match m.message {
        WM_MOUSEMOVE if !swallowed => Some(InputEvent::MotionAbsolute {
            x: f64::from(m.x),
            y: f64::from(m.y),
        }),
        WM_MOUSEWHEEL if delta != 0 => Some(InputEvent::Scroll(Scroll::Wheel {
            dx_v120: 0,
            dy_v120: delta,
        })),
        // Positive to the right, as in `Scroll`.
        WM_MOUSEHWHEEL if delta != 0 => Some(InputEvent::Scroll(Scroll::Wheel {
            dx_v120: delta,
            dy_v120: 0,
        })),
        _ => None,
    }
}

/// Raw Input deltas. Devices that report absolute position
/// (remote desktop, virtual machines, tablets) are converted to a delta
/// by subtracting the previous sample of the same device.
#[derive(Debug, Default)]
pub struct RawMotion {
    last_absolute: Option<(usize, i32, i32)>,
}

/// Upper bound of the range of Raw Input's absolute coordinates.
const ABSOLUTE_MAX: f64 = 65535.0;

impl RawMotion {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, raw: &RawMouse) -> Option<(f64, f64)> {
        // What is injected with SendInput arrives without a device; the hook already saw it.
        if raw.device == 0 {
            return None;
        }
        let (dx, dy) = if raw.flags & MOUSE_MOVE_ABSOLUTE != 0 {
            let prev = self
                .last_absolute
                .replace((raw.device, raw.last_x, raw.last_y));
            let (_, px, py) = prev.filter(|(dev, ..)| *dev == raw.device)?;
            (
                f64::from(raw.last_x - px) * raw.span.0 / ABSOLUTE_MAX,
                f64::from(raw.last_y - py) * raw.span.1 / ABSOLUTE_MAX,
            )
        } else {
            self.last_absolute = None;
            (f64::from(raw.last_x), f64::from(raw.last_y))
        };
        (dx != 0.0 || dy != 0.0).then_some((dx, dy))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(message: u32, high: u16) -> MouseHook {
        MouseHook {
            message,
            x: -1920,
            y: 300,
            mouse_data: u32::from(high) << 16,
            flags: 0,
            time: 0,
            extra_info: 0,
        }
    }

    fn button(b: Button, s: KeyState) -> Option<InputEvent> {
        Some(InputEvent::Button {
            button: b,
            state: s,
        })
    }

    #[test]
    fn buttons() {
        use KeyState::*;
        let cases = [
            (WM_LBUTTONDOWN, 0, Button::Left, Pressed),
            (WM_LBUTTONUP, 0, Button::Left, Released),
            (WM_RBUTTONDOWN, 0, Button::Right, Pressed),
            (WM_RBUTTONUP, 0, Button::Right, Released),
            (WM_MBUTTONDOWN, 0, Button::Middle, Pressed),
            (WM_MBUTTONUP, 0, Button::Middle, Released),
            (WM_XBUTTONDOWN, XBUTTON1, Button::Back, Pressed),
            (WM_XBUTTONUP, XBUTTON1, Button::Back, Released),
            (WM_XBUTTONDOWN, XBUTTON2, Button::Forward, Pressed),
            (WM_XBUTTONUP, XBUTTON2, Button::Forward, Released),
        ];
        for (msg, high, b, s) in cases {
            for swallowed in [false, true] {
                assert_eq!(mouse_event(&hook(msg, high), swallowed), button(b, s));
            }
        }
        assert_eq!(mouse_event(&hook(WM_XBUTTONDOWN, 3), false), None);
    }

    #[test]
    fn wheel_with_sign_and_fractions() {
        let wheel = |msg, d: i16| mouse_event(&hook(msg, d as u16), true);
        assert_eq!(
            wheel(WM_MOUSEWHEEL, -120),
            Some(InputEvent::Scroll(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: -120
            }))
        );
        assert_eq!(
            wheel(WM_MOUSEWHEEL, 30),
            Some(InputEvent::Scroll(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: 30
            }))
        );
        assert_eq!(
            wheel(WM_MOUSEHWHEEL, 240),
            Some(InputEvent::Scroll(Scroll::Wheel {
                dx_v120: 240,
                dy_v120: 0
            }))
        );
        assert_eq!(wheel(WM_MOUSEWHEEL, 0), None);
    }

    #[test]
    fn the_position_only_comes_out_without_grab() {
        assert_eq!(
            mouse_event(&hook(WM_MOUSEMOVE, 0), false),
            Some(InputEvent::MotionAbsolute {
                x: -1920.0,
                y: 300.0
            })
        );
        assert_eq!(mouse_event(&hook(WM_MOUSEMOVE, 0), true), None);
        assert_eq!(mouse_event(&hook(0x0000, 0), false), None);
    }

    fn raw(device: usize, flags: u16, x: i32, y: i32) -> RawMouse {
        RawMouse {
            device,
            flags,
            last_x: x,
            last_y: y,
            span: (3440.0, 1440.0),
        }
    }

    #[test]
    fn relative_raw_as_is_and_without_zeros_or_injected() {
        let mut m = RawMotion::new();
        assert_eq!(m.push(&raw(1, 0, 5, -7)), Some((5.0, -7.0)));
        assert_eq!(m.push(&raw(1, 0, 0, 0)), None);
        assert_eq!(m.push(&raw(0, 0, 5, 5)), None);
    }

    #[test]
    fn absolute_raw_is_converted_to_delta_per_device() {
        let mut m = RawMotion::new();
        let abs = MOUSE_MOVE_ABSOLUTE | MOUSE_VIRTUAL_DESKTOP;
        // The first sample only sets the origin.
        assert_eq!(m.push(&raw(1, abs, 0, 0)), None);
        assert_eq!(
            m.push(&raw(1, abs, 65535, 32767)),
            Some((3440.0, 1440.0 * 32767.0 / 65535.0))
        );
        // Another device: start over.
        assert_eq!(m.push(&raw(2, abs, 100, 100)), None);
        // A relative one in between also breaks the series.
        assert_eq!(m.push(&raw(2, 0, 1, 0)), Some((1.0, 0.0)));
        assert_eq!(m.push(&raw(2, abs, 200, 100)), None);
        assert_eq!(m.push(&raw(2, abs, 200, 100)), None);
    }
}
