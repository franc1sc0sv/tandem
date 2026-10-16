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

use tandem_core::{Abilities, Config, Machine};
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
    pub name: String,
}

/// Lo que esta máquina anuncia. `seen` son los monitores que detectó; sin detección
/// (pruebas, sistemas sin backend) se toman los del puesto.
pub fn local(
    cfg: &AgentConfig,
    desk: &Config,
    seen: Option<&[Seen]>,
    input: &InputStatus,
) -> Capabilities {
    let me = cfg.machine;
    let switch = switches_inputs(me);
    let monitors = match seen {
        Some(seen) => seen
            .iter()
            .map(|s| {
                let m = s.pnp_id.as_deref().and_then(|pnp| {
                    desk.monitors.iter().find(|m| {
                        m.edid_model
                            .as_deref()
                            .is_some_and(|e| e.eq_ignore_ascii_case(pnp))
                    })
                });
                MonitorCaps {
                    monitor: m.map(|m| m.id.clone()),
                    pnp_id: s.pnp_id.clone(),
                    name: s.name.clone(),
                    switch,
                    ddc: switch && m.is_some_and(|m| m.ddc.is_some()),
                }
            })
            .collect(),
        None => desk
            .monitors
            .iter()
            .map(|m| MonitorCaps {
                monitor: Some(m.id.clone()),
                pnp_id: m.edid_model.clone(),
                name: m.label.clone(),
                switch,
                ddc: switch && m.ddc.is_some(),
            })
            .collect(),
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
/// despertarse. Si se equivoca, el paso falla con su motivo y se deshace todo.
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
    let mut wakeable = std::collections::BTreeSet::new();
    if here.power.wakeable {
        wakeable.insert(me);
    }
    if peer.is_none_or(|p| p.power.wakeable) {
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

    fn caps(machine: Machine) -> Capabilities {
        let mut cfg = fixtures::desk();
        cfg.machine = machine;
        local(&cfg, &cfg.desk, None, &InputStatus::new(Machine::Mac))
    }

    #[test]
    fn each_machine_announces_what_it_has() {
        let pc = caps(Machine::Pc);
        assert!(pc.monitors.iter().all(|m| m.switch));
        assert!(!pc.builtin);
        assert!(pc.power.power_off);
        let mac = caps(Machine::Mac);
        assert!(mac.monitors.iter().all(|m| !m.switch));
        assert!(mac.builtin);
        assert!(!mac.power.power_off);
        assert_eq!(mac.os, std::env::consts::OS);
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
                name: "34".into(),
            },
            Seen {
                pnp_id: None,
                name: "sin EDID".into(),
            },
        ];
        let c = local(&cfg, &cfg.desk, Some(&seen), &InputStatus::new(Machine::Pc));
        assert_eq!(c.monitors[0].monitor, Some(first.id));
        assert_eq!(c.monitors[1].monitor, None);
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
}
