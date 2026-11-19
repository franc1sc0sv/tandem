//! Translation of what the CGEventTap delivers into the portable event model.
//!
//! It touches no Apple API: `sys` reads each `CGEvent` into a [`RawEvent`] and
//! this decides which [`InputEvent`]s come out and whether the event is swallowed. That way the logic
//! is tested on Linux.

use crate::event::keymap::{self, MacKeycode};
use crate::event::{Button, HidUsage, InputEvent, KeyState, ModifierKey, Scroll};
use crate::geometry::Point;

/// Mark that Tandem's injection puts in `kCGEventSourceUserData` of its
/// events ("TNDM"). Capture lets them pass without forwarding them, so as not to echo
/// if both run on the same Mac.
pub const INJECTED_MARKER: i64 = 0x544E_444D;

/// Caps Lock keycode on Mac. macOS sends one `FlagsChanged` per press
/// (on activating and on deactivating), never the release.
const MAC_CAPS_LOCK: u16 = 0x39;

/// Device-dependent bits of `CGEventFlags` (the `NX_DEVICE*KEYMASK`
/// of IOKit): they say which side of each modifier is down.
const fn device_bit(key: ModifierKey) -> u64 {
    match key {
        ModifierKey::ControlLeft => 0x0000_0001,
        ModifierKey::ShiftLeft => 0x0000_0002,
        ModifierKey::ShiftRight => 0x0000_0004,
        ModifierKey::MetaLeft => 0x0000_0008,
        ModifierKey::MetaRight => 0x0000_0010,
        ModifierKey::AltLeft => 0x0000_0020,
        ModifierKey::AltRight => 0x0000_0040,
        ModifierKey::ControlRight => 0x0000_2000,
    }
}

const ALL_DEVICE_BITS: u64 = 0x0000_207F;

/// `NSEventSubtype` of the special keys (volume, brightness, media) that
/// arrive as `NSSystemDefined`.
pub const SUBTYPE_AUX_CONTROL_BUTTONS: i16 = 8;

/// A `CGEvent` read, without Apple pointers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RawEvent {
    KeyDown {
        keycode: u16,
    },
    KeyUp {
        keycode: u16,
    },
    /// A modifier changed. `flags` is the full `CGEventGetFlags`.
    FlagsChanged {
        keycode: u16,
        flags: u64,
    },
    /// `button`: `kCGMouseEventButtonNumber` (0 left, 1 right, 2 middle…).
    MouseDown {
        button: i64,
        location: Point,
    },
    MouseUp {
        button: i64,
        location: Point,
    },
    /// Movement with or without a button (`*Dragged` included). `dx`/`dy`:
    /// `kCGMouseEventDeltaX/Y`, which keep arriving with the cursor disassociated.
    Moved {
        dx: f64,
        dy: f64,
        location: Point,
    },
    /// `axis1` is vertical and `axis2` horizontal, with the sign macOS delivers
    /// (already inverted if "natural scrolling" is on).
    Scroll {
        continuous: bool,
        /// `kCGScrollWheelEventFixedPtDeltaAxis*`: lines, with fraction.
        lines_axis1: f64,
        lines_axis2: f64,
        /// `kCGScrollWheelEventPointDeltaAxis*`: pixels, for the trackpad.
        points_axis1: f64,
        points_axis2: f64,
    },
    /// `NSSystemDefined`: `subtype` and `data1` of the `NSEvent`.
    SystemDefined {
        subtype: i16,
        data1: i64,
    },
    /// The system turned the tap off.
    TapDisabled(TapDisabledBy),
    /// Any other type: not input we care about.
    Other,
}

/// Why macOS turned the tap off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapDisabledBy {
    /// The callback took too long (`kCGEventTapDisabledByTimeout`).
    Timeout,
    /// User input (`kCGEventTapDisabledByUserInput`), e.g. Secure Input.
    UserInput,
}

/// How many times the tap was turned off for each reason, since the capture started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TapDisabledCount {
    pub timeout: u32,
    pub user_input: u32,
}

/// What to do with the original event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapAction {
    /// Return it: it goes on its way on this Mac.
    Pass,
    /// Return NULL: the Mac does not see it (the focus is on the other machine).
    Swallow,
    /// Turn the tap on again and let the event pass.
    Reenable,
}

