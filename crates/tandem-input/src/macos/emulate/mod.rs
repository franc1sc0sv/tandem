//! Input injection on macOS.
//!
//! [`MacEmulator`] translates each [`InputEvent`] into the CoreGraphics events that
//! macOS expects from a real keyboard and mouse ([`MacEvent`]) and hands them to a
//! [`MacSink`]. All the translation is portable and is tested on Linux with
//! [`RecordingSink`]; the real sink, [`CgSink`], only exists on macOS.
//!
//! What macOS needs and does not come in the portable event:
//! - Modifiers are not keys: they arrive as `kCGEventFlagsChanged`, and every
//!   key and every click carries the current flags (with the side in the device
//!   bits, so that right Option is told apart from the left).
//! - Arrows carry `NumericPad|SecondaryFn`; F keys, Home/End, PageUp/PageDown and
//!   Delete carry `SecondaryFn`; the numeric keypad, `NumericPad`.
//! - Repeat: a key pressed again without releasing it goes out with
//!   `kCGKeyboardEventAutorepeat`.
//! - Mouse: with a button pressed, movement is `*Dragged`; double and triple
//!   clicks are marked with `kCGMouseEventClickState`, counted here.
//! - Scroll: the wheel goes in lines (accumulating v120 fractions) and the trackpad in
//!   pixels, applying the Mac's "natural scrolling".
//! - Media: macOS does not accept them as keycodes; they go as `NSEvent`
//!   `SystemDefined` subtype 8 with the `NX_KEYTYPE_*`.

#[cfg(target_os = "macos")]
mod cg;
mod sink;

use std::time::{Duration, Instant};

use thiserror::Error;

use crate::event::keymap;
use crate::event::{
    Button, Change, HidUsage, InputEvent, KeyState, ModifierKey, Modifiers, PAGE_CONSUMER,
    PAGE_KEYBOARD, Pressed, Scroll,
};
use crate::geometry::{Point, Rect};
use crate::macos::INJECTED_MARKER;

#[cfg(target_os = "macos")]
pub use cg::{CgSink, displays, power_displays};
pub use sink::RecordingSink;

/// Bits of `CGEventFlags`. The ones below (`DEVICE_*`) are the `NX_DEVICE*` that
/// macOS puts on real events to know which side the modifier is on.
pub mod flags {
    pub const ALPHA_SHIFT: u64 = 0x0001_0000;
    pub const SHIFT: u64 = 0x0002_0000;
    pub const CONTROL: u64 = 0x0004_0000;
    pub const ALTERNATE: u64 = 0x0008_0000;
    pub const COMMAND: u64 = 0x0010_0000;
    pub const NUMERIC_PAD: u64 = 0x0020_0000;
    pub const SECONDARY_FN: u64 = 0x0080_0000;
    /// All HID events carry it; some apps expect it.
    pub const NON_COALESCED: u64 = 0x0000_0100;

    pub const DEVICE_LEFT_CONTROL: u64 = 0x0000_0001;
    pub const DEVICE_LEFT_SHIFT: u64 = 0x0000_0002;
    pub const DEVICE_RIGHT_SHIFT: u64 = 0x0000_0004;
    pub const DEVICE_LEFT_COMMAND: u64 = 0x0000_0008;
    pub const DEVICE_RIGHT_COMMAND: u64 = 0x0000_0010;
    pub const DEVICE_LEFT_ALTERNATE: u64 = 0x0000_0020;
    pub const DEVICE_RIGHT_ALTERNATE: u64 = 0x0000_0040;
    pub const DEVICE_RIGHT_CONTROL: u64 = 0x0000_2000;
}

/// CoreGraphics mouse event type (`kCGEvent*Mouse*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Moved,
    LeftDown,
    LeftUp,
    LeftDragged,
    RightDown,
    RightUp,
    RightDragged,
    OtherDown,
    OtherUp,
    OtherDragged,
}

