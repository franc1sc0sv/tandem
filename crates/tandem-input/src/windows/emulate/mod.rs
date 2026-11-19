//! Input injection on Windows with `SendInput`.
//!
//! [`Emulator`] translates each [`InputEvent`] into [`WinInput`] records (a portable mirror
//! of Win32's `INPUT` structure) and hands them to an [`InputSink`]. The translation
//! lives here and is tested on Linux; [`SendInputSink`] (Windows only) is the real sender.
//!
//! Decisions (docs/research/input-engine.md):
//! - Keys by **scancode** (`KEYEVENTF_SCANCODE`, + `EXTENDEDKEY` for the E0 ones): that way the
//!   system applies its own layout and ES/LatAm, AltGr and dead keys work.
//! - **Absolute** mouse over the virtual desktop (`MOVE|ABSOLUTE|VIRTUALDESK`): `SendInput`'s
//!   relative movement goes through pointer acceleration. The position
//!   is kept in `f64`, so deltas of less than a pixel accumulate instead of being lost.
//! - Wheel with `mouseData` in `WHEEL_DELTA` units (120), which admits fractions.
//! - Everything injected carries [`TANDEM_EXTRA_INFO`] in `dwExtraInfo`.

#[cfg(windows)]
mod sys;

#[cfg(windows)]
pub use sys::{SendInputSink, set_per_monitor_v2_dpi_awareness};

use crate::event::{
    Button, HidUsage, InputEvent, KeyState, ModifierKey, Modifiers, Pressed, Scroll, keymap,
};
use crate::geometry::{Point, Rect};
use crate::windows::TANDEM_EXTRA_INFO;

// Constants of winuser.h, repeated here so the translation is portable. A test
// on Windows checks that they match those of the `windows` crate.
pub const KEYEVENTF_EXTENDEDKEY: u32 = 0x0001;
pub const KEYEVENTF_KEYUP: u32 = 0x0002;
pub const KEYEVENTF_SCANCODE: u32 = 0x0008;
pub const MOUSEEVENTF_MOVE: u32 = 0x0001;
pub const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
pub const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
pub const MOUSEEVENTF_RIGHTDOWN: u32 = 0x0008;
pub const MOUSEEVENTF_RIGHTUP: u32 = 0x0010;
pub const MOUSEEVENTF_MIDDLEDOWN: u32 = 0x0020;
pub const MOUSEEVENTF_MIDDLEUP: u32 = 0x0040;
pub const MOUSEEVENTF_XDOWN: u32 = 0x0080;
pub const MOUSEEVENTF_XUP: u32 = 0x0100;
pub const MOUSEEVENTF_WHEEL: u32 = 0x0800;
pub const MOUSEEVENTF_HWHEEL: u32 = 0x1000;
pub const MOUSEEVENTF_VIRTUALDESK: u32 = 0x4000;
pub const MOUSEEVENTF_ABSOLUTE: u32 = 0x8000;
pub const XBUTTON1: u32 = 0x0001;
pub const XBUTTON2: u32 = 0x0002;
/// `VK_PAUSE`: Pause has no simple scancode (it is the sequence E1 1D 45) and the bare 0x45
/// that Chromium's Windows table gives is interpreted as Num Lock.
pub const VK_PAUSE: u16 = 0x13;

/// Range of `SendInput`'s normalized absolute coordinates.
const ABS_SCALE: i64 = 65_536;

/// Trackpad pixels equivalent to one wheel notch (120). It is what Chromium
/// scrolls on Windows per notch at 100 % scale; refine it on real hardware.
pub const PIXELS_PER_NOTCH: f64 = 100.0;
const WHEEL_DELTA: f64 = 120.0;

/// A Win32 `INPUT` record, without depending on the `windows` crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinInput {
    Keyboard {
        vk: u16,
        scan: u16,
        flags: u32,
        extra_info: usize,
    },
    Mouse {
        dx: i32,
        dy: i32,
        /// Wheel delta (signed, as Win32 stores it) or X button.
        data: u32,
        flags: u32,
        extra_info: usize,
    },
}