/// Translates tap events. It keeps the modifiers it saw down, because
/// `FlagsChanged` does not say whether the key went down or up.
///
/// It also remembers what the Mac saw pressed (with local input): if it is
/// released already with input swallowed, the release passes anyway, or it would stay stuck here
/// (T-093; Windows capture does the same with its `HookFilter`).
#[derive(Debug, Clone, Default)]
pub struct Translator {
    modifiers_down: crate::event::Modifiers,
    /// Bit per Mac keycode (0–127) pressed with local input.
    local_keys: u128,
    /// Bit per button number (0–31) pressed with local input.
    local_buttons: u32,
    /// Modifiers pressed with local input.
    local_modifiers: crate::event::Modifiers,
    disabled: TapDisabledCount,
}

/// Marks (`down`) or unmarks a bit; returns whether it was marked.
fn flip<T>(set: &mut T, bit: T, down: bool) -> bool
where
    T: Copy
        + PartialEq
        + Default
        + std::ops::BitAnd<Output = T>
        + std::ops::BitOrAssign
        + std::ops::BitAndAssign
        + std::ops::Not<Output = T>,
{
    let was = *set & bit != T::default();
    if down {
        *set |= bit;
    } else {
        *set &= !bit;
    }
    was
}

impl Translator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Translates `raw` and decides its fate.
    ///
    /// - `grabbed`: the focus is on the other machine; input is swallowed and the absolute
    ///   position does not come out (the local cursor is frozen).
    /// - `natural_scroll`: the macOS preference, to undo it (see [`Scroll`]).
    pub fn translate(
        &mut self,
        raw: &RawEvent,
        grabbed: bool,
        natural_scroll: bool,
        emit: &mut impl FnMut(InputEvent),
    ) -> TapAction {
        // A release of something the Mac saw pressed: it passes even if everything is swallowed.
        let mut local_release = false;
        let key_bit = |keycode: u16| 1u128.checked_shl(u32::from(keycode)).unwrap_or(0);
        let button_bit = |button: i64| {
            u32::try_from(button)
                .ok()
                .and_then(|b| 1u32.checked_shl(b))
                .unwrap_or(0)
        };
        let input = match *raw {
            RawEvent::TapDisabled(by) => {
                let n = match by {
                    TapDisabledBy::Timeout => &mut self.disabled.timeout,
                    TapDisabledBy::UserInput => &mut self.disabled.user_input,
                };
                *n = n.saturating_add(1);
                // If it is turned off, something took too long in the callback or the system asked for it:
                // it shows in the log, and it is turned on again right away.
                tracing::warn!(reason = ?by, times = *n, "macOS turned off the input tap: it is reactivated");
                return TapAction::Reenable;
            }
            RawEvent::Other => return TapAction::Pass,
            RawEvent::KeyDown { keycode } => {
                if !grabbed {
                    flip(&mut self.local_keys, key_bit(keycode), true);
                }
                key(keycode, KeyState::Pressed, emit);
                true
            }
            RawEvent::KeyUp { keycode } => {
                let bit = key_bit(keycode);
                local_release = bit != 0 && flip(&mut self.local_keys, bit, false);
                key(keycode, KeyState::Released, emit);
                true
            }
            RawEvent::FlagsChanged { keycode, flags } => {
                if let Some((m, down)) = self.flags_changed(keycode, flags, emit) {
                    if down && !grabbed {
                        self.local_modifiers.insert(m);
                    } else if !down && self.local_modifiers.contains(m) {
                        self.local_modifiers.remove(m);
                        local_release = true;
                    }
                }
                true
            }
            RawEvent::MouseDown { button, location } => {
                if !grabbed {
                    flip(&mut self.local_buttons, button_bit(button), true);
                }
                if !grabbed {
                    emit(InputEvent::MotionAbsolute {
                        x: location.x,
                        y: location.y,
                    });
                }
                emit(InputEvent::Button {
                    button: mac_button(button),
                    state: KeyState::Pressed,
                });
                true
            }
            RawEvent::MouseUp { button, location } => {
                let bit = button_bit(button);
                local_release = bit != 0 && flip(&mut self.local_buttons, bit, false);
                if !grabbed {
                    emit(InputEvent::MotionAbsolute {
                        x: location.x,
                        y: location.y,
                    });
                }
                emit(InputEvent::Button {
                    button: mac_button(button),
                    state: KeyState::Released,
                });
                true
            }
            RawEvent::Moved { dx, dy, location } => {
                // First the delta and then the real position: the server adds the
                // delta to the previous position, so at the edge (where macOS already
                // left the cursor fixed) the push crosses, and if it does not cross the real
                // position corrects what acceleration may have changed.
                if dx != 0.0 || dy != 0.0 {
                    emit(InputEvent::MotionRelative { dx, dy });
                }
                if !grabbed {
                    emit(InputEvent::MotionAbsolute {
                        x: location.x,
                        y: location.y,
                    });
                }
                true
            }
            RawEvent::Scroll {
                continuous,
                lines_axis1,
                lines_axis2,
                points_axis1,
                points_axis2,
            } => {
                if let Some(s) = scroll(
                    continuous,
                    lines_axis1,
                    lines_axis2,
                    points_axis1,
                    points_axis2,
                    natural_scroll,
                ) {
                    emit(InputEvent::Scroll(s));
                }
                true
            }
            RawEvent::SystemDefined { subtype, data1 } => {
                if subtype != SUBTYPE_AUX_CONTROL_BUTTONS {
                    return TapAction::Pass;
                }
                if let Some((usage, state)) = aux_key(data1) {
                    emit(InputEvent::Key { usage, state });
                }
                true
            }
        };
        if input && grabbed && !local_release {
            TapAction::Swallow
        } else {
            TapAction::Pass
        }
    }

    /// How many times macOS turned off the tap.
    pub fn tap_disabled(&self) -> TapDisabledCount {
        self.disabled
    }

    /// Modifiers the translator believes are down.
    pub fn modifiers_down(&self) -> crate::event::Modifiers {
        self.modifiers_down
    }

    /// Returns the modifier that changed and whether it ended up down.
    fn flags_changed(
        &mut self,
        keycode: u16,
        flags: u64,
        emit: &mut impl FnMut(InputEvent),
    ) -> Option<(ModifierKey, bool)> {
        if keycode == MAC_CAPS_LOCK {
            // Every Caps Lock FlagsChanged is a full press: the other
            // side toggles its own lock.
            let usage = HidUsage::key(0x39);
            emit(InputEvent::Key {
                usage,
                state: KeyState::Pressed,
            });
            emit(InputEvent::Key {
                usage,
                state: KeyState::Released,
            });
            return None;
        }
        // Fn (0x3F) and any other without a standard HID usage do not travel.
        let usage = keymap::from_mac(MacKeycode(keycode))?;
        let modifier = usage.modifier()?;
        let was_down = self.modifiers_down.contains(modifier);
        // Synthetic events sometimes lack the per-side bits: then every
        // FlagsChanged of that key toggles its state.
        let down = if flags & ALL_DEVICE_BITS != 0 {
            flags & device_bit(modifier) != 0
        } else {
            !was_down
        };
        if down == was_down {
            return None;
        }
        if down {
            self.modifiers_down.insert(modifier);
        } else {
            self.modifiers_down.remove(modifier);
        }
        emit(InputEvent::Key {
            usage,
            state: if down {
                KeyState::Pressed
            } else {
                KeyState::Released
            },
        });
        Some((modifier, down))
    }
}

