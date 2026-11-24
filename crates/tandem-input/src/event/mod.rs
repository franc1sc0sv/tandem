//! Input event model, platform independent.
//!
//! Keys travel by their physical USB HID code: the target system applies its
//! own layout, so ES/LatAm, dead keys and AltGr/Option work on their own
//! (docs/research/input-engine.md). This module depends on no platform;
//! capture and injection convert from and to these types with [`keymap`].
//!
//! - [`keymap`]: HID ↔ Mac keycode ↔ Windows scancode tables.
//! - [`remap`]: modifier remapping by direction (Cmd→Ctrl, etc.).
//! - [`pressed`]: set of pressed keys and buttons, with `release_all()`.

pub mod keymap;
pub mod pressed;
pub mod remap;

use serde::{Deserialize, Serialize};

pub use keymap::{MacKeycode, WinScancode};
pub use pressed::{Change, Pressed};
pub use remap::{Direction, ModifierRemap, RemapError, RemapPolicy};

/// HID keyboard page (Keyboard/Keypad).
pub const PAGE_KEYBOARD: u16 = 0x07;
/// HID consumer page: media keys.
pub const PAGE_CONSUMER: u16 = 0x0C;
/// HID Generic Desktop page: sleep, wake.
pub const PAGE_GENERIC_DESKTOP: u16 = 0x01;

/// Physical code of a key: USB HID page and usage (equivalent to `KeyboardEvent.code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct HidUsage {
    pub page: u16,
    pub id: u16,
}

impl HidUsage {
    pub const fn new(page: u16, id: u16) -> Self {
        Self { page, id }
    }

    /// Usage on the keyboard page (0x07).
    pub const fn key(id: u16) -> Self {
        Self::new(PAGE_KEYBOARD, id)
    }

    /// Usage on the consumer page (0x0C).
    pub const fn consumer(id: u16) -> Self {
        Self::new(PAGE_CONSUMER, id)
    }

    /// The modifier this key represents, if it is one.
    pub fn modifier(self) -> Option<ModifierKey> {
        ModifierKey::from_usage(self)
    }
}

/// The eight HID modifier keys (0xE0–0xE7), with side.
///
/// `Meta` is Cmd on Mac and the Windows key on PC; `Alt` is Option on Mac.
/// `AltRight` on Windows is AltGr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModifierKey {
    ControlLeft,
    ShiftLeft,
    AltLeft,
    MetaLeft,
    ControlRight,
    ShiftRight,
    AltRight,
    MetaRight,
}

impl ModifierKey {
    /// In the order of the HID report bits.
    pub const ALL: [ModifierKey; 8] = [
        ModifierKey::ControlLeft,
        ModifierKey::ShiftLeft,
        ModifierKey::AltLeft,
        ModifierKey::MetaLeft,
        ModifierKey::ControlRight,
        ModifierKey::ShiftRight,
        ModifierKey::AltRight,
        ModifierKey::MetaRight,
    ];

    /// Position in the HID report's modifier byte.
    pub const fn bit(self) -> u8 {
        self as u8
    }

    pub const fn usage(self) -> HidUsage {
        HidUsage::key(0xE0 + self as u16)
    }

    pub fn from_usage(usage: HidUsage) -> Option<Self> {
        if usage.page != PAGE_KEYBOARD || !(0xE0..=0xE7).contains(&usage.id) {
            return None;
        }
        Self::ALL.get(usize::from(usage.id - 0xE0)).copied()
    }
}

/// Set of pressed modifiers, with the HID byte format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const EMPTY: Modifiers = Modifiers(0);

    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, key: ModifierKey) -> bool {
        self.0 & (1 << key.bit()) != 0
    }

    pub fn insert(&mut self, key: ModifierKey) {
        self.0 |= 1 << key.bit();
    }

    pub fn remove(&mut self, key: ModifierKey) {
        self.0 &= !(1 << key.bit());
    }

    pub fn iter(self) -> impl Iterator<Item = ModifierKey> {
        ModifierKey::ALL
            .into_iter()
            .filter(move |k| self.contains(*k))
    }
}

impl FromIterator<ModifierKey> for Modifiers {
    fn from_iter<I: IntoIterator<Item = ModifierKey>>(iter: I) -> Self {
        let mut m = Modifiers::EMPTY;
        for k in iter {
            m.insert(k);
        }
        m
    }
}

