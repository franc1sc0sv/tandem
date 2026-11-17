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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_core::{Abilities, Machine, MachineInfo, Os};
use tandem_proto::{Capabilities, InputCaps, InputStatus, ModuleInfo, PowerCaps, ProcessInfo};

use crate::config::AgentConfig;

/// Cuánto se juntan los cambios de capacidades antes de anunciarlos a la otra máquina
/// (T-098): un permiso concedido o un módulo que arranca cambian varias cosas seguidas.
pub const ANNOUNCE_DEBOUNCE: Duration = Duration::from_millis(250);

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

/// La energía de una máquina con `os` y esta configuración.
pub fn power(cfg: &AgentConfig, os: Os) -> PowerCaps {
    PowerCaps {
        wake_peer: cfg.power.wol.is_some(),
        wakeable: cfg.power.wakeable,
        power_off: powers_off(os),
    }
}

/// Lo que esta máquina (el lado `me`, que es `info`) anuncia, con sus módulos ya armados
/// ([`crate::modules::Registry::snapshot`]).
pub fn local(
    cfg: &AgentConfig,
    me: Machine,
    info: MachineInfo,
    input: &InputStatus,
    process: &ProcessInfo,
    modules: Vec<ModuleInfo>,
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
        power: power(cfg, os),
        services: Vec::new(),
        modules,
        // Hasta T-105 (entornos) y T-110 (monitores) no hay nada que anunciar.
        environments: Vec::new(),
        monitors: Vec::new(),
    }
}

/// Junta en un solo envío lo que pasa dentro de una ventana: el primer cambio la abre y al
/// cerrarse corre el último pedido. Así hay como mucho un anuncio por ventana y ninguno se
/// retrasa más que la ventana.
pub struct Debounce {
    window: Duration,
    pending: Arc<AtomicBool>,
    /// El último pedido de la ventana abierta: lleva lo que valía al pedirlo (por ejemplo,
    /// el enlace con la otra máquina), que puede no ser lo de cuando se abrió.
    last: Arc<Mutex<Option<Fire>>>,
}

type Fire = Box<dyn FnOnce() + Send>;

impl std::fmt::Debug for Debounce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Debounce")
            .field("window", &self.window)
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

impl Debounce {
    pub fn new(window: Duration) -> Self {
        Debounce {
            window,
            pending: Arc::new(AtomicBool::new(false)),
            last: Arc::new(Mutex::new(None)),
        }
    }

    /// Corre `fire` al cerrar la ventana, en lugar de lo pedido antes en la misma. Fuera de
    /// un runtime de tokio (nada que espere) corre enseguida.
    pub fn schedule(&self, fire: impl FnOnce() + Send + 'static) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            fire();
            return;
        };
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(Box::new(fire));
        if self.pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let (pending, last, window) = (self.pending.clone(), self.last.clone(), self.window);
        rt.spawn(async move {
            tokio::time::sleep(window).await;
            // Primero se cierra la ventana y después se toma el pedido: uno que llegue entre
            // medio abre otra ventana, y si este ya lo corrió, esa no encuentra nada.
            pending.store(false, Ordering::SeqCst);
            let fire = last.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(fire) = fire {
                fire();
            }
        });
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
            Vec::new(),
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
            Vec::new(),
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
            Vec::new(),
        );
        assert_eq!(c.hub, Some(true));
        assert!(c.input.capture && c.input.inject);
        input.missing = vec![tandem_proto::InputPermission::Accessibility];
        let c = local(
            &cfg,
            Machine::A,
            info,
            &input,
            &ProcessInfo::default(),
            Vec::new(),
        );
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
            Vec::new(),
        );
        assert_eq!(c.input.process, process);
        // El de verdad: esta misma prueba, con su pid y un ejecutable no vacío.
        let now = current_process();
        assert_eq!(now.pid, std::process::id());
        assert!(!now.exe.is_empty() && now.started > 0);
    }

    #[test]
    fn without_a_runtime_the_debounce_fires_at_once() {
        let fired = Arc::new(AtomicBool::new(false));
        let f = fired.clone();
        Debounce::new(ANNOUNCE_DEBOUNCE).schedule(move || f.store(true, Ordering::SeqCst));
        assert!(fired.load(Ordering::SeqCst));
    }

    /// Una ráfaga dentro de la ventana es un solo envío, al cerrarse; lo que llega después
    /// abre otra.
    #[tokio::test(start_paused = true)]
    async fn a_burst_inside_the_window_fires_once() {
        let d = Debounce::new(ANNOUNCE_DEBOUNCE);
        let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let schedule = |d: &Debounce| {
            let c = count.clone();
            d.schedule(move || {
                c.fetch_add(1, Ordering::SeqCst);
            });
        };
        for _ in 0..5 {
            schedule(&d);
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        assert_eq!(count.load(Ordering::SeqCst), 0, "la ventana sigue abierta");
        tokio::time::sleep(ANNOUNCE_DEBOUNCE).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        schedule(&d);
        tokio::time::sleep(ANNOUNCE_DEBOUNCE + Duration::from_millis(1)).await;
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    /// Al cerrarse la ventana corre el último pedido, no el que la abrió.
    #[tokio::test(start_paused = true)]
    async fn the_window_runs_the_last_request() {
        let d = Debounce::new(ANNOUNCE_DEBOUNCE);
        let ran = Arc::new(Mutex::new(Vec::new()));
        for i in 0..3 {
            let ran = ran.clone();
            d.schedule(move || ran.lock().unwrap().push(i));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(ANNOUNCE_DEBOUNCE).await;
        assert_eq!(*ran.lock().unwrap(), [2]);
    }

    #[test]
    fn modules_environments_and_monitors_are_announced() {
        let cfg = fixtures::desk();
        let modules =
            crate::modules::Registry::for_os(Os::MacOs).snapshot(&crate::modules::ModuleContext {
                os: Os::MacOs,
                input: &InputStatus::new(Machine::A),
                power: &power(&cfg, Os::MacOs),
                switches: &Default::default(),
            });
        let c = local(
            &cfg,
            Machine::A,
            MachineInfo::new(Machine::A, Os::MacOs, None),
            &InputStatus::new(Machine::A),
            &ProcessInfo::default(),
            modules.clone(),
        );
        assert_eq!(c.modules, modules);
        assert!(c.environments.is_empty() && c.monitors.is_empty());
        // El anuncio entero viaja tal cual.
        let back: Capabilities = tandem_proto::decode(&tandem_proto::encode(&c).unwrap()).unwrap();
        assert_eq!(back, c);
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