fn key(keycode: u16, state: KeyState, emit: &mut impl FnMut(InputEvent)) {
    // Automatic repeat is also forwarded as another `Pressed`: the destination
    // does not repeat an injected key on its own. Modifiers only count through
    // FlagsChanged, which is what macOS sends; a KeyDown of one is ignored so as not to
    // desynchronize the state.
    if let Some(usage) = keymap::from_mac(MacKeycode(keycode))
        && usage.modifier().is_none()
    {
        emit(InputEvent::Key { usage, state });
    }
}

/// Button by `kCGMouseEventButtonNumber`.
pub fn mac_button(number: i64) -> Button {
    match number {
        0 => Button::Left,
        1 => Button::Right,
        2 => Button::Middle,
        3 => Button::Back,
        4 => Button::Forward,
        n => Button::Other(u8::try_from(n).unwrap_or(u8::MAX)),
    }
}

/// Converts macOS scroll to the portable convention: positive `y` = wheel
/// forward, positive `x` = right, without "natural scrolling".
///
/// On macOS without natural scrolling, the wheel forward gives positive `axis1` and the
/// tilt to the left positive `axis2`; with natural scrolling both arrive
/// inverted.
pub fn scroll(
    continuous: bool,
    lines_axis1: f64,
    lines_axis2: f64,
    points_axis1: f64,
    points_axis2: f64,
    natural: bool,
) -> Option<Scroll> {
    let sign = if natural { -1.0 } else { 1.0 };
    if continuous {
        let dy = points_axis1 * sign;
        let dx = -points_axis2 * sign;
        if dx == 0.0 && dy == 0.0 {
            return None;
        }
        // `+ 0.0` avoids a -0.0 that adds nothing.
        Some(Scroll::Pixels {
            dx: dx + 0.0,
            dy: dy + 0.0,
        })
    } else {
        let dy_v120 = to_v120(lines_axis1 * sign);
        let dx_v120 = to_v120(-lines_axis2 * sign);
        if dx_v120 == 0 && dy_v120 == 0 {
            return None;
        }
        Some(Scroll::Wheel { dx_v120, dy_v120 })
    }
}

