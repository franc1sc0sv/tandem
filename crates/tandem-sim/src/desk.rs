//! The whole desk: two real `tandemd` in the same process, joined by the simulated
//! network, with simulated backends and the faults of a [`FaultPlan`].

use std::sync::Arc;
use std::time::Duration;

use tandem_core::{Machine, MonitorInput, Os, Plan, State};
use tandem_link::iface::{FakeInterfaces, InterfaceKind, NetInterface, follow_wired};
use tandem_link::manager::{HealthConfig, LinkKind, LinkManager};
use tandem_link::metrics;
use tandem_monitor::{LocalConfig, LocalMonitors};
use tandem_proto::{MonitorView, Reply, TxnEvent, UiCommand};
use tandemd::monitors::MonitorTimes;
use tandemd::{Agent, AgentConfig, NoPeer, StateStore};
use tokio::sync::{broadcast, watch};

use crate::fault::{FaultPlan, Faults};
use crate::monitors::{MonitorSpec, SimMonitors};
use crate::net::{ManagedPeer, SimNet, SimProber};
use crate::scenes::SceneScreens;
use crate::steps::{HardwareTimes, SimSteps};
use crate::world::{Power, World};

/// The network card the (simulated) Wake-on-LAN of each agent goes to.
const WOL_MAC: &str = "aa:bb:cc:dd:ee:ff";

/// Wait cap for anything in the simulation, in (paused) tokio time.
pub const SIM_LIMIT: Duration = Duration::from_secs(3600);

/// Time for whatever is left in the air to finish (shutdown, link switch, notices).
pub const SETTLE: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum SimError {
    #[error("the simulation could not be set up: {0}")]
    Setup(String),
    #[error("the agent rejected the request: {0}")]
    Rejected(String),
    #[error("the maximum time was exceeded waiting for {0}")]
    Timeout(&'static str),
    #[error("the desk does not match what was expected:\n  {}", .0.join("\n  "))]
    Mismatch(Vec<String>),
}

/// How a transaction ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Committed,
    Failed,
}

/// The notices of a transaction, from `Started` to the outcome.
#[derive(Debug, Clone)]
pub struct TxnRun {
    pub events: Vec<TxnEvent>,
}

impl TxnRun {
    pub fn outcome(&self) -> Option<Outcome> {
        match self.events.last()? {
            TxnEvent::Committed { .. } => Some(Outcome::Committed),
            TxnEvent::Failed { .. } => Some(Outcome::Failed),
            _ => None,
        }
    }

    /// Index and reason of the step that failed, if any.
    pub fn failure(&self) -> Option<(u32, &str)> {
        self.events.iter().find_map(|e| match e {
            TxnEvent::StepFailed { index, error, .. } => Some((*index, error.as_str())),
            _ => None,
        })
    }
}

fn is_end(e: &TxnEvent) -> Option<u64> {
    match e {
        TxnEvent::Committed { txn, .. } | TxnEvent::Failed { txn, .. } => Some(*txn),
        _ => None,
    }
}

/// Desk options.
#[derive(Debug, Clone)]
pub struct DeskBuilder {
    links: Vec<LinkKind>,
    times: HardwareTimes,
    primary: Machine,
    /// A's system and B's. Any combination works (DECISIONS D2).
    os: [Os; 2],
    /// The desk's monitors and who has a cable to each one (T-110).
    monitors: Vec<MonitorSpec>,
    scene_screens: SceneScreens,
    /// What each side's `tandem.toml` says besides the machine and Wake-on-LAN (A's and B's).
    toml: [String; 2],
    /// Each DDC request takes its cable's latency on the desk clock too (T-342).
    ddc_takes_time: bool,
}

impl Default for DeskBuilder {
    fn default() -> Self {
        DeskBuilder {
            links: LinkKind::ALL.to_vec(),
            times: HardwareTimes::default(),
            primary: Machine::A,
            os: [Os::MacOs, Os::Windows],
            monitors: Vec::new(),
            scene_screens: SceneScreens::default(),
            toml: Default::default(),
            ddc_takes_time: false,
        }
    }
}

