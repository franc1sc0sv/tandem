use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{Config, Machine, MonitorId};

/// Preferencia para la pantalla integrada de la MacBook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuiltinDisplay {
    /// Fuera del escritorio: ninguna ventana puede ir ahí.
    #[default]
    Off,
    /// Refleja el monitor principal de la Mac.
    Mirror,
    /// Pantalla independiente (comportamiento normal de macOS).
    Extend,
}

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

/// Estado del puesto según Tandem.
///
/// Los monitores no reportan su entrada de forma fiable, así que `owner`
/// es la fuente de verdad: se actualiza solo cuando un paso se aplicó con éxito.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// Máquina que cada monitor está mostrando.
    pub owner: BTreeMap<MonitorId, Machine>,
    /// Qué máquinas están encendidas. Un estado guardado por una versión anterior (con
    /// `pc_on`) no la trae: el agente la rehace con lo que ven los enlaces.
    #[serde(default)]
    pub on: Powered,
    /// Preferencia del usuario; la pantalla efectiva la da [`State::builtin_effective`].
    pub builtin_pref: BuiltinDisplay,
}

impl State {
    /// El estado de partida visto desde `me`: todo en la Mac (hasta que el usuario
    /// confirme lo que muestra cada monitor) y solo `me` encendida, porque de la otra
    /// todavía no se sabe nada.
    pub fn initial(cfg: &Config, me: Machine) -> Self {
        State {
            owner: cfg
                .monitors
                .iter()
                .map(|m| (m.id.clone(), Machine::Mac))
                .collect(),
            on: Powered::only(me),
            builtin_pref: BuiltinDisplay::default(),
        }
    }

    pub fn is_on(&self, machine: Machine) -> bool {
        self.on.is_on(machine)
    }

    /// Monitores que una máquina está mostrando, en el orden del escritorio.
    pub fn monitors_of<'a>(
        &'a self,
        cfg: &'a Config,
        machine: Machine,
    ) -> impl Iterator<Item = &'a MonitorId> + 'a {
        cfg.monitors
            .iter()
            .map(|m| &m.id)
            .filter(move |id| self.owner.get(*id) == Some(&machine))
    }

    /// Si la Mac se queda sin monitores externos, su pantalla integrada se enciende sola.
    pub fn builtin_effective(&self, cfg: &Config) -> BuiltinDisplay {
        if self.monitors_of(cfg, Machine::Mac).next().is_none() {
            BuiltinDisplay::Extend
        } else {
            self.builtin_pref
        }
    }

    /// Una máquina es visible si está encendida y tiene algo donde mirar: un monitor o,
    /// en la Mac, su pantalla.
    pub fn is_visible(&self, cfg: &Config, machine: Machine) -> bool {
        if !self.is_on(machine) {
            return false;
        }
        match machine {
            Machine::Pc => self.monitors_of(cfg, Machine::Pc).next().is_some(),
            Machine::Mac => {
                self.monitors_of(cfg, Machine::Mac).next().is_some()
                    || self.builtin_effective(cfg) != BuiltinDisplay::Off
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixtures::desk;

    #[test]
    fn builtin_turns_on_without_externals() {
        let cfg = desk();
        let mut s = State::initial(&cfg, Machine::Mac);
        assert_eq!(s.builtin_effective(&cfg), BuiltinDisplay::Off);
        for o in s.owner.values_mut() {
            *o = Machine::Pc;
        }
        assert_eq!(s.builtin_effective(&cfg), BuiltinDisplay::Extend);
    }

    #[test]
    fn pc_without_monitors_is_not_visible() {
        let cfg = desk();
        let mut s = State::initial(&cfg, Machine::Mac);
        s.on.pc = true;
        assert!(!s.is_visible(&cfg, Machine::Pc));
        s.owner.insert("m34".into(), Machine::Pc);
        assert!(s.is_visible(&cfg, Machine::Pc));
    }

    #[test]
    fn the_local_machine_starts_on_and_the_other_off() {
        let cfg = desk();
        for me in [Machine::Mac, Machine::Pc] {
            let s = State::initial(&cfg, me);
            assert!(s.is_on(me));
            assert!(!s.is_on(me.other()));
        }
    }

    #[test]
    fn a_mac_that_is_off_is_not_visible() {
        let cfg = desk();
        let mut s = State::initial(&cfg, Machine::Pc);
        assert!(!s.is_visible(&cfg, Machine::Mac));
        s.on.mac = true;
        assert!(s.is_visible(&cfg, Machine::Mac));
    }

    #[test]
    fn a_state_saved_with_pc_on_still_loads() {
        let s: State =
            serde_json::from_str(r#"{"owner":{"m34":"pc"},"pc_on":true,"builtin_pref":"mirror"}"#)
                .unwrap();
        assert_eq!(s.on, Powered::default());
        assert_eq!(s.builtin_pref, BuiltinDisplay::Mirror);
    }
}