fn to_v120(lines: f64) -> i32 {
    let v = (lines * 120.0).round();
    if v.is_nan() {
        0
    } else {
        // `as` saturates at the extremes, which is what we want.
        v as i32
    }
}

/// Special keys of `NSSystemDefined` subtype 8: `data1` carries the
/// `NX_KEYTYPE_*` in bits 16–31 and the state in bits 8–15 (0x0A down, 0x0B
/// up). Returns the equivalent HID usage.
pub fn aux_key(data1: i64) -> Option<(HidUsage, KeyState)> {
    let code = (data1 >> 16) & 0xFFFF;
    let state = match (data1 >> 8) & 0xFF {
        0x0A => KeyState::Pressed,
        0x0B => KeyState::Released,
        _ => return None,
    };
    let usage = match code {
        0 => HidUsage::consumer(0xE9),  // NX_KEYTYPE_SOUND_UP
        1 => HidUsage::consumer(0xEA),  // NX_KEYTYPE_SOUND_DOWN
        2 => HidUsage::consumer(0x6F),  // NX_KEYTYPE_BRIGHTNESS_UP
        3 => HidUsage::consumer(0x70),  // NX_KEYTYPE_BRIGHTNESS_DOWN
        7 => HidUsage::consumer(0xE2),  // NX_KEYTYPE_MUTE
        16 => HidUsage::consumer(0xCD), // NX_KEYTYPE_PLAY
        17 => HidUsage::consumer(0xB5), // NX_KEYTYPE_NEXT
        18 => HidUsage::consumer(0xB6), // NX_KEYTYPE_PREVIOUS
        19 => HidUsage::consumer(0xB3), // NX_KEYTYPE_FAST
        20 => HidUsage::consumer(0xB4), // NX_KEYTYPE_REWIND
        _ => return None,
    };
    Some((usage, state))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::event::Modifiers;

    const ORIGIN: Point = Point::new(0.0, 0.0);

    fn run(t: &mut Translator, raw: RawEvent, grabbed: bool) -> (TapAction, Vec<InputEvent>) {
        let mut out = Vec::new();
        let action = t.translate(&raw, grabbed, false, &mut |e| out.push(e));
        (action, out)
    }

    fn key_ev(id: u16, state: KeyState) -> InputEvent {
        InputEvent::Key {
            usage: HidUsage::key(id),
            state,
        }
    }

    #[test]
    fn keys_by_mac_keycode() {
        let mut t = Translator::new();
        // kVK_ANSI_A = 0x00 → HID 0x04.
        let (a, ev) = run(&mut t, RawEvent::KeyDown { keycode: 0x00 }, false);
        assert_eq!(a, TapAction::Pass);
        assert_eq!(ev, vec![key_ev(0x04, KeyState::Pressed)]);
        let (_, ev) = run(&mut t, RawEvent::KeyUp { keycode: 0x00 }, false);
        assert_eq!(ev, vec![key_ev(0x04, KeyState::Released)]);
        // kVK_Return = 0x24 → HID 0x28.
        let (_, ev) = run(&mut t, RawEvent::KeyDown { keycode: 0x24 }, false);
        assert_eq!(ev, vec![key_ev(0x28, KeyState::Pressed)]);
    }

    #[test]
    fn all_the_keys_of_the_mac_table_are_captured() {
        // Every keycode with a direct HID usage comes out with that usage (except
        // modifiers, which arrive as FlagsChanged).
        let mut t = Translator::new();
        for (usage, mac) in keymap::mac_usages() {
            if keymap::from_mac(mac) != Some(usage) || usage.modifier().is_some() {
                continue;
            }
            let (_, ev) = run(&mut t, RawEvent::KeyDown { keycode: mac.0 }, true);
            assert_eq!(
                ev,
                vec![InputEvent::Key {
                    usage,
                    state: KeyState::Pressed
                }]
            );
        }
    }

    #[test]
    fn unknown_keycode_emits_nothing_but_is_swallowed_with_grab() {
        let mut t = Translator::new();
        let (a, ev) = run(&mut t, RawEvent::KeyDown { keycode: 0x7FFF }, true);
        assert_eq!(a, TapAction::Swallow);
        assert!(ev.is_empty());
    }

    #[test]
    fn with_grab_input_is_swallowed_and_without_grab_it_passes() {
        let inputs = [
            RawEvent::KeyDown { keycode: 0 },
            RawEvent::KeyUp { keycode: 0 },
            RawEvent::FlagsChanged {
                keycode: 0x38,
                flags: 0x0002_0002,
            },
            RawEvent::MouseDown {
                button: 0,
                location: ORIGIN,
            },
            RawEvent::MouseUp {
                button: 0,
                location: ORIGIN,
            },
            RawEvent::Moved {
                dx: 1.0,
                dy: 0.0,
                location: ORIGIN,
            },
            RawEvent::Scroll {
                continuous: false,
                lines_axis1: 1.0,
                lines_axis2: 0.0,
                points_axis1: 10.0,
                points_axis2: 0.0,
            },
            RawEvent::SystemDefined {
                subtype: SUBTYPE_AUX_CONTROL_BUTTONS,
                data1: 0x0000_0A00,
            },
        ];
        for raw in inputs {
            assert_eq!(run(&mut Translator::new(), raw, true).0, TapAction::Swallow);
            assert_eq!(run(&mut Translator::new(), raw, false).0, TapAction::Pass);
        }
    }

    #[test]
    fn other_events_always_pass() {
        for grabbed in [false, true] {
            let mut t = Translator::new();
            assert_eq!(
                run(&mut t, RawEvent::Other, grabbed),
                (TapAction::Pass, vec![])
            );
            // Another NSSystemDefined subtype (e.g. 7, old auxiliary buttons).
            let raw = RawEvent::SystemDefined {
                subtype: 7,
                data1: 0,
            };
            assert_eq!(run(&mut t, raw, grabbed), (TapAction::Pass, vec![]));
        }
    }

    /// T-093: a key, a button or a modifier pressed with local input and
    /// released already with input swallowed (crossed to the other machine with something held)
    /// has to reach the Mac; otherwise it stays stuck here.
    #[test]
    fn releasing_what_was_pressed_locally_passes_even_with_grab() {
        let mut t = Translator::new();
        // `a` (0x00), left click and left Shift with local input.
        let at = Point::new(10.0, 10.0);
        run(&mut t, RawEvent::KeyDown { keycode: 0x00 }, false);
        run(
            &mut t,
            RawEvent::MouseDown {
                button: 0,
                location: at,
            },
            false,
        );
        let shift = RawEvent::FlagsChanged {
            keycode: 0x38,
            flags: 0x0002_0002,
        };
        assert_eq!(run(&mut t, shift, false).0, TapAction::Pass);
        // Already with input swallowed: what is new is swallowed, the releases of what was before pass
        // (and also travel to the other machine, which decides whether they change anything).
        let (action, ev) = run(&mut t, RawEvent::KeyUp { keycode: 0x00 }, true);
        assert_eq!(action, TapAction::Pass);
        assert_eq!(ev, vec![key_ev(0x04, KeyState::Released)]);
        let up = RawEvent::MouseUp {
            button: 0,
            location: at,
        };
        assert_eq!(run(&mut t, up, true).0, TapAction::Pass);
        let shift_up = RawEvent::FlagsChanged {
            keycode: 0x38,
            flags: 0,
        };
        assert_eq!(run(&mut t, shift_up, true).0, TapAction::Pass);
        // What is pressed with input swallowed is swallowed on press and release.
        assert_eq!(
            run(&mut t, RawEvent::KeyDown { keycode: 0x00 }, true).0,
            TapAction::Swallow
        );
        assert_eq!(
            run(&mut t, RawEvent::KeyUp { keycode: 0x00 }, true).0,
            TapAction::Swallow
        );
        assert_eq!(run(&mut t, up, true).0, TapAction::Swallow);
        let shift_down = RawEvent::FlagsChanged {
            keycode: 0x38,
            flags: 0x0002_0002,
        };
        assert_eq!(run(&mut t, shift_down, true).0, TapAction::Swallow);
    }

    /// T-093: the two reasons macOS turns the tap off reactivate it and are
    /// counted (the agent records them).
    #[test]
    fn a_tap_turned_off_is_reactivated_and_recorded() {
        let mut t = Translator::new();
        assert_eq!(t.tap_disabled(), TapDisabledCount::default());
        for by in [
            TapDisabledBy::Timeout,
            TapDisabledBy::UserInput,
            TapDisabledBy::Timeout,
        ] {
            for grabbed in [true, false] {
                assert_eq!(
                    run(&mut t, RawEvent::TapDisabled(by), grabbed),
                    (TapAction::Reenable, vec![])
                );
            }
        }
        assert_eq!(
            t.tap_disabled(),
            TapDisabledCount {
                timeout: 4,
                user_input: 2
            }
        );
    }

    #[test]
    fn modifiers_per_side_with_device_bits() {
        let mut t = Translator::new();
        // Left Shift down: generic bit 0x20000 + NX_DEVICELSHIFTKEYMASK.
        let (_, ev) = run(
            &mut t,
            RawEvent::FlagsChanged {
                keycode: 0x38,
                flags: 0x0002_0002,
            },
            false,
        );
        assert_eq!(ev, vec![key_ev(0xE1, KeyState::Pressed)]);
        // Right Shift down with the left still down.
        let (_, ev) = run(
            &mut t,
            RawEvent::FlagsChanged {
                keycode: 0x3C,
                flags: 0x0002_0006,
            },
            false,
        );
        assert_eq!(ev, vec![key_ev(0xE5, KeyState::Pressed)]);
        // Release the left one: the generic one is still set by the right.
        let (_, ev) = run(
            &mut t,
            RawEvent::FlagsChanged {
                keycode: 0x38,
                flags: 0x0002_0004,
            },
            false,
        );
        assert_eq!(ev, vec![key_ev(0xE1, KeyState::Released)]);
        assert_eq!(
            t.modifiers_down(),
            [ModifierKey::ShiftRight].into_iter().collect::<Modifiers>()
        );
        // Right Cmd (0x36), left Option (0x3A), right Ctrl (0x3E).
        let (_, ev) = run(
            &mut t,
            RawEvent::FlagsChanged {
                keycode: 0x36,
                flags: 0x0010_0014,
            },
            false,
        );
        assert_eq!(ev, vec![key_ev(0xE7, KeyState::Pressed)]);
        let (_, ev) = run(
            &mut t,
            RawEvent::FlagsChanged {
                keycode: 0x3A,
                flags: 0x0018_0034,
            },
            false,
        );
        assert_eq!(ev, vec![key_ev(0xE2, KeyState::Pressed)]);
        let (_, ev) = run(
            &mut t,
            RawEvent::FlagsChanged {
                keycode: 0x3E,
                flags: 0x001C_2034,
            },
            false,
        );
        assert_eq!(ev, vec![key_ev(0xE4, KeyState::Pressed)]);
    }

    #[test]
    fn flags_without_per_side_bits_toggle() {
        let mut t = Translator::new();
        // Synthetic left Cmd: only the generic bit.
        let raw = RawEvent::FlagsChanged {
            keycode: 0x37,
            flags: 0x0010_0000,
        };
        assert_eq!(
            run(&mut t, raw, false).1,
            vec![key_ev(0xE3, KeyState::Pressed)]
        );
        let raw = RawEvent::FlagsChanged {
            keycode: 0x37,
            flags: 0,
        };
        assert_eq!(
            run(&mut t, raw, false).1,
            vec![key_ev(0xE3, KeyState::Released)]
        );
    }

    #[test]
    fn flags_repetidos_no_duplican() {
        let mut t = Translator::new();
        let raw = RawEvent::FlagsChanged {
            keycode: 0x3B,
            flags: 0x0004_0001,
        };
        assert_eq!(run(&mut t, raw, false).1.len(), 1);
        assert!(run(&mut t, raw, false).1.is_empty());
    }

    #[test]
    fn caps_lock_is_a_full_press_every_time() {
        let mut t = Translator::new();
        for flags in [0x0001_0000, 0] {
            let (_, ev) = run(
                &mut t,
                RawEvent::FlagsChanged {
                    keycode: 0x39,
                    flags,
                },
                true,
            );
            assert_eq!(
                ev,
                vec![
                    key_ev(0x39, KeyState::Pressed),
                    key_ev(0x39, KeyState::Released)
                ]
            );
        }
    }

    #[test]
    fn fn_no_viaja() {
        let mut t = Translator::new();
        let raw = RawEvent::FlagsChanged {
            keycode: 0x3F,
            flags: 0x0080_0000,
        };
        assert_eq!(run(&mut t, raw, true), (TapAction::Swallow, vec![]));
    }

    #[test]
    fn buttons_by_number() {
        assert_eq!(mac_button(0), Button::Left);
        assert_eq!(mac_button(1), Button::Right);
        assert_eq!(mac_button(2), Button::Middle);
        assert_eq!(mac_button(3), Button::Back);
        assert_eq!(mac_button(4), Button::Forward);
        assert_eq!(mac_button(9), Button::Other(9));
        assert_eq!(mac_button(1000), Button::Other(255));
        assert_eq!(mac_button(-1), Button::Other(255));
    }

    #[test]
    fn click_with_local_focus_reports_the_position_first() {
        let mut t = Translator::new();
        let at = Point::new(10.0, 20.0);
        let (_, ev) = run(
            &mut t,
            RawEvent::MouseDown {
                button: 1,
                location: at,
            },
            false,
        );
        assert_eq!(
            ev,
            vec![
                InputEvent::MotionAbsolute { x: 10.0, y: 20.0 },
                InputEvent::Button {
                    button: Button::Right,
                    state: KeyState::Pressed
                }
            ]
        );
        let (_, ev) = run(
            &mut t,
            RawEvent::MouseUp {
                button: 1,
                location: at,
            },
            true,
        );
        assert_eq!(
            ev,
            vec![InputEvent::Button {
                button: Button::Right,
                state: KeyState::Released
            }]
        );
    }

    #[test]
    fn local_movement_sends_delta_then_position() {
        let mut t = Translator::new();
        let (_, ev) = run(
            &mut t,
            RawEvent::Moved {
                dx: 3.0,
                dy: -2.0,
                location: Point::new(1919.0, 500.0),
            },
            false,
        );
        assert_eq!(
            ev,
            vec![
                InputEvent::MotionRelative { dx: 3.0, dy: -2.0 },
                InputEvent::MotionAbsolute {
                    x: 1919.0,
                    y: 500.0
                }
            ]
        );
    }

    #[test]
    fn remote_movement_only_sends_delta() {
        let mut t = Translator::new();
        let (a, ev) = run(
            &mut t,
            RawEvent::Moved {
                dx: 3.0,
                dy: 0.0,
                location: ORIGIN,
            },
            true,
        );
        assert_eq!(a, TapAction::Swallow);
        assert_eq!(ev, vec![InputEvent::MotionRelative { dx: 3.0, dy: 0.0 }]);
        // A null movement (e.g. after a warp) sends nothing.
        let (_, ev) = run(
            &mut t,
            RawEvent::Moved {
                dx: 0.0,
                dy: 0.0,
                location: ORIGIN,
            },
            true,
        );
        assert!(ev.is_empty());
    }

    #[test]
    fn wheel_in_v120_without_natural_scrolling() {
        // Wheel forward one notch, without natural scrolling: axis1 = +1.
        assert_eq!(
            scroll(false, 1.0, 0.0, 10.0, 0.0, false),
            Some(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: 120
            })
        );
        // With natural scrolling macOS delivers it inverted; it is undone.
        assert_eq!(
            scroll(false, -1.0, 0.0, -10.0, 0.0, true),
            Some(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: 120
            })
        );
        // Tilt to the left: axis2 positive → x negative.
        assert_eq!(
            scroll(false, 0.0, 0.5, 0.0, 5.0, false),
            Some(Scroll::Wheel {
                dx_v120: -60,
                dy_v120: 0
            })
        );
        // High-resolution wheel: fractions of a notch.
        assert_eq!(
            scroll(false, 0.1, 0.0, 1.0, 0.0, false),
            Some(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: 12
            })
        );
    }

    #[test]
    fn trackpad_en_pixeles() {
        assert_eq!(
            scroll(true, 0.0, 0.0, 12.5, -3.0, false),
            Some(Scroll::Pixels { dx: 3.0, dy: 12.5 })
        );
        assert_eq!(
            scroll(true, 0.0, 0.0, 12.5, -3.0, true),
            Some(Scroll::Pixels {
                dx: -3.0,
                dy: -12.5
            })
        );
    }

    #[test]
    fn null_scroll_does_not_go_out() {
        assert_eq!(scroll(true, 1.0, 1.0, 0.0, 0.0, false), None);
        assert_eq!(scroll(false, 0.001, 0.0, 9.0, 0.0, false), None);
        assert_eq!(scroll(false, f64::NAN, 0.0, 0.0, 0.0, false), None);
    }

    #[test]
    fn the_translator_undoes_natural_scrolling() {
        let mut t = Translator::new();
        let raw = RawEvent::Scroll {
            continuous: true,
            lines_axis1: 0.0,
            lines_axis2: 0.0,
            points_axis1: -4.0,
            points_axis2: 0.0,
        };
        let mut out = Vec::new();
        t.translate(&raw, true, true, &mut |e| out.push(e));
        assert_eq!(
            out,
            vec![InputEvent::Scroll(Scroll::Pixels { dx: 0.0, dy: 4.0 })]
        );
    }

    #[test]
    fn teclas_multimedia() {
        let down = |code: i64| (code << 16) | 0x0A00;
        let up = |code: i64| (code << 16) | 0x0B00;
        assert_eq!(
            aux_key(down(0)),
            Some((HidUsage::consumer(0xE9), KeyState::Pressed))
        );
        assert_eq!(
            aux_key(up(1)),
            Some((HidUsage::consumer(0xEA), KeyState::Released))
        );
        assert_eq!(
            aux_key(down(7)),
            Some((HidUsage::consumer(0xE2), KeyState::Pressed))
        );
        assert_eq!(
            aux_key(down(16)),
            Some((HidUsage::consumer(0xCD), KeyState::Pressed))
        );
        // Repeat (bit 0) does not change the state.
        assert_eq!(
            aux_key(down(17) | 1),
            Some((HidUsage::consumer(0xB5), KeyState::Pressed))
        );
        // Caps Lock also arrives through FlagsChanged: not here.
        assert_eq!(aux_key(down(4)), None);
        // Unknown state.
        assert_eq!(aux_key(16 << 16), None);

        let mut t = Translator::new();
        let raw = RawEvent::SystemDefined {
            subtype: SUBTYPE_AUX_CONTROL_BUTTONS,
            data1: up(18),
        };
        assert_eq!(
            run(&mut t, raw, true),
            (
                TapAction::Swallow,
                vec![InputEvent::Key {
                    usage: HidUsage::consumer(0xB6),
                    state: KeyState::Released
                }]
            )
        );
    }

    fn arb_raw() -> impl Strategy<Value = RawEvent> {
        let p = (-5000.0..5000.0f64, -5000.0..5000.0f64).prop_map(|(x, y)| Point::new(x, y));
        prop_oneof![
            any::<u16>().prop_map(|keycode| RawEvent::KeyDown { keycode }),
            any::<u16>().prop_map(|keycode| RawEvent::KeyUp { keycode }),
            (0x36u16..0x40, any::<u64>())
                .prop_map(|(keycode, flags)| RawEvent::FlagsChanged { keycode, flags }),
            (any::<i64>(), p.clone())
                .prop_map(|(button, location)| RawEvent::MouseDown { button, location }),
            (any::<i64>(), p.clone())
                .prop_map(|(button, location)| RawEvent::MouseUp { button, location }),
            (-100.0..100.0f64, -100.0..100.0f64, p)
                .prop_map(|(dx, dy, location)| RawEvent::Moved { dx, dy, location }),
            (
                any::<bool>(),
                -10.0..10.0f64,
                -10.0..10.0f64,
                -100.0..100.0f64,
                -100.0..100.0f64
            )
                .prop_map(|(continuous, l1, l2, p1, p2)| RawEvent::Scroll {
                    continuous,
                    lines_axis1: l1,
                    lines_axis2: l2,
                    points_axis1: p1,
                    points_axis2: p2,
                }),
            (any::<i16>(), any::<i64>())
                .prop_map(|(subtype, data1)| RawEvent::SystemDefined { subtype, data1 }),
            Just(RawEvent::TapDisabled(TapDisabledBy::Timeout)),
            Just(RawEvent::TapDisabled(TapDisabledBy::UserInput)),
            Just(RawEvent::Other),
        ]
    }

    proptest! {
        /// With remote focus the frozen cursor's position is never reported, and
        /// the modifiers the translator believes are down match the modifier
        /// `Pressed`/`Released` events it emitted.
        #[test]
        fn translator_invariants(events in prop::collection::vec((arb_raw(), any::<bool>()), 0..64)) {
            let mut t = Translator::new();
            let mut mods = Modifiers::EMPTY;
            for (raw, grabbed) in events {
                let mut out = Vec::new();
                let action = t.translate(&raw, grabbed, false, &mut |e| out.push(e));
                if !grabbed {
                    prop_assert_ne!(action, TapAction::Swallow);
                }
                for e in out {
                    match e {
                        InputEvent::MotionAbsolute { .. } => prop_assert!(!grabbed),
                        InputEvent::Key { usage, state } => {
                            if let Some(m) = usage.modifier() {
                                match state {
                                    KeyState::Pressed => {
                                        prop_assert!(!mods.contains(m));
                                        mods.insert(m);
                                    }
                                    KeyState::Released => {
                                        prop_assert!(mods.contains(m));
                                        mods.remove(m);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                prop_assert_eq!(mods, t.modifiers_down());
            }
        }
    }
}
