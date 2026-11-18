//! Modifier remapping, configurable per direction.
//!
//! By default Mac→Win: Cmd→Ctrl, Ctrl→Win, Option→Alt, right Option→AltGr;
//! and the reverse for Win→Mac (docs/research/input-engine.md). It is applied on the
//! server before sending, so the client injects what it receives as is.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{InputEvent, ModifierKey, Modifiers};

/// Direction in which input travels: from the machine that captures to the one that injects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    MacToWin,
    WinToMac,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RemapError {
    /// Two modifiers would go to the same key: releasing one would release the other at the
    /// destination while it is still pressed at the origin.
    #[error("{first:?} and {second:?} are both remapped to {target:?}")]
    NotBijective {
        first: ModifierKey,
        second: ModifierKey,
        target: ModifierKey,
    },
}

/// Destination of each source modifier. It is always a permutation of the eight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "RemapTable", into = "RemapTable")]
pub struct ModifierRemap {
    /// Indexed by the source's `ModifierKey::bit()`.
    targets: [ModifierKey; 8],
}

/// Shape of the configuration: one field per source modifier.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemapTable {
    control_left: ModifierKey,
    shift_left: ModifierKey,
    alt_left: ModifierKey,
    meta_left: ModifierKey,
    control_right: ModifierKey,
    shift_right: ModifierKey,
    alt_right: ModifierKey,
    meta_right: ModifierKey,
}

impl TryFrom<RemapTable> for ModifierRemap {
    type Error = RemapError;

    fn try_from(t: RemapTable) -> Result<Self, RemapError> {
        ModifierRemap::new([
            t.control_left,
            t.shift_left,
            t.alt_left,
            t.meta_left,
            t.control_right,
            t.shift_right,
            t.alt_right,
            t.meta_right,
        ])
    }
}

impl From<ModifierRemap> for RemapTable {
    fn from(r: ModifierRemap) -> Self {
        let [
            control_left,
            shift_left,
            alt_left,
            meta_left,
            control_right,
            shift_right,
            alt_right,
            meta_right,
        ] = r.targets;
        RemapTable {
            control_left,
            shift_left,
            alt_left,
            meta_left,
            control_right,
            shift_right,
            alt_right,
            meta_right,
        }
    }
}

impl ModifierRemap {
    /// `targets[i]` is the destination of `ModifierKey::ALL[i]`.
    pub fn new(targets: [ModifierKey; 8]) -> Result<Self, RemapError> {
        let mut seen: [Option<ModifierKey>; 8] = [None; 8];
        for (source, target) in ModifierKey::ALL.into_iter().zip(targets) {
            let slot = &mut seen[usize::from(target.bit())];
            if let Some(first) = *slot {
                return Err(RemapError::NotBijective {
                    first,
                    second: source,
                    target,
                });
            }
            *slot = Some(source);
        }
        Ok(Self { targets })
    }

    pub const fn identity() -> Self {
        Self {
            targets: ModifierKey::ALL,
        }
    }

    /// Mac→Win: Cmd→Ctrl, Ctrl→Win, Option→Alt, right Option→AltGr.
    pub const fn mac_to_win() -> Self {
        use ModifierKey::*;
        Self {
            targets: [
                MetaLeft,     // left Ctrl → left Win
                ShiftLeft,    //
                AltLeft,      // left Option → Alt
                ControlLeft,  // left Cmd → left Ctrl
                MetaRight,    // right Ctrl → right Win
                ShiftRight,   //
                AltRight,     // right Option → AltGr
                ControlRight, // right Cmd → right Ctrl
            ],
        }
    }

    /// Win→Mac: the inverse of [`ModifierRemap::mac_to_win`].
    pub const fn win_to_mac() -> Self {
        Self::mac_to_win().inverse()
    }

    pub const fn inverse(self) -> Self {
        let mut targets = ModifierKey::ALL;
        let mut i = 0;
        while i < 8 {
            targets[self.targets[i].bit() as usize] = ModifierKey::ALL[i];
            i += 1;
        }
        Self { targets }
    }

    pub const fn map(&self, key: ModifierKey) -> ModifierKey {
        self.targets[key.bit() as usize]
    }

    pub fn map_modifiers(&self, mods: Modifiers) -> Modifiers {
        mods.iter().map(|k| self.map(k)).collect()
    }

