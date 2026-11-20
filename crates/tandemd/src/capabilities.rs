//! What each machine has and knows how to do (T-084, DESIGN §3.7).
//!
//! There is no central machine: each agent builds its [`Capabilities`] with what it has (the
//! hub, the permissions, the power) and announces it to the other on connecting and every time it
//! changes. Plans come from the two announcements ([`abilities`]), not from the machine's
//! name.
//!
//! The only thing that depends on the system is which backends exist: today only Windows knows how to
//! shut itself down. That lives here, in one function per capability that asks for the [`Os`]
//! (never for the side, DECISIONS D2), and the rest of the agent asks for the capability.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_core::{Abilities, Machine, MachineInfo, Os};
use tandem_proto::{Capabilities, InputCaps, InputStatus, ModuleInfo, PowerCaps, ProcessInfo};

use crate::config::AgentConfig;

/// How long capability changes are gathered before announcing them to the other machine
/// (T-098): a granted permission or a module that starts changes several things in a row.
pub const ANNOUNCE_DEBOUNCE: Duration = Duration::from_millis(250);

/// Whether a machine with `os` knows how to shut down cleanly on the other's request. Today, Windows
/// (`shutdown /s`).
pub fn powers_off(os: Os) -> bool {
    os == Os::Windows
}

/// The machine's name on the system, if it can be known without calling anything.
pub fn host_name() -> Option<String> {
    ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|v| std::env::var(v).ok())
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
}

/// The process running now: macOS grants permissions to this exact binary, so the
/// UI shows it as is (T-090).
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

/// The power of a machine with `os` and this configuration.
pub fn power(cfg: &AgentConfig, os: Os) -> PowerCaps {
    PowerCaps {
        wake_peer: cfg.power.wol.is_some(),
        wakeable: cfg.power.wakeable,
        power_off: powers_off(os),
        mac: cfg.power.mac.map(|m| m.to_string()),
        wol_mac: cfg.power.wol.as_ref().map(|w| w.mac.to_string()),
    }
}

/// `power` with the `[power.wol]` that holds now (the one learned when pairing or the one the
/// UI wrote), which may not be the one from the configuration the agent started with.
pub fn with_wol(mut power: PowerCaps, wol: Option<&tandem_power::WolTarget>) -> PowerCaps {
    power.wake_peer = wol.is_some();
    power.wol_mac = wol.map(|w| w.mac.to_string());
    power
}

/// What this machine (side `me`, which is `info`) announces, with its modules already built
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
        // The environments and hardware are added by the agent with its last scan (T-105), and the
        // monitors with what its bus sees ([`crate::monitors::MonitorHub::local_channels`]).
        environments: Vec::new(),
        hardware: tandem_proto::Hardware::default(),
        monitors: Vec::new(),
        // Set by the agent with what its probe found (T-344, [`crate::local_network`]).
        local_network: None,
    }
}

/// Gathers into a single send what happens within a window: the first change opens it and when
/// it closes the last request runs. This way there is at most one announcement per window and none
/// is delayed more than the window.
pub struct Debounce {
    window: Duration,
    pending: Arc<AtomicBool>,
    /// The last request of the open window: it carries what held when requested (for example,
    /// the link to the other machine), which may not be what it was when it opened.
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

    /// Runs `fire` when the window closes, in place of what was requested earlier in it. Outside
    /// a tokio runtime (nothing to wait for) it runs right away.
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
            // First the window is closed and then the request is taken: one that arrives in
            // between opens another window, and if this one already ran it, that one finds nothing.
            pending.store(false, Ordering::SeqCst);
            let fire = last.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(fire) = fire {
                fire();
            }
        });
    }
}

/// What the plan can ask of each machine, seen from `here`.
///
/// While nothing is known about the other (it was not seen since this agent started), it is
/// assumed it lets itself be woken if this one has something to wake it with (`[power.wol]`). If
/// it is wrong, the step fails with its reason.
pub fn abilities(here: &Capabilities, peer: Option<&Capabilities>) -> Abilities {
    let me = here.machine;
    let other = me.other();
    // Waking is a two-party thing: one that sends the Wake-on-LAN and another that lets itself be woken.
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

    /// Both with `[power.wol]`, as in `docs/examples`.
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

    /// A with macOS and B with Windows, like the `docs/examples` desk.
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
        assert!(!c.power.wake_peer, "without [power.wol] it wakes nobody");
    }

    /// What each machine knows how to do comes from its system, not from its side (DECISIONS D2).
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
        // The real one: this very test, with its pid and a non-empty executable.
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

    /// A burst within the window is a single send, when it closes; what arrives afterwards
    /// opens another.
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
        assert_eq!(count.load(Ordering::SeqCst), 0, "the window is still open");
        tokio::time::sleep(ANNOUNCE_DEBOUNCE).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        schedule(&d);
        tokio::time::sleep(ANNOUNCE_DEBOUNCE + Duration::from_millis(1)).await;
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    /// When the window closes the last request runs, not the one that opened it.
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
        // The whole announcement travels as is.
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
        // Without knowing anything about the other, it is assumed it lets itself be woken.
        assert!(abilities(&caps(Machine::B), None).can_wake(Machine::A));
    }

    #[test]
    fn a_peer_that_says_it_cannot_be_woken_is_not() {
        let mut a = caps(Machine::A);
        a.power.wakeable = false;
        let ab = abilities(&caps(Machine::B), Some(&a));
        assert!(!ab.can_wake(Machine::A));
    }

    /// Finding from PR #56: a machine without `[power.wol]` built the other's `PowerOn`, which
    /// always failed on execution instead of being rejected.
    #[test]
    fn without_wake_on_lan_here_the_other_cannot_be_woken() {
        let b = without_wol(Machine::B);
        let a = caps(Machine::A);
        for peer in [Some(&a), None] {
            let ab = abilities(&b, peer);
            assert!(!ab.can_wake(Machine::A), "{peer:?}");
        }
        // And from A: B without WoL cannot wake it.
        let ab = abilities(&a, Some(&b));
        assert!(!ab.can_wake(Machine::A));
        assert!(ab.can_wake(Machine::B));

        // The plan rejects it with its reason (before touching anything).
        let state = tandem_core::State::initial(Machine::B);
        let ab = abilities(&b, Some(&a));
        let e = tandem_core::Plan::power_on(&state, Machine::A, &ab).unwrap_err();
        assert_eq!(e, tandem_core::plan::PlanError::CannotWake(Machine::A));
    }
}