/// Scroll unit (`kCGScrollEventUnit*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollUnit {
    Line,
    Pixel,
}

/// An event ready for `CGEventPost`, without CoreGraphics types.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MacEvent {
    Key {
        keycode: u16,
        down: bool,
        autorepeat: bool,
        flags: u64,
    },
    /// Change of a modifier: `keycode` is the key and `flags` the new state.
    FlagsChanged { keycode: u16, flags: u64 },
    Mouse {
        kind: MouseKind,
        /// Position in global points (origin at the top left of the main screen).
        pos: Point,
        /// `kCGMouseEventButtonNumber`: 0 left, 1 right, 2 middle, …
        button: u32,
        /// `kCGMouseEventClickState`: 1 single, 2 double, 3 triple.
        click_state: i64,
        /// `kCGMouseEventDeltaX/Y`, for games and apps that read the delta.
        delta: (f64, f64),
        flags: u64,
    },
    /// `wheel1` vertical, `wheel2` horizontal, with CoreGraphics's sign.
    Scroll {
        unit: ScrollUnit,
        wheel1: i32,
        wheel2: i32,
        flags: u64,
    },
    /// Media key: `NSEvent` `SystemDefined` subtype 8.
    SystemKey { data1: i64, modifier_flags: u64 },
}

#[derive(Debug, Error)]
pub enum EmulateError {
    #[error("key {0:?} has no keycode on macOS")]
    Unmapped(HidUsage),
    #[error("there are no active screens to place the cursor on")]
    NoScreens,
    #[error("CoreGraphics: {0}")]
    Platform(String),
}

/// What the emulator needs from the system: publish events and read the cursor.
pub trait MacSink {
    /// Posts `event` with `source_user_data` in its `kCGEventSourceUserData`. The emulator
    /// always passes [`INJECTED_MARKER`], so this Mac's own capture recognises what Tandem
    /// injected and does not forward it again (T-241).
    fn post(&mut self, event: &MacEvent, source_user_data: i64) -> Result<(), EmulateError>;

    /// Real position of the cursor, in global points.
    fn cursor(&mut self) -> Result<Point, EmulateError>;

    /// Rectangles of the active screens, in global points.
    fn screens(&mut self) -> Result<Vec<Rect>, EmulateError>;

    /// Double-click window that the user configured on the Mac.
    fn double_click_interval(&mut self) -> Duration;

    /// `true` if the Mac has "natural scrolling" on.
    fn natural_scroll(&mut self) -> bool;
}

/// Maximum distance, in points, between clicks for them to count as a double click.
const CLICK_SLOP: f64 = 4.0;

/// One wheel notch in v120 units.
const V120: i32 = 120;

/// Translates portable events into macOS events. It keeps the state macOS
/// expects to see coherent: pressed keys and buttons, Caps Lock, cursor
/// position, click counter and scroll remainders.
#[derive(Debug)]
pub struct MacEmulator<S: MacSink> {
    sink: S,
    pressed: Pressed,
    caps_lock: bool,
    screens: Vec<Rect>,
    natural_scroll: bool,
    double_click: Duration,
    clicks: ClickCounter,
    wheel_rest: (i32, i32),
    pixel_rest: (f64, f64),
}

impl<S: MacSink> MacEmulator<S> {
    /// Every event leaves tagged as Tandem's: the tap lets it through without echoing it.
    fn post(&mut self, event: MacEvent) -> Result<(), EmulateError> {
        self.sink.post(&event, INJECTED_MARKER)
    }