    /// Remaps an event; what is not a modifier passes unchanged.
    pub fn apply(&self, event: InputEvent) -> InputEvent {
        match event {
            InputEvent::Key { usage, state } => match usage.modifier() {
                Some(m) => InputEvent::Key {
                    usage: self.map(m).usage(),
                    state,
                },
                None => event,
            },
            InputEvent::Modifiers(m) => InputEvent::Modifiers(self.map_modifiers(m)),
            other => other,
        }
    }
}

impl Default for ModifierRemap {
    fn default() -> Self {
        Self::identity()
    }
}

/// The full policy: one table per direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemapPolicy {
    pub mac_to_win: ModifierRemap,
    pub win_to_mac: ModifierRemap,
}

impl Default for RemapPolicy {
    fn default() -> Self {
        Self {
            mac_to_win: ModifierRemap::mac_to_win(),
            win_to_mac: ModifierRemap::win_to_mac(),
        }
    }
}

impl RemapPolicy {
    pub fn for_direction(&self, direction: Direction) -> &ModifierRemap {
        match direction {
            Direction::MacToWin => &self.mac_to_win,
            Direction::WinToMac => &self.win_to_mac,
        }
    }

    pub fn apply(&self, direction: Direction, event: InputEvent) -> InputEvent {
        self.for_direction(direction).apply(event)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Button, HidUsage, KeyState, Scroll};
    use super::*;
    use ModifierKey::*;
    use proptest::prelude::*;

    fn key(m: ModifierKey) -> InputEvent {
        InputEvent::Key {
            usage: m.usage(),
            state: KeyState::Pressed,
        }
    }

    #[test]
    fn default_mac_to_win() {
        let p = RemapPolicy::default();
        let d = Direction::MacToWin;
        assert_eq!(p.apply(d, key(MetaLeft)), key(ControlLeft), "Cmd→Ctrl");
        assert_eq!(
            p.apply(d, key(MetaRight)),
            key(ControlRight),
            "right Cmd→right Ctrl"
        );
        assert_eq!(p.apply(d, key(ControlLeft)), key(MetaLeft), "Ctrl→Win");
        assert_eq!(p.apply(d, key(ControlRight)), key(MetaRight));
        assert_eq!(p.apply(d, key(AltLeft)), key(AltLeft), "Option→Alt");
        assert_eq!(
            p.apply(d, key(AltRight)),
            key(AltRight),
            "right Option→AltGr"
        );
        assert_eq!(p.apply(d, key(ShiftLeft)), key(ShiftLeft));
        assert_eq!(p.apply(d, key(ShiftRight)), key(ShiftRight));
    }

    #[test]
    fn default_win_to_mac() {
        let p = RemapPolicy::default();
        let d = Direction::WinToMac;
        assert_eq!(p.apply(d, key(ControlLeft)), key(MetaLeft), "Ctrl→Cmd");
        assert_eq!(p.apply(d, key(ControlRight)), key(MetaRight));
        assert_eq!(p.apply(d, key(MetaLeft)), key(ControlLeft), "Win→Ctrl");
        assert_eq!(p.apply(d, key(MetaRight)), key(ControlRight));
        assert_eq!(p.apply(d, key(AltLeft)), key(AltLeft), "Alt→Option");
        assert_eq!(
            p.apply(d, key(AltRight)),
            key(AltRight),
            "AltGr→right Option"
        );
    }

    #[test]
    fn copy_on_mac_is_ctrl_c_on_windows() {
        let p = RemapPolicy::default();
        let c = InputEvent::Key {
            usage: HidUsage::key(0x06),
            state: KeyState::Pressed,
        };
        let cmd_c = Modifiers::from_iter([MetaLeft]);
        assert_eq!(p.apply(Direction::MacToWin, c), c);
        assert_eq!(
            p.apply(Direction::MacToWin, InputEvent::Modifiers(cmd_c)),
            InputEvent::Modifiers(Modifiers::from_iter([ControlLeft]))
        );
    }