/// How often the simulation's agents look at the bus.
pub const SIM_MONITOR_TIMES: MonitorTimes = MonitorTimes {
    scan_every: Duration::from_secs(2),
    poll_every: Duration::from_millis(500),
};

impl DeskBuilder {
    /// The wired links (all three by default).
    pub fn links(mut self, links: &[LinkKind]) -> Self {
        self.links = links.to_vec();
        self
    }

    pub fn times(mut self, times: HardwareTimes) -> Self {
        self.times = times;
        self
    }

    /// The machine the user is using at the start: the other one is off (by
    /// default the Mac; with the PC, the mirror scenarios of DESIGN §3.7).
    pub fn primary(mut self, primary: Machine) -> Self {
        self.primary = primary;
        self
    }

    /// Each side's system (by default A with macOS and B with Windows).
    pub fn os(mut self, a: Os, b: Os) -> Self {
        self.os = [a, b];
        self
    }

    /// Monitors connected to the machines (by default none: the module is left without a channel).
    /// The scene monitors (the stand-in for T-100's abstract step) and what each one shows
    /// at the start.
    pub fn scene_monitors(mut self, monitors: &[(&str, Machine)]) -> Self {
        self.scene_screens = SceneScreens::new(monitors);
        self
    }

    /// What `machine`'s `tandem.toml` carries at the start (its desk, its version), as written
    /// in the file: top-level keys first, then tables.
    pub fn toml(mut self, machine: Machine, toml: &str) -> Self {
        self.toml[usize::from(machine == Machine::B)] = toml.to_owned();
        self
    }

    pub fn monitors(mut self, specs: Vec<MonitorSpec>) -> Self {
        self.monitors = specs;
        self
    }

    /// The monitors' DDC requests take their time on the desk clock, one at a time per machine,
    /// as on a real bus: the latency of each cable and the firmware's wait before its verdict. By
    /// default they take time only on the monitors' clock, and tokio's does not move (T-342).
    pub fn ddc_takes_time(mut self) -> Self {
        self.ddc_takes_time = true;
        self
    }