/// State of a key or a button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    Pressed,
    Released,
}

/// Mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Right,
    Middle,
    /// Side "back" button (X1 on Windows, button 3 on Mac).
    Back,
    /// Side "forward" button (X2 on Windows, button 4 on Mac).
    Forward,
    /// Any other, by button number starting at 0 (left).
    Other(u8),
}

/// Scroll. Convention: positive `y` = wheel forward (away from the
/// user, like `WHEEL_DELTA` on Windows); positive `x` = to the right. Each
/// capture undoes its system's "natural scrolling" and each injection reapplies it
/// according to the destination, so that the physical direction is preserved.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Scroll {
    /// Notched wheel, in v120 units (120 = one notch; fractions allowed).
    Wheel { dx_v120: i32, dy_v120: i32 },
    /// Trackpad or continuous wheel, in logical pixels.
    Pixels { dx: f64, dy: f64 },
}

/// An input event, as it travels between the two machines.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    /// Key by physical HID code.
    Key {
        usage: HidUsage,
        state: KeyState,
    },
    Button {
        button: Button,
        state: KeyState,
    },
    /// Relative movement, without the source's acceleration. Capture gives it in
    /// its machine's native units; over the link it travels in logical points
    /// (`session`).
    MotionRelative {
        dx: f64,
        dy: f64,
    },
    /// Absolute position in the machine's native cursor coordinates
    /// (those of its [`Screen`](crate::geometry::Screen)s). It is what the
    /// client injects, so that resynchronization corrects lost datagrams.
    MotionAbsolute {
        x: f64,
        y: f64,
    },
    Scroll(Scroll),
    /// Full modifier state, to synchronize on entering the screen.
    Modifiers(Modifiers),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_occupy_e0_to_e7_in_hid_order() {
        for (i, k) in ModifierKey::ALL.into_iter().enumerate() {
            assert_eq!(k.usage(), HidUsage::key(0xE0 + i as u16));
            assert_eq!(usize::from(k.bit()), i);
            assert_eq!(ModifierKey::from_usage(k.usage()), Some(k));
        }
    }

    #[test]
    fn only_the_keyboard_page_has_modifiers() {
        assert_eq!(HidUsage::consumer(0xE0).modifier(), None);
        assert_eq!(HidUsage::key(0xDF).modifier(), None);
        assert_eq!(HidUsage::key(0xE8).modifier(), None);
        assert_eq!(HidUsage::key(0x04).modifier(), None);
    }

    #[test]
    fn modifiers_insert_remove_and_iterate() {
        let mut m = Modifiers::EMPTY;
        assert!(m.is_empty());
        m.insert(ModifierKey::MetaLeft);
        m.insert(ModifierKey::ShiftRight);
        assert_eq!(m.bits(), 0b0010_1000);
        assert!(m.contains(ModifierKey::MetaLeft));
        assert_eq!(
            m.iter().collect::<Vec<_>>(),
            vec![ModifierKey::MetaLeft, ModifierKey::ShiftRight]
        );
        m.remove(ModifierKey::MetaLeft);
        assert_eq!(m, Modifiers::from_bits(0b0010_0000));
        assert_eq!(
            [ModifierKey::ShiftRight].into_iter().collect::<Modifiers>(),
            m
        );
    }

    #[test]
    fn events_round_trip_through_postcard() {
        let events = [
            InputEvent::Key {
                usage: HidUsage::consumer(0xCD),
                state: KeyState::Pressed,
            },
            InputEvent::Button {
                button: Button::Other(7),
                state: KeyState::Released,
            },
            InputEvent::MotionRelative { dx: 0.25, dy: -3.0 },
            InputEvent::MotionAbsolute { x: 1919.5, y: 0.0 },
            InputEvent::Scroll(Scroll::Wheel {
                dx_v120: -30,
                dy_v120: 120,
            }),
            InputEvent::Scroll(Scroll::Pixels { dx: 0.0, dy: -12.5 }),
            InputEvent::Modifiers(Modifiers::from_bits(0b1000_0001)),
        ];
        for e in events {
            let bytes = postcard::to_stdvec(&e).unwrap();
            assert_eq!(postcard::from_bytes::<InputEvent>(&bytes).unwrap(), e);
        }
    }
}
