//! Lo que cada máquina tiene y sabe hacer (T-084, DESIGN §3.7).
//!
//! No hay una máquina central: cada agente arma su [`Capabilities`] con lo que tiene (el
//! hub, los permisos, la energía) y se lo anuncia al otro al conectarse y cada vez que
//! cambia. Los planes salen de los dos anuncios ([`abilities`]), no del nombre de la
//! máquina.
//!
//! Lo único que todavía depende del sistema es qué backends existen: hoy solo Windows sabe
//! apagarse solo. Eso vive aquí, en una función por capacidad, y el resto del agente
//! pregunta por la capacidad.

use tandem_core::{Abilities, Machine};
use tandem_proto::{Capabilities, InputCaps, InputStatus, PowerCaps, ProcessInfo};

use crate::config::AgentConfig;

/// El nombre corto de la máquina, para los textos de la UI.
pub fn label(machine: Machine) -> &'static str {
    match machine {
        Machine::Mac => "Mac",
        Machine::Pc => "PC",
    }
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

/// El proceso que corre ahora: macOS concede los permisos a este binario exacto, así que la
/// UI lo muestra tal cual (T-090).
pub fn current_process() -> ProcessInfo {
    ProcessInfo {
        exe: std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        pid: std::process::id(),
        started: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    }
}

/// Lo que esta máquina anuncia.
pub fn local(cfg: &AgentConfig, input: &InputStatus, process: &ProcessInfo) -> Capabilities {
    let me = cfg.machine;
    let works = input.problem.is_none() && input.missing.is_empty();
    Capabilities {
        machine: me,
        name: host_name().unwrap_or_else(|| label(me).to_owned()),
        os: os_name().to_owned(),
        hub: input.hub_here,
        input: InputCaps {
            capture: works,
            inject: works,
            missing: input.missing.clone(),
            process: process.clone(),
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
/// supone que se deja despertar si esta tiene con qué despertarla (`[power.wol]`). Si se
/// equivoca, el paso falla con su motivo.
pub fn abilities(here: &Capabilities, peer: Option<&Capabilities>) -> Abilities {
    let me = here.machine;
    let other = me.other();
    // Despertar es cosa de dos: una que manda el Wake-on-LAN y otra que se deja despertar.
    let mut wakeable = std::collections::BTreeSet::new();
    if here.power.wakeable && peer.is_none_or(|p| p.power.wake_peer) {
        wakeable.insert(me);
    }
    if here.power.wake_peer && peer.is_none_or(|p| p.power.wakeable) {
        wakeable.insert(other);
    }
    Abilities { wakeable }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::fixtures;

    /// Las dos con `[power.wol]`, como en `docs/examples`.
    fn caps(machine: Machine) -> Capabilities {
        let mut cfg = fixtures::desk_with_wol();
        cfg.machine = machine;
        local(
            &cfg,
            &InputStatus::new(Machine::Mac),
            &ProcessInfo::default(),
        )
    }

    fn without_wol(machine: Machine) -> Capabilities {
        let mut c = caps(machine);
        c.power.wake_peer = false;
        c
    }

    #[test]
    fn each_machine_announces_what_it_has() {
        let pc = caps(Machine::Pc);
        assert!(pc.power.power_off);
        assert!(pc.power.wake_peer);
        let mac = caps(Machine::Mac);
        assert!(!mac.power.power_off);
        assert_eq!(mac.os, std::env::consts::OS);
        let mut cfg = fixtures::desk();
        cfg.machine = Machine::Pc;
        let c = local(
            &cfg,
            &InputStatus::new(Machine::Pc),
            &ProcessInfo::default(),
        );
        assert!(!c.power.wake_peer, "sin [power.wol] no despierta a nadie");
    }

    #[test]
    fn the_input_engine_state_is_announced() {
        let cfg = fixtures::desk();
        let mut input = InputStatus::new(Machine::Mac);
        input.hub_here = Some(true);
        let c = local(&cfg, &input, &ProcessInfo::default());
        assert_eq!(c.hub, Some(true));
        assert!(c.input.capture && c.input.inject);
        input.missing = vec![tandem_proto::InputPermission::Accessibility];
        let c = local(&cfg, &input, &ProcessInfo::default());
        assert!(!c.input.capture && !c.input.inject);
        assert_eq!(c.input.missing, input.missing);
    }

    #[test]
    fn the_running_process_is_announced_with_the_input_caps() {
        let cfg = fixtures::desk();
        let process = ProcessInfo {
            exe: "/Applications/Tandem.app/Contents/MacOS/tandemd".into(),
            pid: 77,
            started: 5,
        };
        let c = local(&cfg, &InputStatus::new(Machine::Mac), &process);
        assert_eq!(c.input.process, process);
        // El de verdad: esta misma prueba, con su pid y un ejecutable no vacío.
        let now = current_process();
        assert_eq!(now.pid, std::process::id());
        assert!(!now.exe.is_empty() && now.started > 0);
    }

    #[test]
    fn both_can_be_woken_when_both_have_wake_on_lan() {
        let (mac, pc) = (caps(Machine::Mac), caps(Machine::Pc));
        for (here, peer) in [(&mac, &pc), (&pc, &mac)] {
            let a = abilities(here, Some(peer));
            assert!(a.can_wake(Machine::Pc) && a.can_wake(Machine::Mac));
        }
        // Sin saber nada de la otra, se supone que se deja despertar.
        assert!(abilities(&caps(Machine::Pc), None).can_wake(Machine::Mac));
    }

    #[test]
    fn a_peer_that_says_it_cannot_be_woken_is_not() {
        let mut mac = caps(Machine::Mac);
        mac.power.wakeable = false;
        let a = abilities(&caps(Machine::Pc), Some(&mac));
        assert!(!a.can_wake(Machine::Mac));
    }

    /// Hallazgo del PR #56: la PC sin `[power.wol]` armaba `PowerOn { Mac }`, que fallaba
    /// siempre al ejecutarse en vez de rechazarse.
    #[test]
    fn without_wake_on_lan_here_the_other_cannot_be_woken() {
        let pc = without_wol(Machine::Pc);
        let mac = caps(Machine::Mac);
        for peer in [Some(&mac), None] {
            let a = abilities(&pc, peer);
            assert!(!a.can_wake(Machine::Mac), "{peer:?}");
        }
        // Y desde la Mac: la PC sin WoL no la puede despertar a ella.
        let a = abilities(&mac, Some(&pc));
        assert!(!a.can_wake(Machine::Mac));
        assert!(a.can_wake(Machine::Pc));

        // El plan lo rechaza con su motivo (antes de tocar nada).
        let state = tandem_core::State::initial(Machine::Pc);
        let a = abilities(&pc, Some(&mac));
        let e = tandem_core::Plan::power_on(&state, Machine::Mac, &a).unwrap_err();
        assert_eq!(e, tandem_core::plan::PlanError::CannotWake(Machine::Mac));
    }
}