    /// Starts the two agents with the main one on and the other off. Call inside
    /// a single-threaded tokio runtime, ideally with the clock paused.
    pub async fn build(self) -> Result<Desk, SimError> {
        let dir = tempfile::tempdir().map_err(|e| SimError::Setup(e.to_string()))?;
        // Both with `[power.wol]` to wake the other: Wake-on-LAN is simulated.
        let os = self.os;
        let toml = &self.toml;
        let agent_cfg = |machine: Machine| {
            let extra = &toml[usize::from(machine == Machine::B)];
            let mut cfg = AgentConfig::from_toml(&format!(
                "machine = \"{}\"\n{extra}\n[power.wol]\nmac = \"{WOL_MAC}\"\n",
                machine.as_str()
            ))
            .map_err(|e| SimError::Setup(e.to_string()))?;
            cfg.os = match machine {
                Machine::A => os[0],
                Machine::B => os[1],
            };
            Ok::<_, SimError>(cfg)
        };
        let (mac_cfg, pc_cfg) = (agent_cfg(Machine::A)?, agent_cfg(Machine::B)?);
        let world = World::with_primary(self.primary);
        let net = SimNet::new(world.clone(), &self.links);
        let faults = Faults::default();

        let (mac_up_tx, mac_up) = watch::channel(false);
        let (pc_up_tx, pc_up) = watch::channel(false);
        let agent = |agent_cfg: AgentConfig, up| {
            let machine = agent_cfg.machine;
            let steps = SimSteps::new(
                machine,
                world.clone(),
                net.clone(),
                faults.clone(),
                self.times,
                up,
            );
            let store = StateStore::new(dir.path().join(format!("{machine}.json")));
            Arc::new(Agent::with_local_steps(agent_cfg, store, Arc::new(steps)))
        };
        let mac = agent(mac_cfg, mac_up);
        let pc = agent(pc_cfg, pc_up);
        // Each agent with its real link manager, which pings the other over each cable.
        // Like the real network: every time a link appears, the agent introduces itself
        // again (its desk and its capabilities) over it.
        let links =
            |near: &Arc<Agent>, far: &Arc<Agent>, up: watch::Sender<bool>| -> Result<_, SimError> {
                let (manager, events) = LinkManager::spawn(
                    &net.candidates(),
                    SimProber::new(net.clone(), far.clone()),
                    HealthConfig::default(),
                )
                .map_err(|e| SimError::Setup(e.to_string()))?;
                // The Network screen metrics, as in `tandemd::network` (T-102).
                tokio::spawn(metrics::follow(
                    near.net_metrics().clone(),
                    manager.watch(),
                    events,
                    near.link_qos().clone(),
                    metrics::SAMPLE_EVERY,
                ));
                let mut snapshots = manager.watch();
                let peer = Arc::new(ManagedPeer::new(manager));
                let (near, announce) = (near.clone(), peer.clone());
                tokio::spawn(async move {
                    let mut was = false;
                    loop {
                        let active = snapshots.borrow_and_update().active.is_some();
                        up.send_replace(active);
                        if active && !was {
                            near.set_peer(announce.clone());
                        }
                        was = active;
                        if snapshots.changed().await.is_err() {
                            return;
                        }
                    }
                });
                Ok(peer)
            };
        // Each one reconciles the other's power with what its link manager sees.
        let (mac_reach, pc_reach) = (mac_up_tx.subscribe(), pc_up_tx.subscribe());
        let mac_links = links(&mac, &pc, mac_up_tx)?;
        let pc_links = links(&pc, &mac, pc_up_tx)?;
        mac.set_peer(mac_links.clone());
        pc.set_peer(pc_links.clone());
        // Each machine's interfaces: cables plugged in later (T-094) change them
        // and the link manager follows them, as in the real agent.
        let cables = [FakeInterfaces::new(vec![]), FakeInterfaces::new(vec![])];
        let follow = |ifaces: &FakeInterfaces, links: &ManagedPeer| {
            follow_wired(ifaces, links.manager().control(), |i: &NetInterface| {
                Some(crate::net::link_addr(i.kind.link_kind()))
            })
        };
        let mut watches = vec![
            follow(&cables[0], &mac_links),
            follow(&cables[1], &pc_links),
            tokio::spawn(watch_while_on(world.clone(), mac.clone(), mac_reach)),
            tokio::spawn(watch_while_on(world.clone(), pc.clone(), pc_reach)),
        ];
        // With real monitors (the `monitor-ddc` doubles), scenes go to them.
        if self.monitors.is_empty() {
            for agent in [&mac, &pc] {
                let backend = self.scene_screens.backend(agent.machine(), faults.clone());
                agent.set_scene_backend(Arc::new(backend));
            }
        }
        let monitors = (!self.monitors.is_empty()).then(|| SimMonitors::new(&self.monitors));
        if let Some(sim) = &monitors {
            for agent in [&mac, &pc] {
                let clock = sim.clock();
                let wall = clock.clone();
                let core = LocalMonitors::new(
                    Arc::new(sim.bus_of(agent.machine(), world.clone())),
                    Arc::new(clock),
                    // A fixed time plus the double's clock: event times are exact.
                    Arc::new(move || {
                        use monitor_ddc::Clock;
                        1_763_000_000_000 + u64::try_from(wall.now().as_millis()).unwrap_or(0)
                    }),
                    LocalConfig::default(),
                );
                if self.ddc_takes_time {
                    agent.monitor_hub().charge_bus_time(Arc::new(sim.clock()));
                }
                // The fake bus does not block: it is called in place, with the paused clock standing still.
                watches.push(agent.start_monitors(core, SIM_MONITOR_TIMES, true));
            }
        }

        Ok(Desk {
            mac,
            pc,
            world,
            net,
            cables,
            mac_links,
            pc_links,
            faults,
            times: self.times,
            primary: self.primary,
            watches,
            monitors,
            scene_screens: self.scene_screens,
            _dir: dir,
        })
    }
}

