//! Pressed keys and buttons, to be able to release them all at once.
//!
//! On leaving the screen, on disconnecting or on timeout, the client releases everything
//! the server left pressed; otherwise a key would be left stuck.

use std::collections::BTreeSet;

use super::{Button, HidUsage, InputEvent, KeyState, Modifiers};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pressed {
    keys: BTreeSet<HidUsage>,
    buttons: BTreeSet<Button>,
}

/// What changed on recording an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The key or button changed state.
    Changed,
    /// Something already pressed was pressed: automatic repeat.
    Repeat,
    /// Something not pressed was released (or the event is not a key/button).
    Ignored,
}

impl Pressed {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an event and says whether anything changed.
    pub fn record(&mut self, event: &InputEvent) -> Change {
        match *event {
            InputEvent::Key { usage, state } => update(&mut self.keys, usage, state),
            InputEvent::Button { button, state } => update(&mut self.buttons, button, state),
            _ => Change::Ignored,
        }
    }

    pub fn is_key_pressed(&self, usage: HidUsage) -> bool {
        self.keys.contains(&usage)
    }

    pub fn is_button_pressed(&self, button: Button) -> bool {
        self.buttons.contains(&button)
    }

    /// Some mouse button is pressed (a drag is under way).
    pub fn any_button(&self) -> bool {
        !self.buttons.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.buttons.is_empty()
    }

    /// Modifiers pressed right now.
    pub fn modifiers(&self) -> Modifiers {
        self.keys.iter().filter_map(|u| u.modifier()).collect()
    }

    /// Empties the set and returns the events to release everything.
    ///
    /// Order: buttons first, then normal keys and finally modifiers,
    /// so that releasing does not trigger a shortcut (e.g. releasing Ctrl before C).
    pub fn release_all(&mut self) -> Vec<InputEvent> {
        let buttons = std::mem::take(&mut self.buttons);
        let keys = std::mem::take(&mut self.keys);
        let (mods, normal): (Vec<_>, Vec<_>) =
            keys.into_iter().partition(|u| u.modifier().is_some());

        let buttons = buttons.into_iter().map(|button| InputEvent::Button {
            button,
            state: KeyState::Released,
        });
        let keys = normal.into_iter().chain(mods).map(|usage| InputEvent::Key {
            usage,
            state: KeyState::Released,
        });
        buttons.chain(keys).collect()
    }
}