impl WinInput {
    fn key(scan: u16, flags: u32) -> Self {
        WinInput::Keyboard {
            vk: 0,
            scan,
            flags,
            extra_info: TANDEM_EXTRA_INFO,
        }
    }

    fn mouse(dx: i32, dy: i32, data: u32, flags: u32) -> Self {
        WinInput::Mouse {
            dx,
            dy,
            data,
            flags,
            extra_info: TANDEM_EXTRA_INFO,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EmulateError {
    #[error("key {0:?} has no scancode on Windows")]
    UnmappedKey(HidUsage),
    #[error("Windows does not have button {0}")]
    UnsupportedButton(u8),
    #[error("the desktop has no screens")]
    NoDesktop,
    /// `SendInput` accepted fewer events than requested: typically UIPI (the active
    /// window belongs to a process with higher integrity) or the secure desktop.
    #[error("SendInput injected {sent} of {expected} events (UIPI or secure desktop?)")]
    Blocked { sent: usize, expected: usize },
    #[error("{op} failed: {message} ({code:#010X})")]
    Os {
        op: &'static str,
        code: u32,
        message: String,
    },
}

/// The desktop's screens, in physical pixels (Per-Monitor-V2 process).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Desktop {
    /// Rectangle that encloses all of them (`SM_*VIRTUALSCREEN`): the base of `VIRTUALDESK`.
    pub virtual_rect: Rect,
    /// Each monitor. There may be gaps inside `virtual_rect` where there is no screen.
    pub monitors: Vec<Rect>,
}

impl Desktop {
    /// Takes the point to the nearest pixel that is on some monitor, so that the
    /// internal position does not get lost in a gap of the virtual desktop.
    fn clamp(&self, p: Point) -> Point {
        let mut best: Option<(f64, Point)> = None;
        for m in &self.monitors {
            if m.contains(p) {
                return p;
            }
            let c = m.clamp(p);
            let d = (c.x - p.x).hypot(c.y - p.y);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, c));
            }
        }
        best.map_or_else(|| self.virtual_rect.clamp(p), |(_, c)| c)
    }

    /// Pixel → normalized coordinate 0–65535 of `VIRTUALDESK`. Windows converts back
    /// with `n * width / 65536` truncated; rounding up lands exactly.
    fn normalize(&self, px: i32, py: i32) -> (i32, i32) {
        let r = &self.virtual_rect;
        (
            normalize_axis(px, r.x, r.width),
            normalize_axis(py, r.y, r.height),
        )
    }
}

fn normalize_axis(p: i32, origin: i32, size: u32) -> i32 {
    let size = i64::from(size.max(1));
    let off = (i64::from(p) - i64::from(origin)).clamp(0, size - 1);
    let n = (off * ABS_SCALE + size - 1) / size;
    // `n` is in 0..=65535 by construction; the clamp only documents the range.
    n.clamp(0, ABS_SCALE - 1) as i32
}

/// What the emulator needs from the system. On Windows it is [`SendInputSink`].
pub trait InputSink {
    /// Injects the events in a single call, without interleaving with physical input.
    fn send(&mut self, inputs: &[WinInput]) -> Result<(), EmulateError>;
    fn desktop(&self) -> Result<Desktop, EmulateError>;
    /// Real position of the cursor, to notice if another one (the local mouse) moved it.
    fn cursor_pos(&self) -> Option<(i32, i32)>;
}

/// Injection state: cursor position with fractions, wheel remainders and what
/// was left pressed.
pub struct Emulator<S: InputSink> {
    sink: S,
    desktop: Desktop,
    pos: Point,
    /// Last pixel sent; if the real cursor is no longer there, someone else moved it.
    last_px: Option<(i32, i32)>,
    wheel_rest: (f64, f64),
    pressed: Pressed,
}

