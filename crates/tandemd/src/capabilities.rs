//! Lo que cada máquina tiene y sabe hacer (T-084, DESIGN §3.7).
//!
//! No hay una máquina central: cada agente arma su [`Capabilities`] con lo que ve (los
//! monitores, si puede cambiarles la entrada, la pantalla integrada, el hub, los
//! permisos, la energía) y se lo anuncia al otro al conectarse y cada vez que cambia.
//! Los planes salen de los dos anuncios ([`abilities`]), no del nombre de la máquina.
//!
//! Lo único que todavía depende del sistema es qué backends existen: hoy solo Windows
//! cambia entradas por DDC (dxva2; `ddc-macos` espera el spike S4) y solo la MacBook
//! tiene pantalla integrada. Eso vive aquí, en una función por capacidad, y el resto
//! del agente pregunta por la capacidad.

use tandem_core::{Abilities, Config, Machine, MonitorConfig};
use tandem_proto::{Capabilities, InputCaps, InputStatus, MonitorCaps, PowerCaps};

use crate::config::AgentConfig;

/// El nombre corto de la máquina, para los textos de la UI.
pub fn label(machine: Machine) -> &'static str {
    match machine {
        Machine::Mac => "Mac",
        Machine::Pc => "PC",
    }
}

/// Si esta máquina tiene con qué cambiar la entrada de los monitores (DDC por DisplayPort
/// o, sin perfil, pidiéndoselo al usuario). Hoy, la PC (DESIGN §8).
pub fn switches_inputs(machine: Machine) -> bool {
    machine == Machine::Pc
}

/// Si la máquina tiene pantalla integrada (la MacBook, DESIGN §8.1).
pub fn has_builtin(machine: Machine) -> bool {
    machine == Machine::Mac
}

/// Si la máquina sabe apagarse limpio a pedido de la otra. Hoy, la PC (`shutdown /s`).
pub fn powers_off(machine: Machine) -> bool {
    machine == Machine::Pc
}

/// `macos`, `windows` o `linux`.
pub fn os_name() -> &'static str {
    std::env::consts::OS
}

/// El nombre de la máquina en el sistema, si se puede saber sin llamar a nada.
pub fn host_name() -> Option<String> {
    ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|v| std::env::var(v).ok())
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
}

/// Un monitor externo que esta máquina detectó al arrancar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// `SAM7489`, si dio EDID.
    pub pnp_id: Option<String>,
    /// El modelo del EDID (`LC34G55T`), que es lo que suele ir en `edid_model` en un
    /// `tandem.toml` escrito a mano.
    pub model: Option<String>,
    pub name: String,
}

impl Seen {
    /// Si es ese monitor del puesto: su `edid_model` es el id PnP o el modelo.
    fn is(&self, m: &MonitorConfig) -> bool {
        m.edid_model.as_deref().is_some_and(|e| {
            [&self.pnp_id, &self.model]
                .into_iter()
                .flatten()
                .any(|k| k.eq_ignore_ascii_case(e))
        })
    }
}

