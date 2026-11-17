//! Lo que cada máquina tiene y sabe hacer (T-084, DESIGN §3.7).
//!
//! No hay una máquina central: cada agente arma su [`Capabilities`] con lo que tiene (el
//! hub, los permisos, la energía) y se lo anuncia al otro al conectarse y cada vez que
//! cambia. Los planes salen de los dos anuncios ([`abilities`]), no del nombre de la
//! máquina.
//!
//! Lo único que depende del sistema es qué backends existen: hoy solo Windows sabe
//! apagarse solo. Eso vive aquí, en una función por capacidad que pregunta por el [`Os`]
//! (nunca por el lado, DECISIONS D2), y el resto del agente pregunta por la capacidad.

use tandem_core::{Abilities, Machine, MachineInfo, Os};
use tandem_proto::{Capabilities, InputCaps, InputStatus, PowerCaps, ProcessInfo};

use crate::config::AgentConfig;

/// Si una máquina con `os` sabe apagarse limpio a pedido de la otra. Hoy, Windows
/// (`shutdown /s`).
pub fn powers_off(os: Os) -> bool {
    os == Os::Windows
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

/// Lo que esta máquina (el lado `me`, que es `info`) anuncia.
pub fn local(
    cfg: &AgentConfig,
    me: Machine,
    info: MachineInfo,
    input: &InputStatus,
    process: &ProcessInfo,
) -> Capabilities {
    let works = input.problem.is_none() && input.missing.is_empty();
    let os = info.os;
    Capabilities {
        machine: me,
        info,
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
            power_off: powers_off(os),
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
    fn caps_of(machine: Machine, os: Os) -> Capabilities {
        let cfg = fixtures::desk_with_wol();
        local(
            &cfg,
            machine,
            MachineInfo::new(machine, os, None),
            &InputStatus::new(Machine::A),
            &ProcessInfo::default(),
        )
    }

    /// A con macOS y B con Windows, como el puesto de `docs/examples`.
    fn caps(machine: Machine) -> Capabilities {
        let os = match machine {
            Machine::A => Os::MacOs,
            Machine::B => Os::Windows,
        };
        caps_of(machine, os)
    }

    fn without_wol(machine: Machine) -> Capabilities {
        let mut c = caps(machine);
        c.power.wake_peer = false;
        c
    }

    #[test]
    fn each_machine_announces_what_it_has() {
        let pc = caps(Machine::B);
        assert!(pc.power.power_off);
        assert!(pc.power.wake_peer);
        assert_eq!(pc.info.os, Os::Windows);
        let mac = caps(Machine::A);
        assert!(!mac.power.power_off);
        assert_eq!(mac.info.name, "Mac");
        let cfg = fixtures::desk();
        let c = local(
            &cfg,
            Machine::B,
            MachineInfo::new(Machine::B, Os::Windows, None),
            &InputStatus::new(Machine::B),
            &ProcessInfo::default(),
        );
        assert!(!c.power.wake_peer, "sin [power.wol] no despierta a nadie");
    }

    /// Lo que sabe hacer cada máquina sale de su sistema, no de su lado (DECISIONS D2).
    #[test]
    fn powering_off_depends_on_the_system_not_the_side() {
        for side in Machine::ALL {
            assert!(caps_of(side, Os::Windows).power.power_off, "{side}");
            assert!(!caps_of(side, Os::MacOs).power.power_off, "{side}");
        }
    }

    #[test]
    fn the_input_engine_state_is_announced() {
        let cfg = fixtures::desk();
        let info = MachineInfo::new(Machine::A, Os::MacOs, None);
        let mut input = InputStatus::new(Machine::A);
        input.hub_here = Some(true);
        let c = local(
            &cfg,
            Machine::A,
            info.clone(),
            &input,
            &ProcessInfo::default(),
        );
        assert_eq!(c.hub, Some(true));
        assert!(c.input.capture && c.input.inject);
        input.missing = vec![tandem_proto::InputPermission::Accessibility];
        let c = local(&cfg, Machine::A, info, &input, &ProcessInfo::default());
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
        let c = local(
            &cfg,
            Machine::A,
            MachineInfo::new(Machine::A, Os::MacOs, None),
            &InputStatus::new(Machine::A),
            &process,
        );
        assert_eq!(c.input.process, process);
        // El de verdad: esta misma prueba, con su pid y un ejecutable no vacío.
        let now = current_process();
        assert_eq!(now.pid, std::process::id());
        assert!(!now.exe.is_empty() && now.started > 0);
    }

    #[test]
    fn both_can_be_woken_when_both_have_wake_on_lan() {
        let (a, b) = (caps(Machine::A), caps(Machine::B));
        for (here, peer) in [(&a, &b), (&b, &a)] {
            let ab = abilities(here, Some(peer));
            assert!(ab.can_wake(Machine::B) && ab.can_wake(Machine::A));
        }
        // Sin saber nada de la otra, se supone que se deja despertar.
        assert!(abilities(&caps(Machine::B), None).can_wake(Machine::A));
    }

    #[test]
    fn a_peer_that_says_it_cannot_be_woken_is_not() {
        let mut a = caps(Machine::A);
        a.power.wakeable = false;
        let ab = abilities(&caps(Machine::B), Some(&a));
        assert!(!ab.can_wake(Machine::A));
    }

    /// Hallazgo del PR #56: una máquina sin `[power.wol]` armaba `PowerOn` de la otra, que
    /// fallaba siempre al ejecutarse en vez de rechazarse.
    #[test]
    fn without_wake_on_lan_here_the_other_cannot_be_woken() {
        let b = without_wol(Machine::B);
        let a = caps(Machine::A);
        for peer in [Some(&a), None] {
            let ab = abilities(&b, peer);
            assert!(!ab.can_wake(Machine::A), "{peer:?}");
        }
        // Y desde A: B sin WoL no la puede despertar a ella.
        let ab = abilities(&a, Some(&b));
        assert!(!ab.can_wake(Machine::A));
        assert!(ab.can_wake(Machine::B));

        // El plan lo rechaza con su motivo (antes de tocar nada).
        let state = tandem_core::State::initial(Machine::B);
        let ab = abilities(&b, Some(&a));
        let e = tandem_core::Plan::power_on(&state, Machine::A, &ab).unwrap_err();
        assert_eq!(e, tandem_core::plan::PlanError::CannotWake(Machine::A));
    }
}
