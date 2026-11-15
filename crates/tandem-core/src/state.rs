use serde::{Deserialize, Serialize};

use crate::model::Machine;

/// Qué máquinas están encendidas, según quien lleva el estado.
///
/// No hay una máquina central (DESIGN §3.7): la máquina local siempre está encendida
/// para sí misma, y la otra lo está mientras algún enlace la ve o un paso la encendió.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Powered {
    pub mac: bool,
    pub pc: bool,
}

impl Powered {
    pub fn is_on(&self, machine: Machine) -> bool {
        match machine {
            Machine::Mac => self.mac,
            Machine::Pc => self.pc,
        }
    }

    pub fn set(&mut self, machine: Machine, on: bool) {
        match machine {
            Machine::Mac => self.mac = on,
            Machine::Pc => self.pc = on,
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
        Powered {
            mac: true,
            pc: true,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_machine_starts_on_and_the_other_off() {
        for me in [Machine::Mac, Machine::Pc] {
            let s = State::initial(me);
            assert!(s.is_on(me));
            assert!(!s.is_on(me.other()));
        }
    }

    #[test]
    fn a_state_saved_with_monitors_still_loads() {
        let s: State = serde_json::from_str(
            r#"{"owner":{"m34":"pc"},"on":{"mac":true,"pc":true},"builtin_pref":"off","doubt":{"m34":"unknown"}}"#,
        )
        .unwrap();
        assert_eq!(s.on, Powered::both());
    }

    #[test]
    fn a_state_saved_with_pc_on_still_loads() {
        let s: State =
            serde_json::from_str(r#"{"owner":{"m34":"pc"},"pc_on":true,"builtin_pref":"mirror"}"#)
                .unwrap();
        assert_eq!(s.on, Powered::default());
    }
}