impl<S: InputSink> Emulator<S> {
    pub fn new(sink: S) -> Result<Self, EmulateError> {
        let desktop = checked(sink.desktop()?)?;
        let pos = sink
            .cursor_pos()
            .map_or_else(|| center(&desktop.virtual_rect), px_center);
        Ok(Self {
            sink,
            desktop,
            pos,
            last_px: None,
            wheel_rest: (0.0, 0.0),
            pressed: Pressed::new(),
        })
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    /// Internal position of the cursor, with fractions.
    pub fn position(&self) -> Point {
        self.pos
    }

    /// What was injected pressed and has not been released.
    pub fn pressed(&self) -> &Pressed {
        &self.pressed
    }

    /// Reads the screens again; call it with `WM_DISPLAYCHANGE`.
    pub fn refresh_desktop(&mut self) -> Result<(), EmulateError> {
        self.desktop = checked(self.sink.desktop()?)?;
        self.pos = self.desktop.clamp(self.pos);
        self.last_px = None;
        Ok(())
    }

    /// Injects an event. If it fails, the pressed-keys state does not change.
    pub fn emulate(&mut self, event: &InputEvent) -> Result<(), EmulateError> {
        let inputs = match *event {
            InputEvent::Key { usage, state } => vec![key_input(usage, state)?],
            InputEvent::Button { button, state } => vec![button_input(button, state)?],
            InputEvent::MotionRelative { dx, dy } => {
                self.resync();
                let p = Point::new(self.pos.x + dx, self.pos.y + dy);
                return self.move_to(p);
            }
            InputEvent::MotionAbsolute { x, y } => {
                return self.move_to(Point::new(x, y));
            }
            InputEvent::Scroll(scroll) => self.scroll_inputs(scroll),
            InputEvent::Modifiers(target) => return self.sync_modifiers(target),
        };
        self.send(&inputs)?;
        self.pressed.record(event);
        Ok(())
    }

    /// Releases everything that was injected pressed: on leaving the screen, on
    /// disconnecting or on timeout.
    pub fn release_all(&mut self) -> Result<(), EmulateError> {
        let mut pending = self.pressed.clone();
        let events = pending.release_all();
        self.send_presses(&events)?;
        self.pressed = pending;
        Ok(())
    }

    /// Takes the modifiers to the requested state: first releases, then presses.
    fn sync_modifiers(&mut self, target: Modifiers) -> Result<(), EmulateError> {
        let current = self.pressed.modifiers();
        let change = |m: ModifierKey, state| InputEvent::Key {
            usage: m.usage(),
            state,
        };
        let events: Vec<_> = current
            .iter()
            .filter(|m| !target.contains(*m))
            .map(|m| change(m, KeyState::Released))
            .chain(
                target
                    .iter()
                    .filter(|m| !current.contains(*m))
                    .map(|m| change(m, KeyState::Pressed)),
            )
            .collect();
        self.send_presses(&events)?;
        for e in &events {
            self.pressed.record(e);
        }
        Ok(())
    }

    /// Injects in a single call a series of key or button events.
    fn send_presses(&mut self, events: &[InputEvent]) -> Result<(), EmulateError> {
        let inputs = events
            .iter()
            .filter_map(|e| match *e {
                InputEvent::Key { usage, state } => Some(key_input(usage, state)),
                InputEvent::Button { button, state } => Some(button_input(button, state)),
                _ => None,
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.send(&inputs)
    }

    /// If the real cursor is not where we left it, it starts from where it is.
    fn resync(&mut self) {
        if let (Some(real), Some(last)) = (self.sink.cursor_pos(), self.last_px)
            && real != last
        {
            self.pos = px_center(real);
            self.last_px = Some(real);
        }
    }

    fn move_to(&mut self, p: Point) -> Result<(), EmulateError> {
        if !(p.x.is_finite() && p.y.is_finite()) {
            return Ok(());
        }
        self.pos = self.desktop.clamp(p);
        let px = (self.pos.x.floor() as i32, self.pos.y.floor() as i32);
        // Without a pixel change there is nothing to send: the fraction stays accumulated.
        if self.last_px == Some(px) {
            return Ok(());
        }
        let (nx, ny) = self.desktop.normalize(px.0, px.1);
        let flags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
        self.send(&[WinInput::mouse(nx, ny, 0, flags)])?;
        self.last_px = Some(px);
        Ok(())
    }

    fn scroll_inputs(&mut self, scroll: Scroll) -> Vec<WinInput> {
        let (dx, dy) = match scroll {
            Scroll::Wheel { dx_v120, dy_v120 } => (dx_v120, dy_v120),
            Scroll::Pixels { dx, dy } => {
                let factor = WHEEL_DELTA / PIXELS_PER_NOTCH;
                let (rx, ry) = &mut self.wheel_rest;
                (take_whole(rx, dx * factor), take_whole(ry, dy * factor))
            }
        };
        let mut out = Vec::new();
        // Same convention as Windows: positive = forward vertically, right horizontally.
        if dy != 0 {
            out.push(WinInput::mouse(0, 0, dy as u32, MOUSEEVENTF_WHEEL));
        }
        if dx != 0 {
            out.push(WinInput::mouse(0, 0, dx as u32, MOUSEEVENTF_HWHEEL));
        }
        out
    }

    fn send(&mut self, inputs: &[WinInput]) -> Result<(), EmulateError> {
        if inputs.is_empty() {
            return Ok(());
        }
        self.sink.send(inputs)
    }
}

/// Adds `delta` to the remainder and extracts the integer part; the fraction is left for next time.
fn take_whole(rest: &mut f64, delta: f64) -> i32 {
    if !delta.is_finite() {
        return 0;
    }
    *rest += delta;
    // The margin avoids losing a unit to rounding (100 × 1.2 adds up to 119.999…).
    let whole = (*rest + rest.signum() * 1e-9).trunc();
    *rest -= whole;
    whole.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

fn checked(desktop: Desktop) -> Result<Desktop, EmulateError> {
    let empty = |r: &Rect| r.width == 0 || r.height == 0;
    if empty(&desktop.virtual_rect) {
        return Err(EmulateError::NoDesktop);
    }
    Ok(Desktop {
        monitors: desktop.monitors.into_iter().filter(|m| !empty(m)).collect(),
        ..desktop
    })
}

fn center(r: &Rect) -> Point {
    Point::new(
        r.left() + f64::from(r.width) / 2.0,
        r.top() + f64::from(r.height) / 2.0,
    )
}

fn px_center((x, y): (i32, i32)) -> Point {
    Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5)
}

/// Keyboard record for a physical key.
pub fn key_input(usage: HidUsage, state: KeyState) -> Result<WinInput, EmulateError> {
    let up = if state == KeyState::Released {
        KEYEVENTF_KEYUP
    } else {
        0
    };
    if usage == HidUsage::key(0x48) {
        return Ok(WinInput::Keyboard {
            vk: VK_PAUSE,
            scan: 0,
            flags: up,
            extra_info: TANDEM_EXTRA_INFO,
        });
    }
    let sc = keymap::to_win(usage).ok_or(EmulateError::UnmappedKey(usage))?;
    let ext = if sc.is_extended() {
        KEYEVENTF_EXTENDEDKEY
    } else {
        0
    };
    Ok(WinInput::key(
        u16::from(sc.code()),
        KEYEVENTF_SCANCODE | ext | up,
    ))
}

/// Mouse record for a button.
pub fn button_input(button: Button, state: KeyState) -> Result<WinInput, EmulateError> {
    let down = state == KeyState::Pressed;
    let pick = |d: u32, u: u32| if down { d } else { u };
    let (flags, data) = match normalize_button(button)? {
        Button::Left => (pick(MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP), 0),
        Button::Right => (pick(MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP), 0),
        Button::Middle => (pick(MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP), 0),
        Button::Back => (pick(MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP), XBUTTON1),
        Button::Forward => (pick(MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP), XBUTTON2),
        Button::Other(n) => return Err(EmulateError::UnsupportedButton(n)),
    };
    Ok(WinInput::mouse(0, 0, data, flags))
}

/// `Other(n)` with a number that Windows does have → its named button.
fn normalize_button(button: Button) -> Result<Button, EmulateError> {
    Ok(match button {
        Button::Other(0) => Button::Left,
        Button::Other(1) => Button::Right,
        Button::Other(2) => Button::Middle,
        Button::Other(3) => Button::Back,
        Button::Other(4) => Button::Forward,
        Button::Other(n) => return Err(EmulateError::UnsupportedButton(n)),
        b => b,
    })
}

#[cfg(test)]
mod tests;
