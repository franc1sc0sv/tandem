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

/// Estado del puesto según Tandem.
///
/// Los monitores no reportan su entrada de forma fiable, así que `owner`
/// es la fuente de verdad: se actualiza solo cuando un paso se aplicó con éxito.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// Máquina que cada monitor está mostrando.
    pub owner: BTreeMap<MonitorId, Machine>,
    pub pc_on: bool,
    /// Preferencia del usuario; la pantalla efectiva la da [`State::builtin_effective`].
    pub builtin_pref: BuiltinDisplay,
}

impl State {
    /// Todo en la Mac, con la PC apagada: el estado seguro de partida.
    pub fn initial(cfg: &Config) -> Self {
        State {
            owner: cfg
                .monitors
                .iter()
                .map(|m| (m.id.clone(), Machine::Mac))
                .collect(),
            pc_on: false,
            builtin_pref: BuiltinDisplay::default(),
        }
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

    /// Una máquina es visible si tiene algo donde mirar: un monitor o, en la Mac, su pantalla.
    pub fn is_visible(&self, cfg: &Config, machine: Machine) -> bool {
        match machine {
            Machine::Pc => self.pc_on && self.monitors_of(cfg, Machine::Pc).next().is_some(),
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
        let mut s = State::initial(&cfg);
        assert_eq!(s.builtin_effective(&cfg), BuiltinDisplay::Off);
        for o in s.owner.values_mut() {
            *o = Machine::Pc;
        }
        assert_eq!(s.builtin_effective(&cfg), BuiltinDisplay::Extend);
    }

    #[test]
    fn pc_without_monitors_is_not_visible() {
        let cfg = desk();
        let mut s = State::initial(&cfg);
        s.pc_on = true;
        assert!(!s.is_visible(&cfg, Machine::Pc));
        s.owner.insert("m34".into(), Machine::Pc);
        assert!(s.is_visible(&cfg, Machine::Pc));
    }
}
