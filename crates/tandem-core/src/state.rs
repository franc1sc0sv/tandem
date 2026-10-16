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

/// Por qué Tandem no sabe lo que muestra un monitor (T-085).
///
/// Los monitores no dicen su entrada (el 34" siempre lee `0x09`, el 32" no actualiza
/// `0x60` tras un cambio desde su menú) y cada sistema ve al monitor fantasma como activo,
/// así que nada lo detecta solo: lo confirma un cambio de entrada que Tandem mandó o la
/// respuesta del usuario.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Doubt {
    /// Nunca se supo: el dueño es el del estado inicial, inventado.
    Unknown,
    /// Lo último que se supo, de un arranque anterior del agente: desde entonces pudo
    /// cambiar a mano (otra sesión, el menú del monitor).
    Stale,
    /// El usuario dijo que no muestra nada: su entrada no es la de ninguna máquina
    /// encendida. No hay que preguntar más, pero el próximo cambio se la manda.
    Blank,
}

/// Estado del puesto según Tandem.
///
/// Los monitores no reportan su entrada de forma fiable, así que `owner`
/// es la fuente de verdad: se actualiza solo cuando un paso se aplicó con éxito o el
/// usuario dijo lo que muestra el monitor. Lo que no está confirmado va en `doubt`.
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
    /// Los monitores cuyo dueño no está confirmado (T-085). Un plan les manda la entrada
    /// aunque ya parezcan estar donde van: planear sobre un estado inventado deja el
    /// monitor donde estaba. Un estado guardado sin este campo se lee como confirmado; el
    /// agente igual lo marca [`Doubt::Stale`] al arrancar ([`State::forget`]).
    #[serde(default)]
    pub doubt: BTreeMap<MonitorId, Doubt>,
}

impl State {
    /// El estado de partida visto desde `me`: todo en la Mac, sin confirmar (T-085), y
    /// solo `me` encendida, porque de la otra todavía no se sabe nada.
    pub fn initial(cfg: &Config, me: Machine) -> Self {
        State {
            owner: cfg
                .monitors
                .iter()
                .map(|m| (m.id.clone(), Machine::Mac))
                .collect(),
            on: Powered::only(me),
            builtin_pref: BuiltinDisplay::default(),
            doubt: cfg
                .monitors
                .iter()
                .map(|m| (m.id.clone(), Doubt::Unknown))
                .collect(),
        }
    }

    /// Lo mismo, dando por sabido lo que muestra cada monitor. Para pruebas y para quien
    /// sí lo sabe (la simulación del puesto).
    pub fn confirmed(mut self) -> Self {
        self.doubt.clear();
        self
    }

    /// Si Tandem sabe lo que muestra cada monitor: nada pendiente de preguntar o aplicar.
    /// Un monitor que el usuario dijo que está en negro ([`Doubt::Blank`]) cuenta como
    /// sabido.
    pub fn is_confirmed(&self) -> bool {
        !self
            .doubt
            .values()
            .any(|d| matches!(d, Doubt::Unknown | Doubt::Stale))
    }

    /// Si a `monitor` hay que mandarle la entrada aunque ya parezca estar donde va.
    pub fn in_doubt(&self, monitor: &MonitorId) -> bool {
        self.doubt.contains_key(monitor)
    }

    /// Lo guardado en un arranque anterior: desde entonces todo pudo cambiar a mano, así
    /// que cada monitor queda sin confirmar. Lo que nunca se supo sigue sin saberse.
    pub fn forget(&mut self) {
        for id in self.owner.keys() {
            let d = self.doubt.entry(id.clone()).or_insert(Doubt::Stale);
            if *d == Doubt::Blank {
                *d = Doubt::Stale;
            }
        }
    }

    /// El usuario dice lo que muestra `monitor`: una máquina, o nada (`None`). No manda
    /// nada a ningún monitor; solo corrige lo que cree Tandem.
    pub fn record_shown(&mut self, monitor: &MonitorId, shows: Option<Machine>) {
        match shows {
            Some(machine) => {
                self.owner.insert(monitor.clone(), machine);
                self.doubt.remove(monitor);
            }
            None => {
                self.doubt.insert(monitor.clone(), Doubt::Blank);
            }
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
    fn the_initial_state_is_unconfirmed_until_each_monitor_is_known() {
        let cfg = desk();
        let mut s = State::initial(&cfg, Machine::Pc);
        assert!(!s.is_confirmed());
        assert!(s.in_doubt(&"m34".into()));
        s.record_shown(&"m34".into(), Some(Machine::Pc));
        assert_eq!(s.owner[&"m34".into()], Machine::Pc);
        assert!(!s.in_doubt(&"m34".into()));
        assert!(!s.is_confirmed(), "falta el 32\"");
        // «Nada / negro»: ya no se pregunta, pero el próximo cambio le manda la entrada.
        s.record_shown(&"m32".into(), None);
        assert!(s.is_confirmed());
        assert_eq!(s.doubt[&"m32".into()], Doubt::Blank);
        assert_eq!(s.owner[&"m32".into()], Machine::Mac);
    }

    #[test]
    fn a_restored_state_is_stale_but_never_known_stays_unknown() {
        let cfg = desk();
        let mut s = State::initial(&cfg, Machine::Mac);
        s.record_shown(&"m34".into(), Some(Machine::Pc));
        s.record_shown(&"m32".into(), None);
        s.forget();
        assert!(!s.is_confirmed());
        assert_eq!(s.doubt[&"m34".into()], Doubt::Stale);
        assert_eq!(s.doubt[&"m32".into()], Doubt::Stale);
        assert_eq!(s.owner[&"m34".into()], Machine::Pc, "lo último que se supo");
        let mut fresh = State::initial(&cfg, Machine::Mac);
        fresh.forget();
        assert_eq!(fresh.doubt[&"m34".into()], Doubt::Unknown);
    }

    #[test]
    fn a_state_saved_before_t085_loads_as_confirmed() {
        let s: State = serde_json::from_str(
            r#"{"owner":{"m34":"pc"},"on":{"mac":true,"pc":true},"builtin_pref":"off"}"#,
        )
        .unwrap();
        assert!(s.is_confirmed());
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
