use serde::{Deserialize, Serialize};

use crate::model::Machine;

/// Which machines are on, according to whoever keeps the state.
///
/// There is no central machine (DESIGN §3.7): the local machine is always on
/// for itself, and the other is on while some link sees it or a step turned it on.
///
/// A state saved before the A/B sides (`mac`/`pc`) carries neither: it is read as
/// everything off and the agent rebuilds it (the local one always on; the other per the links).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Powered {
    pub a: bool,
    pub b: bool,
}

impl Powered {
    pub fn is_on(&self, machine: Machine) -> bool {
        match machine {
            Machine::A => self.a,
            Machine::B => self.b,
        }
    }

    pub fn set(&mut self, machine: Machine, on: bool) {
        match machine {
            Machine::A => self.a = on,
            Machine::B => self.b = on,
        }
    }

    /// Only `machine` on.
    pub fn only(machine: Machine) -> Self {
        let mut p = Powered::default();
        p.set(machine, true);
        p
    }

    /// Both on.
    pub fn both() -> Self {
        Powered { a: true, b: true }
    }

    /// The same with the sides swapped (the machine discovered it is the other side).
    pub fn swapped(self) -> Self {
        Powered {
            a: self.b,
            b: self.a,
        }
    }
}

/// Desk state according to Tandem: which machines are on and which scene is in place.
///
/// A state saved by an earlier version also carries the monitors' data (`owner`,
/// `builtin_pref`, `doubt`); it is ignored when read.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct State {
    /// A state saved with `pc_on` (before T-084) does not carry it: the agent rebuilds it
    /// from what the links see.
    #[serde(default)]
    pub on: Powered,
    /// The id of the scene last applied that still holds (D6). `None` if
    /// one was never applied, was deleted or a machine it needed was turned off.
    #[serde(default)]
    pub scene: Option<String>,
}

impl State {
    /// The starting state seen from `me`: only `me` on, because nothing is known yet
    /// about the other.
    pub fn initial(me: Machine) -> Self {
        State {
            on: Powered::only(me),
            scene: None,
        }
    }

    pub fn is_on(&self, machine: Machine) -> bool {
        self.on.is_on(machine)
    }

    /// The same state with the sides swapped.
    pub fn swapped(&self) -> Self {
        State {
            on: self.on.swapped(),
            scene: self.scene.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_machine_starts_on_and_the_other_off() {
        for me in Machine::ALL {
            let s = State::initial(me);
            assert!(s.is_on(me));
            assert!(!s.is_on(me.other()));
        }
    }

    #[test]
    fn a_state_saved_with_monitors_still_loads() {
        let s: State = serde_json::from_str(
            r#"{"owner":{"m34":"b"},"on":{"a":true,"b":true},"builtin_pref":"off","doubt":{"m34":"unknown"}}"#,
        )
        .unwrap();
        assert_eq!(s.on, Powered::both());
    }

    /// Before the A/B sides `mac`/`pc` was saved: which is which is not guessed.
    #[test]
    fn a_state_saved_with_mac_and_pc_loads_as_unknown() {
        let s: State = serde_json::from_str(r#"{"on":{"mac":true,"pc":true}}"#).unwrap();
        assert_eq!(s.on, Powered::default());
    }

    #[test]
    fn swapping_sides_swaps_who_is_on() {
        let s = State::initial(Machine::A).swapped();
        assert!(s.is_on(Machine::B) && !s.is_on(Machine::A));
    }

    #[test]
    fn a_state_saved_with_pc_on_still_loads() {
        let s: State =
            serde_json::from_str(r#"{"owner":{"m34":"pc"},"pc_on":true,"builtin_pref":"mirror"}"#)
                .unwrap();
        assert_eq!(s.on, Powered::default());
    }
}