fn update<T: Ord + Copy>(set: &mut BTreeSet<T>, item: T, state: KeyState) -> Change {
    match state {
        KeyState::Pressed if set.insert(item) => Change::Changed,
        KeyState::Pressed => Change::Repeat,
        KeyState::Released if set.remove(&item) => Change::Changed,
        KeyState::Released => Change::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::super::ModifierKey;
    use super::*;
    use proptest::prelude::*;

    fn key(id: u16, state: KeyState) -> InputEvent {
        InputEvent::Key {
            usage: HidUsage::key(id),
            state,
        }
    }

    #[test]
    fn press_repeat_release() {
        let mut p = Pressed::new();
        assert_eq!(p.record(&key(0x04, KeyState::Pressed)), Change::Changed);
        assert_eq!(p.record(&key(0x04, KeyState::Pressed)), Change::Repeat);
        assert!(p.is_key_pressed(HidUsage::key(0x04)));
        assert_eq!(p.record(&key(0x04, KeyState::Released)), Change::Changed);
        assert_eq!(p.record(&key(0x04, KeyState::Released)), Change::Ignored);
        assert!(p.is_empty());
        assert_eq!(
            p.record(&InputEvent::MotionRelative { dx: 1.0, dy: 1.0 }),
            Change::Ignored
        );
    }

    #[test]
    fn release_all_releases_modifiers_last() {
        let mut p = Pressed::new();
        p.record(&key(0xE0, KeyState::Pressed)); // Ctrl
        p.record(&key(0x06, KeyState::Pressed)); // C
        p.record(&InputEvent::Button {
            button: Button::Left,
            state: KeyState::Pressed,
        });
        assert_eq!(
            p.modifiers(),
            Modifiers::from_iter([ModifierKey::ControlLeft])
        );
        assert!(p.is_button_pressed(Button::Left));

        assert_eq!(
            p.release_all(),
            vec![
                InputEvent::Button {
                    button: Button::Left,
                    state: KeyState::Released
                },
                key(0x06, KeyState::Released),
                key(0xE0, KeyState::Released),
            ]
        );
        assert!(p.is_empty());
        assert!(p.release_all().is_empty());
    }

    fn is_modifier(e: &InputEvent) -> bool {
        matches!(e, InputEvent::Key { usage, .. } if usage.modifier().is_some())
    }

    fn any_event() -> impl Strategy<Value = InputEvent> {
        let state = prop_oneof![Just(KeyState::Pressed), Just(KeyState::Released)];
        let usage = prop_oneof![0x04u16..0x10, 0xE0u16..=0xE7].prop_map(HidUsage::key);
        let button = prop_oneof![
            Just(Button::Left),
            Just(Button::Right),
            Just(Button::Middle),
            (5u8..8).prop_map(Button::Other),
        ];
        prop_oneof![
            (usage, state.clone()).prop_map(|(usage, state)| InputEvent::Key { usage, state }),
            (button, state).prop_map(|(button, state)| InputEvent::Button { button, state }),
            Just(InputEvent::MotionRelative { dx: 1.0, dy: 0.0 }),
        ]
    }

    proptest! {
        #[test]
        fn release_all_leaves_everything_released(events in prop::collection::vec(any_event(), 0..64)) {
            let mut p = Pressed::new();
            for e in &events {
                p.record(e);
            }
            let before = p.clone();
            let releases = p.release_all();
            prop_assert!(p.is_empty());

            // Exactly what was pressed, once each, and nothing else.
            let mut replay = before.clone();
            for e in &releases {
                prop_assert_eq!(replay.record(e), Change::Changed);
            }
            prop_assert!(replay.is_empty());
            prop_assert_eq!(releases.len(), before.keys.len() + before.buttons.len());

            // No modifier is released before a normal key.
            if let Some(i) = releases.iter().position(is_modifier) {
                prop_assert!(releases[i..].iter().all(is_modifier));
            }
        }

        /// QA round 1: the server remaps and the client keeps its own `Pressed`.
        /// Releasing everything at the destination and undoing the remap gives exactly what
        /// was pressed at the origin: no modifier is left stuck.
        #[test]
        fn releasing_all_after_remapping_releases_the_same_as_at_the_origin(
            events in prop::collection::vec(any_event(), 0..64),
            perm in Just(ModifierKey::ALL).prop_shuffle(),
        ) {
            use super::super::ModifierRemap;
            let remap = ModifierRemap::new(perm).unwrap();
            let mut origin = Pressed::new();
            let mut target = Pressed::new();
            for e in &events {
                let a = origin.record(e);
                let b = target.record(&remap.apply(*e));
                // The remap is bijective: the destination sees the same changes.
                prop_assert_eq!(a, b);
            }
            let back: Vec<InputEvent> = target
                .release_all()
                .into_iter()
                .map(|e| remap.inverse().apply(e))
                .collect();
            let mut want = origin.release_all();
            let mut got = back.clone();
            let key = |e: &InputEvent| format!("{e:?}");
            want.sort_by_key(key);
            got.sort_by_key(key);
            prop_assert_eq!(got, want);
            // At the destination the modifiers are also released last.
            if let Some(i) = back.iter().position(is_modifier) {
                prop_assert!(back[i..].iter().all(is_modifier));
            }
        }

        #[test]
        fn modifiers_reflects_the_keys(events in prop::collection::vec(any_event(), 0..64)) {
            let mut p = Pressed::new();
            for e in &events {
                p.record(e);
            }
            for k in ModifierKey::ALL {
                prop_assert_eq!(p.modifiers().contains(k), p.is_key_pressed(k.usage()));
            }
        }
    }
}
