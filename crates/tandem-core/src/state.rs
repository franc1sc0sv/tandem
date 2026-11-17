use serde::{Deserialize, Serialize};

use crate::model::Machine;

/// Qué máquinas están encendidas, según quien lleva el estado.
///
/// No hay una máquina central (DESIGN §3.7): la máquina local siempre está encendida
/// para sí misma, y la otra lo está mientras algún enlace la ve o un paso la encendió.
///
/// Un estado guardado antes de los lados A/B (`mac`/`pc`) no trae ninguno: se lee como
/// todo apagado y el agente lo rehace (la local siempre prendida; la otra según los enlaces).
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

    /// Solo `machine` encendida.
    pub fn only(machine: Machine) -> Self {
        let mut p = Powered::default();
        p.set(machine, true);
        p
    }

    /// Las dos encendidas.
    pub fn both() -> Self {
        Powered { a: true, b: true }
    }

    /// Lo mismo con los lados cambiados (la máquina descubrió que es el otro lado).
    pub fn swapped(self) -> Self {
        Powered {
            a: self.b,
            b: self.a,
        }
    }
}

/// Estado del puesto según Tandem: hoy, qué máquinas están encendidas.
///
/// Un estado guardado por una versión anterior trae además lo de los monitores (`owner`,
/// `builtin_pref`, `doubt`); se ignora al leerlo.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct State {
    /// Un estado guardado con `pc_on` (antes de T-084) no la trae: el agente la rehace
    /// con lo que ven los enlaces.
    #[serde(default)]
    pub on: Powered,
}

impl State {
    /// El estado de partida visto desde `me`: solo `me` encendida, porque de la otra
    /// todavía no se sabe nada.
    pub fn initial(me: Machine) -> Self {
        State {
            on: Powered::only(me),
        }
    }

    pub fn is_on(&self, machine: Machine) -> bool {
        self.on.is_on(machine)
    }

    /// El mismo estado con los lados cambiados.
    pub fn swapped(&self) -> Self {
        State {
            on: self.on.swapped(),
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

    /// Antes de los lados A/B se guardaba `mac`/`pc`: no se adivina cuál es cuál.
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