/// Two agents in the same process over the simulated desk.
pub struct Desk {
    pub mac: Arc<Agent>,
    pub pc: Arc<Agent>,
    pub world: World,
    pub net: SimNet,
    cables: [FakeInterfaces; 2],
    mac_links: Arc<ManagedPeer>,
    pc_links: Arc<ManagedPeer>,
    faults: Faults,
    times: HardwareTimes,
    primary: Machine,
    watches: Vec<tokio::task::JoinHandle<()>>,
    monitors: Option<SimMonitors>,
    scene_screens: SceneScreens,
    _dir: tempfile::TempDir,
}

impl Drop for Desk {
    fn drop(&mut self) {
        for w in &self.watches {
            w.abort();
        }
        // Breaks the cycle agent → link manager → the other side's agent.
        self.mac.set_peer(Arc::new(NoPeer));
        self.pc.set_peer(Arc::new(NoPeer));
    }
}

/// The watch over `agent`'s pair while its machine is on: an off one does not
/// run anything, and on coming back it starts again with what it had saved.
async fn watch_while_on(world: World, agent: Arc<Agent>, reach: watch::Receiver<bool>) {
    let me = agent.machine();
    let mut power = world.subscribe_power();
    loop {
        if power
            .wait_for(|p| p.get(&me) == Some(&Power::On))
            .await
            .is_err()
        {
            return;
        }
        let task = tokio::spawn(tandemd::peer_watch::watch(agent.clone(), reach.clone()));
        let off = power.wait_for(|p| p.get(&me) != Some(&Power::On)).await;
        task.abort();
        if off.is_err() {
            return;
        }
    }
}

async fn within<F: Future>(what: &'static str, f: F) -> Result<F::Output, SimError> {
    tokio::time::timeout(SIM_LIMIT, f)
        .await
        .map_err(|_| SimError::Timeout(what))
}

/// The interface each machine sees when the `kind` cable is plugged in.
fn cable_interface(kind: LinkKind) -> NetInterface {
    let (name, iface_kind) = match kind {
        LinkKind::Usb4 => ("USB4 P2P Network Adapter", InterfaceKind::Usb4),
        LinkKind::Ethernet => ("Ethernet", InterfaceKind::Ethernet),
        LinkKind::Wifi => ("Wi-Fi", InterfaceKind::Wifi),
    };
    NetInterface {
        index: 100 + u32::from(kind.priority()),
        name: name.into(),
        kind: iface_kind,
        addrs: vec![],
        mac: None,
        speed_mbps: None,
        up: true,
    }
}

impl Desk {
    /// The simulated interfaces of a machine, to plug and unplug by hand.
    pub fn interfaces(&self, machine: Machine) -> &FakeInterfaces {
        &self.cables[usize::from(machine == Machine::B)]
    }

    /// Plugs the `kind` cable between the two machines: the link exists and the interface
    /// appears on both, which notice by themselves.
    pub fn plug_cable(&self, kind: LinkKind) {
        self.net.plug(kind);
        for ifaces in &self.cables {
            ifaces.plug(cable_interface(kind));
        }
    }

    /// Unplugs the `kind` cable: the interface goes down on both machines and nothing passes through it.
    pub fn unplug_cable(&self, kind: LinkKind) {
        let index = cable_interface(kind).index;
        for ifaces in &self.cables {
            ifaces.unplug(index);
        }
        self.net.cut(kind);
    }

    pub fn builder() -> DeskBuilder {
        DeskBuilder::default()
    }

    /// The desk with the default options: three links, the Mac on, the PC off.
    pub async fn new() -> Result<Desk, SimError> {
        DeskBuilder::default().build().await
    }

    pub fn agent(&self, machine: Machine) -> &Arc<Agent> {
        match machine {
            Machine::A => &self.mac,
            Machine::B => &self.pc,
        }
    }