    #[test]
    fn other_events_pass_unchanged() {
        let p = RemapPolicy::default();
        for e in [
            InputEvent::Button {
                button: Button::Left,
                state: KeyState::Released,
            },
            InputEvent::MotionRelative { dx: 1.5, dy: -2.0 },
            InputEvent::MotionAbsolute { x: 10.0, y: 20.0 },
            InputEvent::Scroll(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: 120,
            }),
            InputEvent::Key {
                usage: HidUsage::consumer(0xE0),
                state: KeyState::Pressed,
            },
        ] {
            assert_eq!(p.apply(Direction::MacToWin, e), e);
            assert_eq!(p.apply(Direction::WinToMac, e), e);
        }
    }

    #[test]
    fn rejects_non_bijective_tables() {
        let mut t = ModifierKey::ALL;
        t[0] = MetaLeft; // left Ctrl and left Cmd → left Cmd
        assert_eq!(
            ModifierRemap::new(t),
            Err(RemapError::NotBijective {
                first: ControlLeft,
                second: MetaLeft,
                target: MetaLeft,
            })
        );
    }

    #[test]
    fn identity_and_default() {
        let id = ModifierRemap::default();
        assert_eq!(id, ModifierRemap::identity());
        for k in ModifierKey::ALL {
            assert_eq!(id.map(k), k);
        }
    }

    #[test]
    fn toml_configuration_per_direction() {
        let toml = r#"
            [mac_to_win]
            control_left = "control_left"
            shift_left = "shift_left"
            alt_left = "alt_left"
            meta_left = "meta_left"
            control_right = "control_right"
            shift_right = "shift_right"
            alt_right = "alt_right"
            meta_right = "meta_right"
        "#;
        let p: RemapPolicy = toml::from_str(toml).unwrap();
        assert_eq!(p.mac_to_win, ModifierRemap::identity());
        assert_eq!(
            p.win_to_mac,
            ModifierRemap::win_to_mac(),
            "what is omitted stays at the default"
        );

        let s = toml::to_string(&RemapPolicy::default()).unwrap();
        assert!(s.contains("meta_left = \"control_left\""), "{s}");
        assert_eq!(
            toml::from_str::<RemapPolicy>(&s).unwrap(),
            RemapPolicy::default()
        );
    }

    #[test]
    fn an_invalid_configuration_is_rejected() {
        let toml = r#"
            [win_to_mac]
            control_left = "meta_left"
            shift_left = "shift_left"
            alt_left = "alt_left"
            meta_left = "meta_left"
            control_right = "control_right"
            shift_right = "shift_right"
            alt_right = "alt_right"
            meta_right = "meta_right"
        "#;
        let err = toml::from_str::<RemapPolicy>(toml).unwrap_err().to_string();
        assert!(err.contains("are both remapped"), "{err}");
    }

    fn any_remap() -> impl Strategy<Value = ModifierRemap> {
        Just(ModifierKey::ALL.to_vec())
            .prop_shuffle()
            .prop_map(|v| {
                let mut t = ModifierKey::ALL;
                t.copy_from_slice(&v);
                ModifierRemap::new(t).unwrap()
            })
    }

    proptest! {
        #[test]
        fn inversa_deshace(r in any_remap(), bits in any::<u8>()) {
            let m = Modifiers::from_bits(bits);
            prop_assert_eq!(r.inverse().map_modifiers(r.map_modifiers(m)), m);
            prop_assert_eq!(r.inverse().inverse(), r);
        }

        #[test]
        fn keeps_the_number_of_modifiers(r in any_remap(), bits in any::<u8>()) {
            let m = Modifiers::from_bits(bits);
            prop_assert_eq!(r.map_modifiers(m).bits().count_ones(), bits.count_ones());
        }

        #[test]
        fn modifiers_match_keys(r in any_remap(), bits in any::<u8>()) {
            // Remapping the set = remapping each pressed key.
            let m = Modifiers::from_bits(bits);
            let per_key: Modifiers = m
                .iter()
                .filter_map(|k| match r.apply(key(k)) {
                    InputEvent::Key { usage, .. } => usage.modifier(),
                    _ => None,
                })
                .collect();
            prop_assert_eq!(r.map_modifiers(m), per_key);
        }
    }

    #[test]
    fn win_a_mac_deshace_mac_a_win() {
        let p = RemapPolicy::default();
        for k in ModifierKey::ALL {
            assert_eq!(p.win_to_mac.map(p.mac_to_win.map(k)), k);
        }
    }
}
