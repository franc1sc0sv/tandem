//! Teclas y botones presionados, para poder soltarlos todos de una vez.
//!
//! Al salir de la pantalla, al desconectarse o por timeout, el cliente suelta todo
//! lo que el servidor dejó presionado; si no, una tecla quedaría pegada.

use std::collections::BTreeSet;

use super::{Button, HidUsage, InputEvent, KeyState, Modifiers};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pressed {
    keys: BTreeSet<HidUsage>,
    buttons: BTreeSet<Button>,
}

/// Qué cambió al registrar un evento.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// La tecla o el botón cambió de estado.
    Changed,
    /// Se presionó algo que ya estaba presionado: repetición automática.
    Repeat,
    /// Se soltó algo que no estaba presionado (o el evento no es de tecla/botón).
    Ignored,
}

impl Pressed {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra un evento y dice si cambió algo.
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

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.buttons.is_empty()
    }

    /// Modificadores presionados en este momento.
    pub fn modifiers(&self) -> Modifiers {
        self.keys.iter().filter_map(|u| u.modifier()).collect()
    }

    /// Vacía el conjunto y devuelve los eventos para soltar todo.
    ///
    /// Orden: primero botones, después teclas normales y al final los modificadores,
    /// para que soltar no dispare un atajo (p. ej. soltar Ctrl antes que C).
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
    fn presionar_repetir_soltar() {
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
    fn release_all_suelta_modificadores_al_final() {
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
        fn release_all_deja_todo_suelto(events in prop::collection::vec(any_event(), 0..64)) {
            let mut p = Pressed::new();
            for e in &events {
                p.record(e);
            }
            let before = p.clone();
            let releases = p.release_all();
            prop_assert!(p.is_empty());

            // Exactamente lo que estaba presionado, una vez cada uno, y nada más.
            let mut replay = before.clone();
            for e in &releases {
                prop_assert_eq!(replay.record(e), Change::Changed);
            }
            prop_assert!(replay.is_empty());
            prop_assert_eq!(releases.len(), before.keys.len() + before.buttons.len());

            // Ningún modificador se suelta antes que una tecla normal.
            if let Some(i) = releases.iter().position(is_modifier) {
                prop_assert!(releases[i..].iter().all(is_modifier));
            }
        }

        #[test]
        fn modifiers_refleja_las_teclas(events in prop::collection::vec(any_event(), 0..64)) {
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