/// Lo que esta máquina anuncia. `seen` son los monitores que detectó al arrancar
/// ([`crate::autoconfig::seen`]), atados al puesto por su id PnP o su modelo; sin detección (pruebas, Linux, un backend que falló) se
/// toman los del puesto.
///
/// Un monitor del puesto cableado a esta máquina que no se detectó se anuncia igual: con
/// la entrada de la otra puesta, el sistema puede no verlo, y es justo el que hay que
/// traer. Uno que no está cableado aquí no se puede cambiar desde aquí (DDC va por el
/// cable), así que solo se anuncia si se lo detectó, y sin cambio de entrada.
pub fn local(
    cfg: &AgentConfig,
    desk: &Config,
    seen: Option<&[Seen]>,
    input: &InputStatus,
) -> Capabilities {
    let me = cfg.machine;
    let switch = |m: Option<&MonitorConfig>| {
        switches_inputs(me) && m.is_some_and(|m| m.wiring.contains_key(&me))
    };
    let of_desk = |m: &MonitorConfig| MonitorCaps {
        monitor: Some(m.id.clone()),
        pnp_id: m.edid_model.clone(),
        name: m.label.clone(),
        switch: switch(Some(m)),
        ddc: switch(Some(m)) && m.ddc.is_some(),
    };
    let monitors = match seen {
        Some(seen) => {
            let mut bound = std::collections::BTreeSet::new();
            let mut out: Vec<MonitorCaps> = seen
                .iter()
                .map(|s| {
                    let m = desk
                        .monitors
                        .iter()
                        .find(|m| !bound.contains(&m.id) && s.is(m));
                    if let Some(m) = m {
                        bound.insert(m.id.clone());
                    }
                    MonitorCaps {
                        monitor: m.map(|m| m.id.clone()),
                        pnp_id: s.pnp_id.clone(),
                        name: s.name.clone(),
                        switch: switch(m),
                        ddc: switch(m) && m.is_some_and(|m| m.ddc.is_some()),
                    }
                })
                .collect();
            out.extend(
                desk.monitors
                    .iter()
                    .filter(|m| !bound.contains(&m.id) && m.wiring.contains_key(&me))
                    .map(of_desk),
            );
            out
        }
        None => desk.monitors.iter().map(of_desk).collect(),
    };
    let works = input.problem.is_none() && input.missing.is_empty();
    Capabilities {
        machine: me,
        name: host_name().unwrap_or_else(|| label(me).to_owned()),
        os: os_name().to_owned(),
        monitors,
        builtin: has_builtin(me),
        hub: input.hub_here,
        input: InputCaps {
            capture: works,
            inject: works,
            missing: input.missing.clone(),
        },
        power: PowerCaps {
            wake_peer: cfg.power.wol.is_some(),
            wakeable: cfg.power.wakeable,
            power_off: powers_off(me),
        },
        services: Vec::new(),
    }
}