    /// Reads from the sink the screens, the double click and natural scrolling.
    pub fn new(mut sink: S) -> Result<Self, EmulateError> {
        let screens = sink.screens()?;
        let double_click = sink.double_click_interval();
        let natural_scroll = sink.natural_scroll();
        Ok(Self {
            sink,
            pressed: Pressed::new(),
            caps_lock: false,
            screens,
            natural_scroll,
            double_click,
            clicks: ClickCounter::default(),
            wheel_rest: (0, 0),
            pixel_rest: (0.0, 0.0),
        })
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    /// Rereads the screens and preferences; call it when the screen
    /// configuration changes.
    pub fn refresh(&mut self) -> Result<(), EmulateError> {
        self.screens = self.sink.screens()?;
        self.double_click = self.sink.double_click_interval();
        self.natural_scroll = self.sink.natural_scroll();
        Ok(())
    }

    /// Injects an event.
    pub fn emit(&mut self, event: &InputEvent) -> Result<(), EmulateError> {
        self.emit_at(event, Instant::now())
    }

    /// Releases everything that is pressed (on leaving the screen, on
    /// disconnecting or on timeout), so that nothing is left stuck on the Mac.
    pub fn release_all(&mut self) -> Result<(), EmulateError> {
        let now = Instant::now();
        let mut first_err = None;
        // They are released one at a time, in the order of `Pressed::release_all`, so that
        // each event carries the modifiers that are still down at that moment
        // (releasing C with Cmd still pressed, like a real keyboard).
        for event in self.pressed.clone().release_all() {
            // It tries to release everything even if one fails.
            if let Err(e) = self.emit_at(&event, now) {
                first_err.get_or_insert(e);
            }
        }
        first_err.map_or(Ok(()), Err)
    }

    fn emit_at(&mut self, event: &InputEvent, now: Instant) -> Result<(), EmulateError> {
        match *event {
            InputEvent::Key { usage, state } => {
                // Validate before recording: a key without a keycode must not be left
                // pressed forever.
                if media_key(usage).is_none() && keymap::to_mac(usage).is_none() {
                    return Err(EmulateError::Unmapped(usage));
                }
                let change = self.pressed.record(event);
                self.key(usage, state, change)
            }
            InputEvent::Button { button, state } => {
                // `Other(0)` and `Left` are the same button: a single name so that
                // the pressed set and the drag are not confused.
                let button = normalize_button(button);
                match self.pressed.record(&InputEvent::Button { button, state }) {
                    Change::Changed => self.button(button, state, now),
                    Change::Repeat | Change::Ignored => Ok(()),
                }
            }
            InputEvent::MotionRelative { dx, dy } => {
                let from = self.sink.cursor()?;
                self.motion(Point::new(from.x + dx, from.y + dy), from)
            }
            InputEvent::MotionAbsolute { x, y } => {
                let from = self.sink.cursor()?;
                self.motion(Point::new(x, y), from)
            }
            InputEvent::Scroll(scroll) => self.scroll(scroll),
            InputEvent::Modifiers(target) => self.sync_modifiers(target, now),
        }
    }

    fn key(
        &mut self,
        usage: HidUsage,
        state: KeyState,
        change: Change,
    ) -> Result<(), EmulateError> {
        if change == Change::Ignored {
            return Ok(());
        }
        let down = state == KeyState::Pressed;
        let repeat = change == Change::Repeat;

        if let Some(key) = media_key(usage) {
            return self.post(MacEvent::SystemKey {
                data1: system_key_data1(key, down, repeat),
                modifier_flags: system_key_modifier_flags(down),
            });
        }
        let keycode = keymap::to_mac(usage)
            .ok_or(EmulateError::Unmapped(usage))?
            .0;

        if usage.modifier().is_some() {
            // A held modifier does not repeat on macOS.
            if repeat {
                return Ok(());
            }
            let flags = self.flags();
            return self.post(MacEvent::FlagsChanged { keycode, flags });
        }
        if usage == CAPS_LOCK {
            // Caps Lock toggles on press; releasing it changes nothing.
            if !down || repeat {
                return Ok(());
            }
            self.caps_lock = !self.caps_lock;
            let flags = self.flags();
            return self.post(MacEvent::FlagsChanged { keycode, flags });
        }
        self.post(MacEvent::Key {
            keycode,
            down,
            autorepeat: repeat,
            flags: self.flags() | key_flags(keycode),
        })
    }

    fn button(
        &mut self,
        button: Button,
        state: KeyState,
        now: Instant,
    ) -> Result<(), EmulateError> {
        let pos = self.sink.cursor()?;
        let down = state == KeyState::Pressed;
        let click_state = if down {
            self.clicks.press(button, pos, now, self.double_click)
        } else {
            self.clicks.release(button)
        };
        let number = button_number(button);
        let kind = match (number, down) {
            (0, true) => MouseKind::LeftDown,
            (0, false) => MouseKind::LeftUp,
            (1, true) => MouseKind::RightDown,
            (1, false) => MouseKind::RightUp,
            (_, true) => MouseKind::OtherDown,
            (_, false) => MouseKind::OtherUp,
        };
        self.post(MacEvent::Mouse {
            kind,
            pos,
            button: number,
            click_state,
            delta: (0.0, 0.0),
            flags: self.flags(),
        })
    }

    fn motion(&mut self, to: Point, from: Point) -> Result<(), EmulateError> {
        let to = clamp_to_screens(&self.screens, to).ok_or(EmulateError::NoScreens)?;
        let (kind, button) = self.drag_kind();
        self.post(MacEvent::Mouse {
            kind,
            pos: to,
            button,
            click_state: 0,
            delta: (to.x - from.x, to.y - from.y),
            flags: self.flags(),
        })
    }

    /// With a button pressed the movement is a drag of the lowest button.
    fn drag_kind(&self) -> (MouseKind, u32) {
        let named = [
            Button::Left,
            Button::Right,
            Button::Middle,
            Button::Back,
            Button::Forward,
        ];
        let held = named
            .into_iter()
            .chain((5..=u8::MAX).map(Button::Other))
            .find(|b| self.pressed.is_button_pressed(*b));
        match held.map(button_number) {
            None => (MouseKind::Moved, 0),
            Some(0) => (MouseKind::LeftDragged, 0),
            Some(1) => (MouseKind::RightDragged, 1),
            Some(n) => (MouseKind::OtherDragged, n),
        }
    }

    fn scroll(&mut self, scroll: Scroll) -> Result<(), EmulateError> {
        // Portable convention: y+ = wheel forward, x+ = right. In
        // CoreGraphics without natural scrolling, forward is positive wheel1 and right
        // is negative wheel2; with natural scrolling both are inverted.
        let sign = if self.natural_scroll { -1 } else { 1 };
        let (unit, dx, dy) = match scroll {
            Scroll::Wheel { dx_v120, dy_v120 } => {
                let (rx, x) = split_v120(self.wheel_rest.0.saturating_add(dx_v120));
                let (ry, y) = split_v120(self.wheel_rest.1.saturating_add(dy_v120));
                self.wheel_rest = (rx, ry);
                (ScrollUnit::Line, x, y)
            }
            Scroll::Pixels { dx, dy } => {
                let (rx, x) = split_pixels(self.pixel_rest.0 + dx);
                let (ry, y) = split_pixels(self.pixel_rest.1 + dy);
                self.pixel_rest = (rx, ry);
                (ScrollUnit::Pixel, x, y)
            }
        };
        if dx == 0 && dy == 0 {
            return Ok(());
        }
        self.post(MacEvent::Scroll {
            unit,
            wheel1: dy.saturating_mul(sign),
            wheel2: dx.saturating_mul(-sign),
            flags: self.flags(),
        })
    }

    /// Leaves the pressed modifiers equal to `target`: releases the ones
    /// that are extra and presses the ones that are missing.
    fn sync_modifiers(&mut self, target: Modifiers, now: Instant) -> Result<(), EmulateError> {
        let current = self.pressed.modifiers();
        for key in ModifierKey::ALL {
            let state = match (current.contains(key), target.contains(key)) {
                (true, false) => KeyState::Released,
                (false, true) => KeyState::Pressed,
                _ => continue,
            };
            let event = InputEvent::Key {
                usage: key.usage(),
                state,
            };
            self.emit_at(&event, now)?;
        }
        Ok(())
    }

    /// Current flags: pressed modifiers, with their side, and Caps Lock.
    fn flags(&self) -> u64 {
        modifier_flags(self.pressed.modifiers(), self.caps_lock)
    }
}

const CAPS_LOCK: HidUsage = HidUsage::key(0x39);

/// `CGEventFlags` of a set of modifiers.
pub fn modifier_flags(mods: Modifiers, caps_lock: bool) -> u64 {
    use flags::*;
    let mut f = NON_COALESCED;
    if caps_lock {
        f |= ALPHA_SHIFT;
    }
    for key in mods.iter() {
        f |= match key {
            ModifierKey::ControlLeft => CONTROL | DEVICE_LEFT_CONTROL,
            ModifierKey::ControlRight => CONTROL | DEVICE_RIGHT_CONTROL,
            ModifierKey::ShiftLeft => SHIFT | DEVICE_LEFT_SHIFT,
            ModifierKey::ShiftRight => SHIFT | DEVICE_RIGHT_SHIFT,
            ModifierKey::AltLeft => ALTERNATE | DEVICE_LEFT_ALTERNATE,
            ModifierKey::AltRight => ALTERNATE | DEVICE_RIGHT_ALTERNATE,
            ModifierKey::MetaLeft => COMMAND | DEVICE_LEFT_COMMAND,
            ModifierKey::MetaRight => COMMAND | DEVICE_RIGHT_COMMAND,
        };
    }
    f
}

/// The key's own flags, as a Mac keyboard sets them.
pub fn key_flags(keycode: u16) -> u64 {
    use flags::*;
    match keycode {
        // Arrows: ← → ↓ ↑.
        0x7B..=0x7E => NUMERIC_PAD | SECONDARY_FN,
        // F1–F20, Help/Insert, Home, PageUp, Delete, End, PageDown.
        0x7A | 0x78 | 0x63 | 0x76 | 0x60 | 0x61 | 0x62 | 0x64 | 0x65 | 0x6D | 0x67 | 0x6F
        | 0x69 | 0x6B | 0x71 | 0x6A | 0x40 | 0x4F | 0x50 | 0x5A | 0x72 | 0x73 | 0x74 | 0x75
        | 0x77 | 0x79 => SECONDARY_FN,
        // Numeric keypad.
        0x41 | 0x43 | 0x45 | 0x47 | 0x4B | 0x4C | 0x4E | 0x51 | 0x52..=0x59 | 0x5B | 0x5C => {
            NUMERIC_PAD
        }
        _ => 0,
    }
}

/// `NX_KEYTYPE_*` of a media key, if it is one. It includes the volume of the
/// keyboard page (0x7F–0x81): as a keycode macOS ignores it.
pub fn media_key(usage: HidUsage) -> Option<u32> {
    let key = match (usage.page, usage.id) {
        (PAGE_CONSUMER, 0xE9) | (PAGE_KEYBOARD, 0x80) => 0, // NX_KEYTYPE_SOUND_UP
        (PAGE_CONSUMER, 0xEA) | (PAGE_KEYBOARD, 0x81) => 1, // NX_KEYTYPE_SOUND_DOWN
        (PAGE_CONSUMER, 0x6F) => 2,                         // NX_KEYTYPE_BRIGHTNESS_UP
        (PAGE_CONSUMER, 0x70) => 3,                         // NX_KEYTYPE_BRIGHTNESS_DOWN
        (PAGE_CONSUMER, 0xE2) | (PAGE_KEYBOARD, 0x7F) => 7, // NX_KEYTYPE_MUTE
        (PAGE_CONSUMER, 0xB8) => 14,                        // NX_KEYTYPE_EJECT
        (PAGE_CONSUMER, 0xCD | 0xB0) => 16,                 // NX_KEYTYPE_PLAY
        (PAGE_CONSUMER, 0xB5) => 17,                        // NX_KEYTYPE_NEXT
        (PAGE_CONSUMER, 0xB6) => 18,                        // NX_KEYTYPE_PREVIOUS
        (PAGE_CONSUMER, 0xB3) => 19,                        // NX_KEYTYPE_FAST
        (PAGE_CONSUMER, 0xB4) => 20,                        // NX_KEYTYPE_REWIND
        _ => return None,
    };
    Some(key)
}

/// `data1` of a media key's `NSEvent`: type in the high 16 bits,
/// 0xA (down) or 0xB (up) in the next byte and the repeat in bit 0.
pub fn system_key_data1(key: u32, down: bool, repeat: bool) -> i64 {
    let state: i64 = if down { 0xA } else { 0xB };
    (i64::from(key) << 16) | (state << 8) | i64::from(repeat)
}

/// `modifierFlags` of a media key's `NSEvent`.
pub fn system_key_modifier_flags(down: bool) -> u64 {
    if down { 0xA00 } else { 0xB00 }
}

/// CoreGraphics button number. Back and forward are 3 and 4, as on a
/// USB mouse on the Mac.
pub fn button_number(button: Button) -> u32 {
    match button {
        Button::Left => 0,
        Button::Right => 1,
        Button::Middle => 2,
        Button::Back => 3,
        Button::Forward => 4,
        Button::Other(n) => u32::from(n),
    }
}

/// `Other(n)` with the button's own name, if it has one.
fn normalize_button(button: Button) -> Button {
    match button {
        Button::Other(0) => Button::Left,
        Button::Other(1) => Button::Right,
        Button::Other(2) => Button::Middle,
        Button::Other(3) => Button::Back,
        Button::Other(4) => Button::Forward,
        b => b,
    }
}

/// Takes the point to the nearest active screen, so that the cursor never
/// ends up outside the screens (macOS would leave it somewhere odd).
pub fn clamp_to_screens(screens: &[Rect], p: Point) -> Option<Point> {
    screens
        .iter()
        .map(|r| r.clamp(p))
        .min_by(|a, b| dist2(*a, p).total_cmp(&dist2(*b, p)))
}

fn dist2(a: Point, b: Point) -> f64 {
    (a.x - b.x).powi(2) + (a.y - b.y).powi(2)
}

/// Splits v120 units into whole lines and the remainder, toward zero.
fn split_v120(total: i32) -> (i32, i32) {
    (total % V120, total / V120)
}

/// Splits pixels into an integer and the fractional remainder, toward zero.
fn split_pixels(total: f64) -> (f64, i32) {
    let whole = total.trunc();
    // `as` saturates at the extremes, which is what we want with absurd values.
    (total - whole, whole as i32)
}

/// Counts consecutive clicks of the same button, in the same place and within the
/// double-click window, as macOS does with a real mouse.
#[derive(Debug, Default)]
struct ClickCounter {
    last: Option<LastClick>,
}

#[derive(Debug, Clone, Copy)]
struct LastClick {
    button: Button,
    at: Instant,
    pos: Point,
    count: i64,
}

impl ClickCounter {
    fn press(&mut self, button: Button, pos: Point, now: Instant, window: Duration) -> i64 {
        let count = match self.last {
            Some(l)
                if l.button == button
                    && now.saturating_duration_since(l.at) <= window
                    && dist2(l.pos, pos) <= CLICK_SLOP * CLICK_SLOP =>
            {
                l.count.saturating_add(1)
            }
            _ => 1,
        };
        self.last = Some(LastClick {
            button,
            at: now,
            pos,
            count,
        });
        count
    }

    /// The release event carries the same count as the press.
    fn release(&self, button: Button) -> i64 {
        match self.last {
            Some(l) if l.button == button => l.count,
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests;