    /// The link `machine` now uses to talk to the other agent.
    pub fn active_link(&self, machine: Machine) -> Option<LinkKind> {
        match machine {
            Machine::A => self.mac_links.active_link(),
            Machine::B => self.pc_links.active_link(),
        }
    }

    /// Replaces the pending faults and schedules the plan's link drops.
    pub fn arm(&self, plan: FaultPlan) {
        self.faults.arm(plan.faults);
        for (after, kind) in plan.cuts {
            let net = self.net.clone();
            tokio::spawn(async move {
                tokio::time::sleep(after).await;
                net.cut(kind);
            });
        }
    }

    /// Power `machine` on (or off) from the UI of `on`, through the same path as the UI, and
    /// wait for the outcome.
    pub async fn set_power(
        &self,
        on: Machine,
        machine: Machine,
        up: bool,
    ) -> Result<TxnRun, SimError> {
        self.request(on, UiCommand::SetPower { machine, on: up })
            .await
    }

    /// Apply scene `id` from the UI of `on` and wait for the outcome.
    pub async fn apply_scene(&self, on: Machine, id: &str) -> Result<TxnRun, SimError> {
        self.request(on, UiCommand::ApplyScene { id: id.to_owned() })
            .await
    }

    /// The scene monitors ([`DeskBuilder::scene_monitors`]).
    pub fn scene_screens(&self) -> &SceneScreens {
        &self.scene_screens
    }

    /// Power `machine` on from the other one.
    pub async fn power_on(&self, machine: Machine) -> Result<TxnRun, SimError> {
        self.set_power(machine.other(), machine, true).await
    }