/// Lo que el plan puede pedirle a cada máquina, visto desde `here`.
///
/// Mientras no se sepa nada de la otra (no se la vio desde que arrancó este agente), se
/// supone que puede lo que esta no: cambiar las entradas que esta no cambia y
/// despertarse, si esta tiene con qué despertarla (`[power.wol]`). Si se equivoca, el paso falla con su motivo y se deshace todo.
pub fn abilities(desk: &Config, here: &Capabilities, peer: Option<&Capabilities>) -> Abilities {
    let me = here.machine;
    let other = me.other();
    let can = |c: &Capabilities, id| {
        c.monitors
            .iter()
            .any(|m| m.switch && m.monitor.as_ref() == Some(id))
    };
    let switchers = desk
        .monitors
        .iter()
        .map(|m| {
            let mut who = Vec::new();
            if can(here, &m.id) {
                who.push(me);
            }
            let peer_can = match peer {
                Some(p) => can(p, &m.id),
                None => who.is_empty(),
            };
            if peer_can {
                who.push(other);
            }
            (m.id.clone(), who)
        })
        .collect();
    // Despertar es cosa de dos: una que manda el Wake-on-LAN y otra que se deja despertar.
    let mut wakeable = std::collections::BTreeSet::new();
    if here.power.wakeable && peer.is_none_or(|p| p.power.wake_peer) {
        wakeable.insert(me);
    }
    if here.power.wake_peer && peer.is_none_or(|p| p.power.wakeable) {
        wakeable.insert(other);
    }
    Abilities {
        switchers,
        wakeable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::fixtures;

    /// Las dos con `[power.wol]`, como en `docs/examples`.
    fn caps(machine: Machine) -> Capabilities {
        let mut cfg = fixtures::desk_with_wol();
        cfg.machine = machine;
        local(&cfg, &cfg.desk, None, &InputStatus::new(Machine::Mac))
    }

    fn without_wol(machine: Machine) -> Capabilities {
        let mut c = caps(machine);
        c.power.wake_peer = false;
        c
    }

    #[test]
    fn each_machine_announces_what_it_has() {
        let pc = caps(Machine::Pc);
        assert!(pc.monitors.iter().all(|m| m.switch));
        assert!(!pc.builtin);
        assert!(pc.power.power_off);
        assert!(pc.power.wake_peer);
        let mac = caps(Machine::Mac);
        assert!(mac.monitors.iter().all(|m| !m.switch));
        assert!(mac.builtin);
        assert!(!mac.power.power_off);
        assert_eq!(mac.os, std::env::consts::OS);
        let mut cfg = fixtures::desk();
        cfg.machine = Machine::Pc;
        let c = local(&cfg, &cfg.desk, None, &InputStatus::new(Machine::Pc));
        assert!(!c.power.wake_peer, "sin [power.wol] no despierta a nadie");
    }

    #[test]
    fn only_profiled_monitors_switch_by_ddc() {
        let pc = caps(Machine::Pc);
        let desk = fixtures::desk().desk;
        for m in &pc.monitors {
            let cfg = desk.monitor(m.monitor.as_ref().unwrap()).unwrap();
            assert_eq!(m.ddc, cfg.ddc.is_some(), "{}", cfg.id);
        }
    }

    #[test]
    fn seen_monitors_are_bound_to_the_desk_by_pnp_id() {
        let mut cfg = fixtures::desk();
        cfg.machine = Machine::Pc;
        let first = cfg.desk.monitors[0].clone();
        let pnp = first.edid_model.clone().unwrap().to_ascii_lowercase();
        let seen = [
            Seen {
                pnp_id: Some(pnp),
                model: None,
                name: "34".into(),
            },
            Seen {
                pnp_id: None,
                model: None,
                name: "sin EDID".into(),
            },
        ];
        let c = local(&cfg, &cfg.desk, Some(&seen), &InputStatus::new(Machine::Pc));
        assert_eq!(c.monitors[0].monitor, Some(first.id));
        assert!(c.monitors[0].switch && c.monitors[0].ddc);
        assert_eq!(c.monitors[1].monitor, None);
        assert!(
            !c.monitors[1].switch,
            "uno que no es del puesto no se cambia"
        );
        // El 32" no se detectó, pero está cableado aquí: con la entrada de la Mac puesta,
        // Windows puede no verlo, y hay que poder traerlo.
        assert_eq!(c.monitors[2].monitor, Some("m32".into()));
        assert!(c.monitors[2].switch);
        assert_eq!(c.monitors.len(), 3);
    }

    #[test]
    fn a_hand_written_edid_model_binds_by_the_edid_name() {
        let mut cfg = fixtures::desk();
        cfg.machine = Machine::Pc;
        // El fixture tiene `edid_model = "LC34G55T"`, el nombre del EDID, no el id PnP.
        let seen = [Seen {
            pnp_id: Some("SAM711A".into()),
            model: Some("LC34G55T".into()),
            name: "34\"".into(),
        }];
        let c = local(&cfg, &cfg.desk, Some(&seen), &InputStatus::new(Machine::Pc));
        assert_eq!(c.monitors[0].monitor, Some("m34".into()));
        assert_eq!(c.monitors[0].pnp_id.as_deref(), Some("SAM711A"));
        assert_eq!(
            c.monitors.len(),
            2,
            "sin duplicar el 34\": {:?}",
            c.monitors
        );
    }

    #[test]
    fn a_monitor_wired_only_to_the_other_is_not_announced_unless_seen() {
        let mut cfg = fixtures::desk();
        cfg.machine = Machine::Pc;
        let m32 = cfg
            .desk
            .monitors
            .iter_mut()
            .find(|m| m.id == "m32".into())
            .unwrap();
        m32.wiring.remove(&Machine::Pc);
        m32.edid_model = Some("SAM7489".into());
        let input = InputStatus::new(Machine::Pc);
        let c = local(&cfg, &cfg.desk, Some(&[]), &input);
        assert_eq!(c.monitors.len(), 1, "{:?}", c.monitors);
        assert_eq!(c.monitors[0].monitor, Some("m34".into()));
        // Detectado (le llega señal por otro lado) sigue sin cambiarse desde aquí.
        let seen = [Seen {
            pnp_id: Some("SAM7489".into()),
            model: None,
            name: "32".into(),
        }];
        let c = local(&cfg, &cfg.desk, Some(&seen), &input);
        let m32 = c
            .monitors
            .iter()
            .find(|m| m.monitor == Some("m32".into()))
            .unwrap();
        assert!(!m32.switch && !m32.ddc);
        // Sin detección, tampoco se dice que se lo pueda cambiar.
        let c = local(&cfg, &cfg.desk, None, &input);
        let m32 = c
            .monitors
            .iter()
            .find(|m| m.monitor == Some("m32".into()))
            .unwrap();
        assert!(!m32.switch);
    }

    #[test]
    fn the_plan_asks_whoever_can_from_either_side() {
        let desk = fixtures::desk().desk;
        let (mac, pc) = (caps(Machine::Mac), caps(Machine::Pc));
        for (here, peer) in [(&mac, &pc), (&pc, &mac)] {
            let a = abilities(&desk, here, Some(peer));
            assert!(a.switchers.values().all(|w| w == &[Machine::Pc]));
            assert!(a.can_wake(Machine::Pc) && a.can_wake(Machine::Mac));
        }
    }

    #[test]
    fn without_the_peer_it_is_assumed_to_do_what_this_one_cannot() {
        let desk = fixtures::desk().desk;
        let a = abilities(&desk, &caps(Machine::Mac), None);
        assert!(a.switchers.values().all(|w| w == &[Machine::Pc]));
        // La PC sí puede: no se supone que la Mac también.
        let a = abilities(&desk, &caps(Machine::Pc), None);
        assert!(a.switchers.values().all(|w| w == &[Machine::Pc]));
        assert!(a.can_wake(Machine::Mac));
    }

    #[test]
    fn a_peer_that_says_it_cannot_be_woken_is_not() {
        let desk = fixtures::desk().desk;
        let mut mac = caps(Machine::Mac);
        mac.power.wakeable = false;
        let a = abilities(&desk, &caps(Machine::Pc), Some(&mac));
        assert!(!a.can_wake(Machine::Mac));
    }

    /// Hallazgo del PR #56: la PC sin `[power.wol]` armaba `PowerOn { Mac }`, que fallaba
    /// siempre al ejecutarse en vez de rechazarse.
    #[test]
    fn without_wake_on_lan_here_the_other_cannot_be_woken() {
        let desk = fixtures::desk().desk;
        let pc = without_wol(Machine::Pc);
        let mac = caps(Machine::Mac);
        for peer in [Some(&mac), None] {
            let a = abilities(&desk, &pc, peer);
            assert!(!a.can_wake(Machine::Mac), "{peer:?}");
        }
        // Y desde la Mac: la PC sin WoL no la puede despertar a ella.
        let a = abilities(&desk, &mac, Some(&pc));
        assert!(!a.can_wake(Machine::Mac));
        assert!(a.can_wake(Machine::Pc));

        // El plan lo rechaza con su motivo (antes de tocar nada).
        let mut state = tandem_core::State::initial(&desk, Machine::Pc);
        let shared = tandem_core::Mode::Shared(
            [("m34".into(), Machine::Pc), ("m32".into(), Machine::Mac)].into(),
        );
        let a = abilities(&desk, &pc, Some(&mac));
        state.on.set(Machine::Mac, false);
        let e = tandem_core::Plan::to_mode(&desk, &state, &shared, &a).unwrap_err();
        assert_eq!(e, tandem_core::plan::PlanError::CannotWake(Machine::Mac));
    }
}