    /// A UI request that starts a transaction; waits for its outcome.
    async fn request(&self, on: Machine, cmd: UiCommand) -> Result<TxnRun, SimError> {
        let agent = self.agent(on);
        let mut rx = agent.subscribe_txns();
        let txn = match agent.handle(cmd) {
            Reply::Accepted { txn } => txn,
            Reply::Rejected(why) => return Err(SimError::Rejected(why)),
            other => return Err(SimError::Rejected(format!("{other:?}"))),
        };
        let mut events = Vec::new();
        loop {
            let e = match within("the transaction outcome", rx.recv()).await? {
                Ok(e) => e,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    return Err(SimError::Setup(format!("{n} notices were lost")));
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(SimError::Setup("the orchestrator closed".into()));
                }
            };
            let end = is_end(&e) == Some(txn);
            events.push(e);
            if end {
                return Ok(TxnRun { events });
            }
        }
    }

    /// The machine the user was using at the start.
    pub fn primary(&self) -> Machine {
        self.primary
    }

    /// The physical monitors, if the desk has any ([`DeskBuilder::monitors`]).
    pub fn monitors(&self) -> Option<&SimMonitors> {
        self.monitors.as_ref()
    }

    /// Change a monitor's input from the UI of `on` and wait for the outcome.
    pub async fn switch_monitor(
        &self,
        on: Machine,
        monitor: &str,
        input: MonitorInput,
    ) -> Result<TxnRun, SimError> {
        let cmd = UiCommand::SwitchMonitor {
            monitor: monitor.to_owned(),
            input,
        };
        self.request(on, cmd).await
    }

    /// Waits for the monitors `machine` sees to satisfy `ok`; returns that view.
    pub async fn wait_monitors(
        &self,
        machine: Machine,
        what: &'static str,
        ok: impl Fn(&[MonitorView]) -> bool,
    ) -> Result<Vec<MonitorView>, SimError> {
        let mut rx = self.agent(machine).subscribe_monitors();
        let views = within(what, rx.wait_for(|v| ok(v)))
            .await?
            .map_err(|_| SimError::Setup("the agent closed".into()))?
            .clone();
        Ok(views)
    }

    /// PC shutdown orchestrated by the Mac (DESIGN §5).
    pub async fn shutdown_pc(&self) -> Result<TxnRun, SimError> {
        self.shutdown(Machine::A, Machine::B).await
    }

    /// Clean shutdown of `machine` orchestrated by `on`.
    ///
    /// The plan is built and run in the orchestrator the way the agent would with
    /// [`UiCommand::SetPower`], to get the transaction's notices even if the machine
    /// doing the orchestrating is the one shutting down.
    pub async fn shutdown(&self, on: Machine, machine: Machine) -> Result<TxnRun, SimError> {
        let agent = self.agent(on);
        let orch = agent.orchestrator();
        let mut rx = agent.subscribe_txns();
        let txn = orch
            .try_begin()
            .ok_or_else(|| SimError::Rejected(tandemd::agent::BUSY.into()))?;
        let mut state = agent.state();
        let plan = Plan::power_off(&state, machine);
        let outcome = within("the shutdown", orch.run(txn, &mut state, &plan)).await;
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                orch.end();
                return Err(e);
            }
        };
        if let Err(e) = agent.set_state(state.clone()) {
            tracing::warn!("{e}");
        }
        orch.end();
        orch.finish(txn, &state, &outcome);
        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        Ok(TxnRun { events })
    }

    /// The PC suddenly loses power.
    pub fn pull_pc_plug(&self) {
        self.pull_plug(Machine::B);
    }

    /// `machine` suddenly loses power (or suspends), with no agent
    /// asking for it: its agent stops answering.
    pub fn pull_plug(&self, machine: Machine) {
        self.world.lock().power_off(machine);
        self.net.drop_all();
    }

    /// Someone powers the PC on with its button, without Wake-on-LAN.
    pub fn press_pc_power_button(&self) {
        self.press_power_button(Machine::B);
    }

    /// Someone powers `machine` on (or wakes it) with its button, without Wake-on-LAN.
    pub fn press_power_button(&self, machine: Machine) {
        {
            let mut w = self.world.lock();
            if w.power_of(machine) != Power::Off {
                return;
            }
            w.set_power(machine, Power::Booting);
        }
        let (world, boot) = (self.world.clone(), self.times.boot_of(machine));
        tokio::spawn(async move {
            tokio::time::sleep(boot).await;
            world.lock().boot_done(machine);
        });
    }

    /// Lets time pass so that whatever was left in the air finishes.
    pub async fn settle(&self) {
        tokio::time::sleep(SETTLE).await;
    }

    /// Power matches what the main machine believes, and nothing impossible happened.
    pub async fn check(&self) -> Result<(), SimError> {
        self.check_from(self.primary).await
    }

    /// Power matches what `machine` believes.
    pub async fn check_from(&self, machine: Machine) -> Result<(), SimError> {
        self.settle().await;
        self.world
            .check(&self.agent(machine).state())
            .map_err(SimError::Mismatch)
    }

    /// Waits for the PC's agent to have the same state as the Mac's.
    pub async fn pc_agrees(&self) -> Result<State, SimError> {
        self.agrees(Machine::B).await
    }

    /// Waits for `machine`'s agent to have the same state as the other's.
    pub async fn agrees(&self, machine: Machine) -> Result<State, SimError> {
        let want = self.agent(machine.other()).state();
        let mut rx = self.agent(machine).subscribe();
        within(
            "the other machine to receive the state",
            rx.wait_for(|s| *s == want),
        )
        .await?
        .map_err(|_| SimError::Setup("the agent closed".into()))?;
        Ok(want)
    }

    /// Waits for the main machine to see the other's agent over some link.
    pub async fn wait_link(&self, kind: LinkKind) -> Result<(), SimError> {
        let links = match self.primary {
            Machine::A => &self.mac_links,
            Machine::B => &self.pc_links,
        };
        let mut rx = links.manager().watch();
        within(
            "the link",
            rx.wait_for(|s| s.active.as_ref().is_some_and(|a| a.kind == kind)),
        )
        .await?
        .map(|_| ())
        .map_err(|_| SimError::Setup("the link manager closed".into()))
    }
}
