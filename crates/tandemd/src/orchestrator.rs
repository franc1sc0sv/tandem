//! Orchestrator: runs a [`Plan`] as a transaction, handing each step to the
//! machine that has to do it (DESIGN §3.3 and §5).
//!
//! Each step goes to this machine ([`LocalSteps`]) or to the other agent over the
//! control plane ([`PeerLink`]), always with a maximum time ([`StepTimeouts`]). Execution
//! is done by [`tandem_core::run`]; here steps are only dispatched and progress is published as
//! [`TxnEvent`] for the UI.
//!
//! Power steps have rules of their own: [`Step::PowerOn`] is sending the Wake-on-LAN
//! from the other machine (repeated every [`WOL_RESEND`]) and waiting for the agent of the
//! powered-on one to connect, and a [`Step::PowerOff`] whose reply is lost is taken as done
//! (the machine is most likely shutting down).
//!
//! [`Step::SwitchMonitor`] is done by the monitor's owner (DECISIONS D13), which comes from
//! [`MonitorControl`]: if it is this machine, here; if not, the other is asked over the
//! control plane. If a later step fails, or the change is left without a verdict, it goes back to the
//! previous input (rollback).
//!
//! Scene steps ([`Step::is_scene`]: the abstract monitor, the arrangement and the geometry)
//! are done by the machine that applies the scene, with what [`Orchestrator::run_scene`]
//! gives it ([`SceneSteps`]); they are never asked of the other. If a later step fails, each one already
//! done goes back to how it was and the UI sees it as [`TxnEvent::StepUndone`].
//!
//! There is no central machine (DESIGN §3.7): either of the two orchestrates, but one at
//! a time: the one doing steps for the other is on loan until it receives its final state, and the
//! one that orchestrates rejects foreign steps.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use tandem_core::{Executor, Machine, MonitorInput, Plan, State, Step, TxnError, TxnReport};
use tandem_proto::{ControlMsg, MonitorReport, TxnEvent};

use tokio::sync::{broadcast, watch};

/// What a step answers that this machine does not know how to do yet.
pub const STEP_NOT_IMPLEMENTED: &str = "not implemented";
/// The step falls to the other agent and there is no connection to it.
pub const PEER_NOT_CONNECTED: &str = "the other agent is not connected";
/// The connection to the other agent was cut with a step pending.
pub const PEER_LOST: &str = "the connection to the other agent was lost";
/// The other agent asked for a step while this machine orchestrates its own change.
pub const BUSY_HERE: &str = "this machine is in the middle of another change";
/// A monitor that no machine can control now.
pub const NO_OWNER: &str = "no machine has a channel to the monitor";
/// It is not yet known which input the monitor shows (the owner's first reading is missing).
pub const UNKNOWN_INPUT: &str =
    "it is not yet known which input is shown; without that it could not go back";
/// A scene step outside a scene, or requested by the other machine.
pub const SCENE_ONLY: &str = "only the machine that applies the scene does it";

/// This machine's scene steps (DESIGN §5): the agent builds them for each scene that
/// applies ([`crate::scenes`]).
pub trait SceneSteps: Send + Sync {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a>;
    /// The step that leaves everything as it was before `step`, read right before doing it
    /// (`None`: there is nothing to undo). An error rejects the step without doing it: without knowing
    /// how it was, it could not go back.
    fn inverse(&self, step: &Step) -> Result<Option<Step>, String>;
}

/// The desk's monitors, seen from the orchestrator ([`crate::monitors::MonitorHub`]).
pub trait MonitorControl: Send + Sync {
    /// The machine that owns `monitor`, which is the one that changes it.
    fn owner(&self, monitor: &str) -> Option<Machine>;
    /// The input on screen according to the owner: it goes back to that if there is something to undo.
    fn input(&self, monitor: &str) -> Option<MonitorInput>;
    /// This machine changes `monitor` itself although the other owns it: the owner is unreachable
    /// and this machine has a channel to it (T-346).
    fn direct(&self, _monitor: &str) -> bool {
        false
    }
    /// Changes the input here (this machine is the owner, or [`MonitorControl::direct`]) and waits for the verdict. An error that
    /// starts with [`tandem_monitor::NO_VERDICT`] leaves in doubt whether it changed.
    fn switch<'a>(&'a self, monitor: &'a str, input: MonitorInput) -> StepFuture<'a>;
    /// The owner (here or on the other machine) confirmed `input` on `monitor` with the firmware's
    /// verdict: it is shown right away, without waiting for the owner's next reading (T-342). If
    /// this machine is the owner, it returns what it now reads to send to the other.
    fn confirmed(&self, _monitor: &str, _input: MonitorInput) -> Option<Vec<MonitorReport>> {
        None
    }
}

/// The machines' displays of each monitor (T-341, [`crate::displays`]).
pub trait DisplayWake: Send + Sync {
    /// The machine input `input` of `monitor` goes to, if it is known.
    fn machine_on(&self, monitor: &str, input: MonitorInput) -> Option<Machine>;
    /// Turns this machine's display of `monitor` back on, if it turned it off.
    fn display_on<'a>(&'a self, monitor: &'a str) -> StepFuture<'a>;
}

/// How often the Wake-on-LAN is repeated while waiting for the other machine: a UDP
/// packet can be lost, and the card takes time to be ready after a power cut.
pub const WOL_RESEND: Duration = Duration::from_secs(5);

/// Result of a step: the error is text to show in the UI.
pub type StepFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

/// The steps this machine does by itself (power).
///
/// [`Step::PowerOn`] here is only sending a Wake-on-LAN to the other machine: waiting for it and
/// repeating the send is done by the [`Orchestrator`].
pub trait LocalSteps: Send + Sync {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a>;

    /// Whom this machine wakes changed (learned when pairing or written by the UI,
    /// T-159). Only real power uses it.
    fn set_wol(&self, _wol: Option<tandem_power::WolTarget>) {}
}

/// Executor of a machine without backends: every step answers [`STEP_NOT_IMPLEMENTED`].
#[derive(Debug, Default, Clone, Copy)]
pub struct Unimplemented;

impl LocalSteps for Unimplemented {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
        Box::pin(async move { Err(format!("{}: {STEP_NOT_IMPLEMENTED}", describe(step))) })
    }
}

/// The other agent, seen from the orchestrator.
pub trait PeerLink: Send + Sync {
    /// Asks the other agent for a step and waits for its result.
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a>;
    /// Tells it the final state of a transaction, without waiting for a reply.
    fn publish_state(&self, state: &State);
    /// Sends it a notice without a reply (capabilities, desk). Without a connection it is lost:
    /// it is repeated when the next link appears.
    fn send(&self, msg: ControlMsg) {
        let _ = msg;
    }
    /// Like [`PeerLink::send`] for what replaces itself (the live cursor's
    /// samples): if the link lags, it is discarded instead of accumulating.
    fn send_lossy(&self, msg: ControlMsg) {
        self.send(msg);
    }
    /// Holds the link the steps go over for the duration of an own
    /// transaction: the link is not switched midway unless the active one drops. It is released
    /// on dropping what is returned. Without a link manager there is nothing to hold.
    fn hold_link(&self) -> Option<LinkHold> {
        None
    }
    /// The active link with the other agent and how long its last ping took (T-087: setup
    /// shows it). `None` without a link.
    fn rtt(&self) -> Option<(tandem_proto::Link, Duration)> {
        None
    }
}

/// What [`PeerLink::hold_link`] returns; the hold lasts as long as it lives.
pub type LinkHold = Box<dyn Send + Sync>;

/// Without a connection to the other agent.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoPeer;

impl PeerLink for NoPeer {
    fn execute<'a>(&'a self, _step: &'a Step) -> StepFuture<'a> {
        Box::pin(async { Err(PEER_NOT_CONNECTED.to_owned()) })
    }

    fn publish_state(&self, _state: &State) {}
}

/// The machine that knows how to do each power step; a monitor change depends on its
/// owner ([`Orchestrator::target_of`]) and gives `None`.
pub fn target(step: &Step) -> Option<Machine> {
    match step {
        // A powered-off machine cannot turn itself on: the Wake-on-LAN comes from the other.
        Step::PowerOn { machine } => Some(machine.other()),
        Step::PowerOff { machine } => Some(*machine),
        Step::SwitchMonitor { .. } => None,
        // Scene ones are done by whoever applies it ([`Orchestrator::run_scene`]).
        Step::ShowMachine { .. } | Step::Arrange { .. } | Step::Geometry => None,
        Step::DisplayOn { machine, .. } => Some(*machine),
    }
}

/// Short text of a step, for errors and logs.
pub fn describe(step: &Step) -> String {
    match step {
        Step::PowerOn { machine } => format!("power on machine {machine}"),
        Step::PowerOff { machine } => format!("power off machine {machine}"),
        Step::SwitchMonitor { monitor, input } => {
            format!("set {input} on monitor {monitor}")
        }
        Step::ShowMachine { monitor, machine } => {
            format!("show machine {machine} on monitor {monitor}")
        }
        Step::Arrange { .. } => "arrange the desktops".to_owned(),
        Step::Geometry => "move the keyboard and mouse to the new geometry".to_owned(),
        Step::DisplayOn { monitor, machine } => {
            format!("turn on the display of monitor {monitor} on machine {machine}")
        }
    }
}

/// A monitor change's error leaves in doubt whether it changed (no verdict from the firmware).
fn no_verdict(error: &str) -> bool {
    error.contains(tandem_monitor::NO_VERDICT)
}

/// What a step that got no reply says: it may have been done anyway.
const NO_ANSWER: &str = "no reply within";

/// An error of a scene step that leaves in doubt whether it was done: the scene's monitor is an
/// input change done by the owner (T-111), with the same errors.
fn scene_uncertain(error: &str) -> bool {
    no_verdict(error) || error.contains(PEER_LOST) || error.contains(NO_ANSWER)
}

/// Maximum time per step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepTimeouts {
    /// Powering on the other machine includes waiting for its agent to answer.
    pub power_on: Duration,
    /// All the other steps.
    pub step: Duration,
    /// Changing a monitor's input: the firmware takes a few seconds to give its verdict
    /// (the verdict wait, 12 s, is included).
    pub monitor: Duration,
}

/// How long an input change with its verdict can take.
pub const MONITOR_STEP: Duration = Duration::from_secs(20);

impl Default for StepTimeouts {
    fn default() -> Self {
        StepTimeouts {
            power_on: Duration::from_secs(120),
            step: Duration::from_secs(10),
            monitor: MONITOR_STEP,
        }
    }
}

impl StepTimeouts {
    pub fn for_step(&self, step: &Step) -> Duration {
        match step {
            Step::PowerOn { .. } => self.power_on,
            Step::PowerOff { .. } => self.step,
            Step::SwitchMonitor { .. } => self.monitor,
            // A scene's monitor is a `SwitchMonitor` of the owner (T-111).
            Step::ShowMachine { .. } => self.monitor,
            Step::Arrange { .. } | Step::Geometry | Step::DisplayOn { .. } => self.step,
        }
    }
}

/// Transaction notices kept for a slow subscriber before it loses any.
const EVENT_BUFFER: usize = 256;

/// Dispatches steps and publishes the progress of an agent's transactions.
pub struct Orchestrator {
    /// This machine's side; it changes only if when pairing it turns out to be the other.
    me: Mutex<Machine>,
    local: Arc<dyn LocalSteps>,
    peer: RwLock<Arc<dyn PeerLink>>,
    /// Whether there is a connection to the other agent: powering on the other ends when it becomes `true`.
    connected: watch::Sender<bool>,
    timeouts: StepTimeouts,
    events: broadcast::Sender<TxnEvent>,
    next_txn: AtomicU64,
    busy: AtomicBool,
    /// Until when this machine is on loan to a transaction of the other agent.
    /// While one orchestrates, the other cannot start a change of its own, and one that
    /// orchestrates does not accept foreign steps. Both checks go under this lock so
    /// they do not cross.
    peer_lease: Mutex<Option<tokio::time::Instant>>,
    /// The other agent's steps run one at a time even if they arrive over streams (links)
    /// different ones.
    peer_turn: tokio::sync::Mutex<()>,
    /// The link held during the current own transaction.
    link_hold: Mutex<Option<LinkHold>>,
    /// The monitors; without them, every monitor change fails with [`NO_OWNER`].
    monitors: RwLock<Option<Arc<dyn MonitorControl>>>,
    /// The displays; without them, a monitor change wakes no display first.
    displays: RwLock<Option<Arc<dyn DisplayWake>>>,
}

impl std::fmt::Debug for Orchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Orchestrator")
            .field("me", &self.me())
            .field("timeouts", &self.timeouts)
            .field("busy", &self.busy)
            .finish_non_exhaustive()
    }
}

impl Orchestrator {
    pub fn new(me: Machine, local: Arc<dyn LocalSteps>, timeouts: StepTimeouts) -> Self {
        Orchestrator {
            me: Mutex::new(me),
            local,
            peer: RwLock::new(Arc::new(NoPeer)),
            connected: watch::Sender::new(false),
            timeouts,
            events: broadcast::channel(EVENT_BUFFER).0,
            next_txn: AtomicU64::new(1),
            busy: AtomicBool::new(false),
            peer_lease: Mutex::new(None),
            peer_turn: tokio::sync::Mutex::new(()),
            link_hold: Mutex::new(None),
            monitors: RwLock::new(None),
            displays: RwLock::new(None),
        }
    }

    /// Whom this machine wakes, from now on.
    pub fn set_wol(&self, wol: Option<tandem_power::WolTarget>) {
        self.local.set_wol(wol);
    }

    /// Who knows about the desk's monitors.
    pub fn set_monitors(&self, monitors: Arc<dyn MonitorControl>) {
        *self
            .monitors
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(monitors);
    }

    /// Who turns the displays back on before a monitor change (T-341).
    pub fn set_displays(&self, displays: Arc<dyn DisplayWake>) {
        *self
            .displays
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(displays);
    }

    fn displays(&self) -> Option<Arc<dyn DisplayWake>> {
        self.displays
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn monitors(&self) -> Option<Arc<dyn MonitorControl>> {
        self.monitors
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The machine that does `step`: the one from [`target`], or the monitor's owner. While the
    /// owner is unreachable, this machine changes a monitor it has a channel to (T-346).
    pub fn target_of(&self, step: &Step) -> Result<Machine, String> {
        match step {
            Step::SwitchMonitor { monitor, .. } => {
                let monitors = self.monitors();
                let owner = monitors
                    .as_ref()
                    .and_then(|m| m.owner(monitor))
                    .ok_or_else(|| format!("{}: {NO_OWNER}", describe(step)))?;
                let direct = monitors.is_some_and(|m| m.direct(monitor));
                Ok(if direct { self.me() } else { owner })
            }
            s if s.is_scene() => Err(format!("{}: {SCENE_ONLY}", describe(step))),
            _ => target(step).ok_or_else(|| describe(step)),
        }
    }

    fn me(&self) -> Machine {
        *self.me.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// This machine turned out to be the other side ([`crate::Agent::adopt_side`]).
    pub fn set_machine(&self, side: Machine) {
        *self.me.lock().unwrap_or_else(PoisonError::into_inner) = side;
    }

    /// Changes the connection to the other agent. Steps already in progress continue with the previous one.
    pub fn set_peer(&self, peer: Arc<dyn PeerLink>) {
        *self.peer.write().unwrap_or_else(PoisonError::into_inner) = peer;
        self.connected.send_replace(true);
    }

    /// The connection to the other agent was lost (for example, the PC powered off).
    pub fn clear_peer(&self) {
        *self.peer.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(NoPeer);
        self.connected.send_replace(false);
    }

    /// Whether there is a connection to the other agent, and notices when it changes.
    pub fn peer_connected(&self) -> watch::Receiver<bool> {
        self.connected.subscribe()
    }

    pub fn peer(&self) -> Arc<dyn PeerLink> {
        self.peer
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TxnEvent> {
        self.events.subscribe()
    }

    /// Reserves the orchestrator for a transaction; `None` if one is already in progress,
    /// its own or the other agent's.
    pub fn try_begin(&self) -> Option<u64> {
        let lease = self.lease();
        if lease.is_some_and(|until| tokio::time::Instant::now() < until) {
            return None;
        }
        let txn = self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| self.next_txn.fetch_add(1, Ordering::Relaxed))?;
        *self.hold() = self.peer().hold_link();
        Some(txn)
    }

    fn hold(&self) -> std::sync::MutexGuard<'_, Option<LinkHold>> {
        self.link_hold
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn lease(&self) -> std::sync::MutexGuard<'_, Option<tokio::time::Instant>> {
        self.peer_lease
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// No transaction in progress, neither its own nor the other agent's. It is only a snapshot: to
    /// reserve the orchestrator there is [`Orchestrator::try_begin`].
    pub fn is_idle(&self) -> bool {
        let leased = self
            .lease()
            .is_some_and(|until| tokio::time::Instant::now() < until);
        !leased && !self.busy.load(Ordering::Acquire)
    }

    /// The other agent's transaction ended (its final state arrived).
    pub fn release_peer_lease(&self) {
        *self.lease() = None;
    }

    /// How long the loan lasts after each step of the other agent: the most it can
    /// take to ask for the next one (an own step with its maximum time, the
    /// longest is powering on the other) plus one step of ours. If its final state
    /// is lost, the machine is free after this at most.
    fn lease_length(&self) -> Duration {
        self.timeouts.power_on + self.timeouts.step
    }

    /// Releases what was reserved with [`Orchestrator::try_begin`].
    pub fn end(&self) {
        // First the link: the final notice to the other agent can already go over the best one.
        drop(self.hold().take());
        self.busy.store(false, Ordering::Release);
    }

    fn emit(&self, event: TxnEvent) {
        // Without a connected UI there is nobody to notify, and that is not an error.
        let _ = self.events.send(event);
    }

    /// Runs `plan` over `state` publishing progress; the outcome is published by
    /// [`Orchestrator::finish`], once the state was saved.
    pub async fn run(
        &self,
        txn: u64,
        state: &mut State,
        plan: &Plan,
    ) -> Result<TxnReport, TxnError> {
        self.run_with(txn, state, plan, None).await
    }

    /// [`Orchestrator::run`] of a scene: its scene steps are done by `scene`.
    pub async fn run_scene(
        &self,
        txn: u64,
        state: &mut State,
        plan: &Plan,
        scene: Arc<dyn SceneSteps>,
    ) -> Result<TxnReport, TxnError> {
        self.run_with(txn, state, plan, Some(scene)).await
    }

    async fn run_with(
        &self,
        txn: u64,
        state: &mut State,
        plan: &Plan,
        scene: Option<Arc<dyn SceneSteps>>,
    ) -> Result<TxnReport, TxnError> {
        self.emit(TxnEvent::Started {
            txn,
            steps: plan.steps.clone(),
        });
        let mut d = Dispatcher {
            orch: self,
            txn,
            next: 0,
            inverses: Vec::new(),
            scene,
        };
        tandem_core::run(state, plan, &mut d).await
    }

    /// Publishes the outcome to the UI and the final state to the other agent.
    pub fn finish(&self, txn: u64, state: &State, outcome: &Result<TxnReport, TxnError>) {
        let event = match outcome {
            Ok(_) => TxnEvent::Committed {
                txn,
                state: state.clone(),
            },
            Err(TxnError::Failed { .. }) => TxnEvent::Failed {
                txn,
                state: state.clone(),
            },
        };
        self.emit(event);
        self.peer().publish_state(state);
    }

    /// A step with its maximum time, on the machine it falls to.
    pub async fn dispatch(&self, step: &Step) -> Result<(), String> {
        self.attempt(step).await.map_err(|f| f.error)
    }

    async fn attempt(&self, step: &Step) -> Result<(), StepFailure> {
        if let Step::SwitchMonitor { monitor, input } = step {
            self.wake_display(monitor, *input).await;
        }
        let limit = self.timeouts.for_step(step);
        let work = async {
            let target = self.target_of(step).map_err(StepFailure::certain)?;
            if target == self.me() {
                self.run_local(step).await.map_err(|error| StepFailure {
                    uncertain: no_verdict(&error),
                    error,
                })
            } else {
                let r = self.remote(step).await;
                if r.is_ok() {
                    self.confirmed(step);
                }
                r
            }
        };
        match tokio::time::timeout(limit, work).await {
            Ok(r) => r,
            Err(_) => Err(StepFailure {
                error: format!("{}: {NO_ANSWER} {} s", describe(step), limit.as_secs()),
                uncertain: true,
            }),
        }
    }

    /// Before a change to `input` of `monitor`, the machine that input goes to turns its display
    /// back on (T-341): with it off, the monitor would land on a dark input. Whatever happens,
    /// the change goes ahead: a machine without display control simply has nothing to do.
    async fn wake_display(&self, monitor: &str, input: MonitorInput) {
        let Some(displays) = self.displays() else {
            return;
        };
        let Some(machine) = displays.machine_on(monitor, input) else {
            return;
        };
        let step = Step::DisplayOn {
            monitor: monitor.to_owned(),
            machine,
        };
        let work = async {
            if machine == self.me() {
                displays.display_on(monitor).await
            } else {
                self.peer().execute(&step).await
            }
        };
        match tokio::time::timeout(self.timeouts.for_step(&step), work).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) if e == PEER_NOT_CONNECTED => {}
            Ok(Err(e)) => tracing::warn!("{}: {e}", describe(&step)),
            Err(_) => tracing::warn!("{}: {NO_ANSWER} the time limit", describe(&step)),
        }
    }

    async fn run_local(&self, step: &Step) -> Result<(), String> {
        match step {
            Step::DisplayOn { monitor, .. } => match self.displays() {
                Some(d) => d.display_on(monitor).await,
                None => Ok(()),
            },
            Step::PowerOn { machine } if *machine != self.me() => self.power_on_peer(step).await,
            Step::SwitchMonitor { monitor, input } => match self.monitors() {
                Some(m) => {
                    m.switch(monitor, *input).await?;
                    self.confirmed(step);
                    Ok(())
                }
                None => Err(format!("{}: {NO_OWNER}", describe(step))),
            },
            _ => self.local.execute(step).await,
        }
    }

    /// A [`Step::SwitchMonitor`] with a confirmed verdict: the new input is shown on this machine
    /// and, if it is the owner, on the other too, before the step's reply. Whoever waits for it (the
    /// scene's geometry, the UI) does not depend on the next full reading of the bus (T-342).
    fn confirmed(&self, step: &Step) {
        let (Step::SwitchMonitor { monitor, input }, Some(m)) = (step, self.monitors()) else {
            return;
        };
        if let Some(reports) = m.confirmed(monitor, *input) {
            self.peer().send(ControlMsg::Monitors(reports));
        }
    }

    /// Wake-on-LAN until the other machine's agent connects. The cap is set by
    /// the caller (`power_on_timeout_secs`); an error sending the packet cuts it short right away.
    async fn power_on_peer(&self, step: &Step) -> Result<(), String> {
        let mut connected = self.connected.subscribe();
        loop {
            self.local.execute(step).await?;
            tokio::select! {
                // The sender lives in `self`: the wait cannot be left without a sender.
                r = connected.wait_for(|c| *c) => {
                    return r.map(|_| ()).map_err(|_| PEER_LOST.to_owned());
                }
                () = tokio::time::sleep(WOL_RESEND) => {
                    tracing::debug!(
                        "machine {} not responding yet: repeating the Wake-on-LAN",
                        self.me().other()
                    );
                }
            }
        }
    }

    async fn remote(&self, step: &Step) -> Result<(), StepFailure> {
        let peer = self.peer();
        peer.execute(step).await.map_err(|e| StepFailure {
            uncertain: e == PEER_LOST || no_verdict(&e),
            error: format!("machine {}: {e}", self.me().other()),
        })
    }

    /// A step requested by the other agent: only the ones that fall to this machine are accepted.
    pub async fn execute_for_peer(&self, step: &Step) -> Result<(), String> {
        let me = self.me();
        if self.target_of(step)? != me {
            return Err(format!(
                "{}: does not belong to machine {me}",
                describe(step)
            ));
        }
        // The wait is bounded: whoever has the turn runs with its maximum time.
        let _turn = self.peer_turn.lock().await;
        {
            let mut lease = self.lease();
            if self.busy.load(Ordering::Acquire) {
                return Err(format!("{}: {BUSY_HERE}", describe(step)));
            }
            *lease = Some(tokio::time::Instant::now() + self.lease_length());
        }
        let limit = self.timeouts.for_step(step);
        match tokio::time::timeout(limit, self.run_local(step)).await {
            Ok(r) => r,
            Err(_) => Err(format!(
                "{}: no reply within {} s",
                describe(step),
                limit.as_secs()
            )),
        }
    }
}

/// Why a step failed.
struct StepFailure {
    error: String,
    /// Whether it may have been done anyway: the time ran out or the connection was cut.
    uncertain: bool,
}

impl StepFailure {
    fn certain(error: String) -> Self {
        StepFailure {
            error,
            uncertain: false,
        }
    }
}

/// A transaction's [`Executor`]: dispatches and counts the steps for the UI.
struct Dispatcher<'a> {
    orch: &'a Orchestrator,
    txn: u64,
    next: u32,
    /// Each done step that is known how to undo (monitor change, scene steps), with its
    /// position in the plan and the step that undoes it.
    inverses: Vec<(u32, Step, Step)>,
    /// Whoever does the scene steps; `None` outside a scene.
    scene: Option<Arc<dyn SceneSteps>>,
}

impl Dispatcher<'_> {
    /// The step that leaves the monitor as it was before `step` (`None` if there is nothing to
    /// undo). An input change without knowing the previous one would have nowhere to go back to if it
    /// is left without a verdict: it is rejected without touching the monitor.
    fn inverse(&self, step: &Step) -> Result<Option<Step>, String> {
        if step.is_scene() {
            // Without a scene the step fails on its own, with [`SCENE_ONLY`].
            return match &self.scene {
                Some(scene) => scene.inverse(step),
                None => Ok(None),
            };
        }
        let Step::SwitchMonitor { monitor, input } = step else {
            return Ok(None);
        };
        // Without monitors, the step fails on its own for having no owner ([`NO_OWNER`]).
        let Some(monitors) = self.orch.monitors() else {
            return Ok(None);
        };
        let before = monitors
            .input(monitor)
            .ok_or_else(|| format!("{}: {UNKNOWN_INPUT}", describe(step)))?;
        Ok((before != *input).then(|| Step::SwitchMonitor {
            monitor: monitor.clone(),
            input: before,
        }))
    }

    async fn roll_back(&self, inverse: &Step) -> Result<(), String> {
        tracing::warn!(txn = self.txn, "rollback: {}", describe(inverse));
        self.perform(inverse).await.map_err(|f| f.error)
    }

    /// A step with its maximum time: scene ones, here with [`SceneSteps`]; the rest, on
    /// the machine it falls to.
    async fn perform(&self, step: &Step) -> Result<(), StepFailure> {
        let scene = match &self.scene {
            Some(scene) if step.is_scene() => scene,
            _ => return self.orch.attempt(step).await,
        };
        let limit = self.orch.timeouts.for_step(step);
        match tokio::time::timeout(limit, scene.execute(step)).await {
            Ok(r) => r.map_err(|error| StepFailure {
                uncertain: scene_uncertain(&error),
                error,
            }),
            Err(_) => Err(StepFailure {
                error: format!("{}: {NO_ANSWER} {} s", describe(step), limit.as_secs()),
                uncertain: true,
            }),
        }
    }
}

impl Executor for Dispatcher<'_> {
    async fn execute(&mut self, step: &Step) -> Result<(), String> {
        let (txn, index) = (self.txn, self.next);
        self.orch.emit(TxnEvent::StepStarted { txn, index });
        let inverse = match self.inverse(step) {
            Ok(inverse) => inverse,
            Err(error) => {
                tracing::warn!(txn, index, "{error}");
                self.orch.emit(TxnEvent::StepFailed {
                    txn,
                    index,
                    error: error.clone(),
                });
                return Err(error);
            }
        };
        match self.perform(step).await {
            Ok(()) => {
                if let Some(inverse) = inverse {
                    self.inverses.push((index, step.clone(), inverse));
                }
                self.orch.emit(TxnEvent::StepDone { txn, index });
                self.next += 1;
                Ok(())
            }
            // If the shutdown reply does not arrive, it most likely powered off
            // (or is about to). It is taken as done; if it is still on, the peer watch
            // sees it again and marks it on.
            Err(StepFailure {
                error,
                uncertain: true,
            }) if matches!(step, Step::PowerOff { .. }) => {
                tracing::warn!(
                    txn,
                    index,
                    "the shutdown did not answer; taken as done: {error}"
                );
                self.orch.emit(TxnEvent::StepDone { txn, index });
                self.next += 1;
                Ok(())
            }
            Err(StepFailure {
                mut error,
                uncertain,
            }) => {
                tracing::warn!(txn, index, "a step failed: {error}");
                // An input change that may have been done (without verdict, without reply) is
                // undone here: for the transaction it was not applied.
                if uncertain
                    && let Some(inverse) = &inverse
                    && let Err(e) = self.roll_back(inverse).await
                {
                    error.push_str(&format!("; could not go back: {e}"));
                }
                self.orch.emit(TxnEvent::StepFailed {
                    txn,
                    index,
                    error: error.clone(),
                });
                Err(error)
            }
        }
    }

    async fn undo(&mut self, step: &Step) -> Result<(), String> {
        let Some(at) = self.inverses.iter().rposition(|(_, done, _)| done == step) else {
            return Ok(());
        };
        let (index, _, inverse) = self.inverses.remove(at);
        self.roll_back(&inverse).await?;
        self.orch.emit(TxnEvent::StepUndone {
            txn: self.txn,
            index,
        });
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use tandem_core::Abilities;

    use super::*;

    /// Simulated steps: they record everything, fail on the indicated ones and take as long as told.
    #[derive(Default)]
    pub struct FakeSteps {
        pub log: Mutex<Vec<Step>>,
        pub fail_on: Vec<Step>,
        pub delays: Vec<(Step, Duration)>,
        /// They are applied, but the reply is lost with the connection.
        pub lost_reply_on: Vec<Step>,
    }

    impl LocalSteps for FakeSteps {
        fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
            Box::pin(async move {
                self.log
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(step.clone());
                if let Some((_, d)) = self.delays.iter().find(|(s, _)| s == step) {
                    tokio::time::sleep(*d).await;
                }
                if self.fail_on.contains(step) {
                    Err("simulated failure".to_owned())
                } else if self.lost_reply_on.contains(step) {
                    Err(PEER_LOST.to_owned())
                } else {
                    Ok(())
                }
            })
        }
    }

    impl FakeSteps {
        pub fn steps(&self) -> Vec<Step> {
            self.log
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    /// The simulated other agent without a network: another [`FakeSteps`].
    pub struct FakePeer(pub Arc<FakeSteps>);

    impl PeerLink for FakePeer {
        fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
            self.0.execute(step)
        }
        fn publish_state(&self, _state: &State) {}
    }

    const PC_ON: Step = Step::PowerOn {
        machine: Machine::B,
    };
    const PC_OFF: Step = Step::PowerOff {
        machine: Machine::B,
    };

    fn mac(local: FakeSteps, peer: FakeSteps) -> (Orchestrator, Arc<FakeSteps>, Arc<FakeSteps>) {
        let local = Arc::new(local);
        let peer = Arc::new(peer);
        let o = Orchestrator::new(Machine::A, local.clone(), StepTimeouts::default());
        o.set_peer(Arc::new(FakePeer(peer.clone())));
        (o, local, peer)
    }

    fn drain(rx: &mut broadcast::Receiver<TxnEvent>) -> Vec<TxnEvent> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    /// Seen from the Mac, with the PC on.
    fn both_on() -> State {
        let mut s = State::initial(Machine::A);
        s.on.b = true;
        s
    }

    #[test]
    fn routes_each_step_to_its_machine() {
        assert_eq!(target(&PC_ON), Some(Machine::A));
        assert_eq!(target(&PC_OFF), Some(Machine::B));
        assert_eq!(describe(&PC_ON), "power on machine B");
        assert_eq!(
            describe(&Step::PowerOff {
                machine: Machine::A
            }),
            "power off machine A"
        );
    }

    #[test]
    fn power_on_gets_the_long_timeout() {
        let t = StepTimeouts::default();
        assert_eq!(t.for_step(&PC_ON), Duration::from_secs(120));
        assert_eq!(t.for_step(&PC_OFF), Duration::from_secs(10));
    }

    #[tokio::test]
    async fn dispatches_locally_or_remotely() {
        let (o, local, peer) = mac(FakeSteps::default(), FakeSteps::default());
        o.dispatch(&PC_OFF).await.unwrap();
        o.dispatch(&PC_ON).await.unwrap();
        assert_eq!(local.steps(), [PC_ON]);
        assert_eq!(peer.steps(), [PC_OFF]);
    }

    #[tokio::test]
    async fn remote_errors_name_the_machine() {
        let (o, _, _) = mac(
            FakeSteps::default(),
            FakeSteps {
                fail_on: vec![PC_OFF],
                ..Default::default()
            },
        );
        let e = o.dispatch(&PC_OFF).await.unwrap_err();
        assert_eq!(e, "machine B: simulated failure");
    }

    #[tokio::test]
    async fn without_peer_remote_steps_fail_clearly() {
        let o = Orchestrator::new(Machine::A, Arc::new(Unimplemented), StepTimeouts::default());
        let e = o.dispatch(&PC_OFF).await.unwrap_err();
        assert!(e.contains(PEER_NOT_CONNECTED), "{e}");
        let e = o.dispatch(&PC_ON).await.unwrap_err();
        assert_eq!(e, format!("power on machine B: {STEP_NOT_IMPLEMENTED}"));
    }

    #[tokio::test]
    async fn refuses_peer_requests_for_the_peers_own_steps() {
        let (o, local, _) = mac(FakeSteps::default(), FakeSteps::default());
        assert!(o.execute_for_peer(&PC_OFF).await.is_err());
        o.execute_for_peer(&PC_ON).await.unwrap();
        assert_eq!(local.steps(), [PC_ON]);
    }

    #[test]
    fn only_one_transaction_at_a_time() {
        let o = Orchestrator::new(Machine::A, Arc::new(Unimplemented), StepTimeouts::default());
        let a = o.try_begin().unwrap();
        assert_eq!(o.try_begin(), None);
        o.end();
        let b = o.try_begin().unwrap();
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn publishes_progress_of_a_committed_transaction() {
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        let mut rx = o.subscribe();
        let mut state = both_on();
        let plan = Plan::power_off(&state, Machine::B);
        let outcome = o.run(7, &mut state, &plan).await;
        o.finish(7, &state, &outcome);
        assert!(outcome.is_ok());
        assert!(!state.on.b);
        assert_eq!(
            drain(&mut rx),
            [
                TxnEvent::Started {
                    txn: 7,
                    steps: plan.steps.clone(),
                },
                TxnEvent::StepStarted { txn: 7, index: 0 },
                TxnEvent::StepDone { txn: 7, index: 0 },
                TxnEvent::Committed { txn: 7, state },
            ]
        );
    }

    #[tokio::test]
    async fn a_failed_step_publishes_the_failure_and_keeps_the_state() {
        let (o, _, _) = mac(
            FakeSteps::default(),
            FakeSteps {
                fail_on: vec![PC_OFF],
                ..Default::default()
            },
        );
        let mut rx = o.subscribe();
        let mut state = both_on();
        let initial = state.clone();
        let plan = Plan::power_off(&state, Machine::B);
        let outcome = o.run(1, &mut state, &plan).await;
        o.finish(1, &state, &outcome);
        assert!(matches!(outcome, Err(TxnError::Failed { .. })));
        assert_eq!(state, initial);
        let events = drain(&mut rx);
        assert_eq!(
            events[events.len() - 2..],
            [
                TxnEvent::StepFailed {
                    txn: 1,
                    index: 0,
                    error: "machine B: simulated failure".into()
                },
                TxnEvent::Failed {
                    txn: 1,
                    state: initial
                },
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn power_on_resends_wake_until_the_pc_agent_connects() {
        let local = Arc::new(FakeSteps::default());
        let o = Arc::new(Orchestrator::new(
            Machine::A,
            local.clone(),
            StepTimeouts::default(),
        ));
        assert!(!*o.peer_connected().borrow());
        let o2 = o.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(12)).await;
            o2.set_peer(Arc::new(FakePeer(Arc::new(FakeSteps::default()))));
        });
        let start = tokio::time::Instant::now();
        o.dispatch(&PC_ON).await.unwrap();
        assert_eq!(start.elapsed(), Duration::from_secs(12));
        // Sends at 0, 5 and 10 s.
        assert_eq!(local.steps(), vec![PC_ON; 3]);
    }

    #[tokio::test(start_paused = true)]
    async fn power_on_without_the_pc_agent_times_out() {
        let local = Arc::new(FakeSteps::default());
        let o = Orchestrator::new(Machine::A, local.clone(), StepTimeouts::default());
        let e = o.dispatch(&PC_ON).await.unwrap_err();
        assert_eq!(e, "power on machine B: no reply within 120 s");
        // One send every 5 s from 0; the one at 120 s coincides with the cap and may or may not go out.
        let sent = local.steps().len();
        assert!((24..=25).contains(&sent), "{sent} sends");
    }

    #[tokio::test(start_paused = true)]
    async fn power_on_stops_at_a_send_error() {
        let o = Orchestrator::new(
            Machine::A,
            Arc::new(FakeSteps {
                fail_on: vec![PC_ON],
                ..Default::default()
            }),
            StepTimeouts::default(),
        );
        let start = tokio::time::Instant::now();
        assert_eq!(o.dispatch(&PC_ON).await.unwrap_err(), "simulated failure");
        assert_eq!(start.elapsed(), Duration::ZERO);
    }

    #[tokio::test]
    async fn losing_the_peer_makes_power_on_wait_again() {
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        assert!(*o.peer_connected().borrow());
        o.clear_peer();
        assert!(!*o.peer_connected().borrow());
        let e = o.dispatch(&PC_OFF).await.unwrap_err();
        assert!(e.contains(PEER_NOT_CONNECTED), "{e}");
    }

    #[tokio::test(start_paused = true)]
    async fn power_on_may_take_longer_than_other_steps() {
        // 60 s to power on machine B is within the 120 s.
        let (o, _, _) = mac(
            FakeSteps {
                delays: vec![(PC_ON, Duration::from_secs(60))],
                ..Default::default()
            },
            FakeSteps::default(),
        );
        let mut state = State::initial(Machine::A);
        let plan = Plan::power_on(&state, Machine::B, &Abilities::both_wake()).unwrap();
        o.run(4, &mut state, &plan).await.unwrap();
        assert!(state.on.b);

        // 121 s, no.
        let (o, _, _) = mac(
            FakeSteps {
                delays: vec![(PC_ON, Duration::from_secs(121))],
                ..Default::default()
            },
            FakeSteps::default(),
        );
        let mut state = State::initial(Machine::A);
        let err = o.run(5, &mut state, &plan).await.unwrap_err();
        assert!(matches!(err, TxnError::Failed { step: PC_ON, .. }));
        assert_eq!(state, State::initial(Machine::A));
    }

    // QA round 2: each agent only looked after its own transactions, so
    // the Mac and the PC could orchestrate at once over the same desk.
    #[tokio::test]
    async fn while_orchestrating_peer_steps_are_refused_without_running() {
        let (o, local, _) = mac(FakeSteps::default(), FakeSteps::default());
        let _txn = o.try_begin().unwrap();
        let e = o.execute_for_peer(&PC_ON).await.unwrap_err();
        assert!(e.contains(BUSY_HERE), "{e}");
        assert!(local.steps().is_empty());
        o.end();
        o.execute_for_peer(&PC_ON).await.unwrap();
        assert_eq!(local.steps(), [PC_ON]);
    }

    // T-064: two requests from the other agent arriving over different streams do not trample each other.
    #[tokio::test(start_paused = true)]
    async fn peer_steps_from_different_streams_run_one_at_a_time() {
        let mac_off = Step::PowerOff {
            machine: Machine::A,
        };
        let (o, local, _) = mac(
            FakeSteps {
                delays: vec![(PC_ON, Duration::from_secs(3))],
                ..Default::default()
            },
            FakeSteps::default(),
        );
        let start = tokio::time::Instant::now();
        let (first, second) = tokio::join!(
            async {
                o.execute_for_peer(&PC_ON).await.unwrap();
                start.elapsed()
            },
            async {
                tokio::task::yield_now().await;
                o.execute_for_peer(&mac_off).await.unwrap();
                start.elapsed()
            },
        );
        assert_eq!(first, Duration::from_secs(3));
        assert!(second >= first, "the second finished earlier: {second:?}");
        assert_eq!(local.steps(), [PC_ON, mac_off]);
    }

    /// An other agent that counts how many link holds are alive.
    struct HoldingPeer(Arc<std::sync::atomic::AtomicUsize>);

    struct CountedHold(Arc<std::sync::atomic::AtomicUsize>);

    impl Drop for CountedHold {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl PeerLink for HoldingPeer {
        fn execute<'a>(&'a self, _step: &'a Step) -> StepFuture<'a> {
            Box::pin(async { Ok(()) })
        }
        fn publish_state(&self, _state: &State) {}
        fn hold_link(&self) -> Option<LinkHold> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Some(Box::new(CountedHold(self.0.clone())))
        }
    }

    // T-065: during an own transaction the link does not change unless it drops.
    #[test]
    fn a_transaction_holds_the_link_until_it_ends() {
        let holds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let o = Orchestrator::new(
            Machine::A,
            Arc::new(FakeSteps::default()),
            StepTimeouts::default(),
        );
        o.set_peer(Arc::new(HoldingPeer(holds.clone())));
        let count = || holds.load(Ordering::SeqCst);

        assert_eq!(count(), 0);
        o.try_begin().unwrap();
        assert_eq!(count(), 1);
        // A second rejected attempt holds nothing more.
        assert_eq!(o.try_begin(), None);
        assert_eq!(count(), 1);
        o.end();
        assert_eq!(count(), 0);
        o.try_begin().unwrap();
        o.end();
        assert_eq!(count(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn serving_the_peer_blocks_own_changes_until_it_finishes() {
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        o.execute_for_peer(&PC_ON).await.unwrap();
        assert_eq!(o.try_begin(), None, "the machine is on loan");
        // The other's transaction final state arrives: it is free.
        o.release_peer_lease();
        let txn = o.try_begin().unwrap();
        o.end();

        // If the final state is lost, the loan expires on its own.
        o.execute_for_peer(&PC_ON).await.unwrap();
        let t = StepTimeouts::default();
        tokio::time::advance(t.power_on + t.step - Duration::from_millis(1)).await;
        assert_eq!(o.try_begin(), None);
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(o.try_begin().is_some_and(|next| next > txn));
    }

    /// Test monitors: a fixed owner, an input and what each change answers.
    struct FakeMonitors {
        owner: Option<Machine>,
        /// [`MonitorControl::direct`]: the owner is unreachable and this machine has a channel.
        direct: bool,
        input: Option<MonitorInput>,
        answer: Mutex<Vec<Result<(), String>>>,
        switched: Mutex<Vec<MonitorInput>>,
        confirmed: Mutex<Vec<MonitorInput>>,
    }

    impl FakeMonitors {
        fn new(owner: Option<Machine>, answers: Vec<Result<(), String>>) -> Arc<Self> {
            Arc::new(FakeMonitors {
                owner,
                direct: false,
                input: Some(MonitorInput::DisplayPort),
                answer: Mutex::new(answers),
                switched: Mutex::new(Vec::new()),
                confirmed: Mutex::new(Vec::new()),
            })
        }

        fn switched(&self) -> Vec<MonitorInput> {
            self.switched.lock().unwrap().clone()
        }

        fn confirmed(&self) -> Vec<MonitorInput> {
            self.confirmed.lock().unwrap().clone()
        }
    }

    impl MonitorControl for FakeMonitors {
        fn owner(&self, _monitor: &str) -> Option<Machine> {
            self.owner
        }
        fn input(&self, _monitor: &str) -> Option<MonitorInput> {
            self.input
        }
        fn direct(&self, _monitor: &str) -> bool {
            self.direct
        }
        fn switch<'a>(&'a self, _monitor: &'a str, input: MonitorInput) -> StepFuture<'a> {
            self.switched.lock().unwrap().push(input);
            let mut answers = self.answer.lock().unwrap();
            let r = if answers.is_empty() {
                Ok(())
            } else {
                answers.remove(0)
            };
            Box::pin(async move { r })
        }
        fn confirmed(&self, _monitor: &str, input: MonitorInput) -> Option<Vec<MonitorReport>> {
            self.confirmed.lock().unwrap().push(input);
            // As the owner, what it reads goes to the other.
            (self.owner == Some(Machine::A)).then(Vec::new)
        }
    }

    /// The other agent that also keeps the notices it receives.
    #[derive(Default)]
    struct NoticePeer {
        steps: FakeSteps,
        sent: Mutex<Vec<ControlMsg>>,
    }

    impl PeerLink for NoticePeer {
        fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
            self.steps.execute(step)
        }
        fn publish_state(&self, _state: &State) {}
        fn send(&self, msg: ControlMsg) {
            self.sent.lock().unwrap().push(msg);
        }
    }

    fn to_hdmi() -> Step {
        Step::SwitchMonitor {
            monitor: "m".into(),
            input: MonitorInput::Hdmi,
        }
    }

    /// Scene steps are done by whoever applies the scene: never the other on request, nor outside
    /// a scene.
    #[tokio::test]
    async fn scene_steps_are_never_done_for_the_other() {
        let (o, local, peer) = mac(FakeSteps::default(), FakeSteps::default());
        for step in [
            Step::Geometry,
            Step::Arrange { layout: None },
            Step::ShowMachine {
                monitor: "m".into(),
                machine: Machine::B,
            },
        ] {
            let e = o.execute_for_peer(&step).await.unwrap_err();
            assert!(e.contains(SCENE_ONLY), "{e}");
            let e = o.dispatch(&step).await.unwrap_err();
            assert!(e.contains(SCENE_ONLY), "{e}");
        }
        assert!(local.steps().is_empty() && peer.steps().is_empty());
    }

    /// T-346: with the owner unreachable and a channel here, this machine switches by itself
    /// instead of asking the other.
    #[tokio::test]
    async fn without_the_owner_a_machine_with_a_channel_switches_itself() {
        let (o, local, peer) = mac(FakeSteps::default(), FakeSteps::default());
        let m = Arc::new(FakeMonitors {
            direct: true,
            ..Arc::into_inner(FakeMonitors::new(Some(Machine::B), vec![])).unwrap()
        });
        o.set_monitors(m.clone());
        assert_eq!(o.target_of(&to_hdmi()), Ok(Machine::A));
        o.dispatch(&to_hdmi()).await.unwrap();
        assert_eq!(m.switched(), [MonitorInput::Hdmi]);
        assert_eq!(m.confirmed(), [MonitorInput::Hdmi]);
        assert!(local.steps().is_empty() && peer.steps().is_empty());
    }

    #[tokio::test]
    async fn a_monitor_switch_goes_to_its_owner() {
        // This machine owns: it does it here, without going through the power steps.
        let (o, local, peer) = mac(FakeSteps::default(), FakeSteps::default());
        let m = FakeMonitors::new(Some(Machine::A), vec![]);
        o.set_monitors(m.clone());
        o.dispatch(&to_hdmi()).await.unwrap();
        assert_eq!(m.switched(), [MonitorInput::Hdmi]);
        assert!(local.steps().is_empty() && peer.steps().is_empty());
        assert_eq!(o.timeouts.for_step(&to_hdmi()), MONITOR_STEP);

        // The other owns: it is asked.
        let (o, _, peer) = mac(FakeSteps::default(), FakeSteps::default());
        let m = FakeMonitors::new(Some(Machine::B), vec![]);
        o.set_monitors(m.clone());
        o.dispatch(&to_hdmi()).await.unwrap();
        assert_eq!(peer.steps(), [to_hdmi()]);
        assert!(m.switched().is_empty());
        // And the other does not do it for this one if the owner is this one.
        let e = o.execute_for_peer(&to_hdmi()).await.unwrap_err();
        assert!(e.contains("does not belong to"), "{e}");

        // Without an owner (or without monitors) it says why.
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        let e = o.dispatch(&to_hdmi()).await.unwrap_err();
        assert!(e.contains(NO_OWNER), "{e}");
        o.set_monitors(FakeMonitors::new(None, vec![]));
        let e = o.dispatch(&to_hdmi()).await.unwrap_err();
        assert!(e.contains(NO_OWNER), "{e}");
    }

    /// T-342: a change with a confirmed verdict is shown at once, on the machine that asked and, if
    /// the owner did it, on the other too (before the step's reply), without waiting for the next
    /// reading of the bus. A failed verdict changes nothing.
    #[tokio::test]
    async fn a_confirmed_switch_is_shown_at_once() {
        // This machine owns: it confirms and sends what it reads to the other.
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        let peer = Arc::new(NoticePeer::default());
        o.set_peer(peer.clone());
        let m = FakeMonitors::new(Some(Machine::A), vec![Ok(()), Err("still on DP".into())]);
        o.set_monitors(m.clone());
        o.dispatch(&to_hdmi()).await.unwrap();
        assert_eq!(m.confirmed(), [MonitorInput::Hdmi]);
        assert!(matches!(
            peer.sent.lock().unwrap()[..],
            [ControlMsg::Monitors(_)]
        ));
        o.dispatch(&to_hdmi()).await.unwrap_err();
        assert_eq!(m.confirmed(), [MonitorInput::Hdmi], "a failed verdict");

        // The same when the other machine asks for it.
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        let peer = Arc::new(NoticePeer::default());
        o.set_peer(peer.clone());
        let m = FakeMonitors::new(Some(Machine::A), vec![]);
        o.set_monitors(m.clone());
        o.execute_for_peer(&to_hdmi()).await.unwrap();
        assert_eq!(m.confirmed(), [MonitorInput::Hdmi]);
        assert!(matches!(
            peer.sent.lock().unwrap()[..],
            [ControlMsg::Monitors(_)]
        ));

        // The other owns: it is confirmed here when it answers, and nothing is sent.
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        let peer = Arc::new(NoticePeer::default());
        o.set_peer(peer.clone());
        let m = FakeMonitors::new(Some(Machine::B), vec![]);
        o.set_monitors(m.clone());
        o.dispatch(&to_hdmi()).await.unwrap();
        assert_eq!(peer.steps.steps(), [to_hdmi()]);
        assert_eq!(m.confirmed(), [MonitorInput::Hdmi]);
        assert!(peer.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_switch_without_verdict_goes_back_to_the_input_before() {
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        let m = FakeMonitors::new(
            Some(Machine::A),
            vec![Err(format!("{} en 12 s", tandem_monitor::NO_VERDICT))],
        );
        o.set_monitors(m.clone());
        let mut state = both_on();
        let plan = Plan::switch_monitor("m", MonitorInput::Hdmi);
        let err = o.run(1, &mut state, &plan).await.unwrap_err();
        assert!(matches!(err, TxnError::Failed { .. }));
        assert_eq!(
            m.switched(),
            [MonitorInput::Hdmi, MonitorInput::DisplayPort]
        );
    }

    /// Review of #80: without knowing the previous input, a change without a verdict would have nowhere to
    /// go back to. It is rejected before touching the monitor.
    #[tokio::test]
    async fn a_switch_with_an_unknown_input_is_refused() {
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        let m = Arc::new(FakeMonitors {
            owner: Some(Machine::A),
            direct: false,
            input: None,
            answer: Mutex::new(Vec::new()),
            switched: Mutex::new(Vec::new()),
            confirmed: Mutex::new(Vec::new()),
        });
        o.set_monitors(m.clone());
        let mut state = both_on();
        let plan = Plan::switch_monitor("m", MonitorInput::Hdmi);
        let TxnError::Failed { error, .. } = o.run(1, &mut state, &plan).await.unwrap_err();
        assert!(error.contains(UNKNOWN_INPUT), "{error}");
        assert!(m.switched().is_empty());
    }

    #[tokio::test]
    async fn a_failed_verdict_is_not_undone() {
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        let m = FakeMonitors::new(Some(Machine::A), vec![Err("still on DP".into())]);
        o.set_monitors(m.clone());
        let mut state = both_on();
        let plan = Plan::switch_monitor("m", MonitorInput::Hdmi);
        o.run(1, &mut state, &plan).await.unwrap_err();
        assert_eq!(m.switched(), [MonitorInput::Hdmi]);
    }

    #[tokio::test]
    async fn a_later_failure_undoes_the_switch() {
        let (o, _, _) = mac(
            FakeSteps::default(),
            FakeSteps {
                fail_on: vec![PC_OFF],
                ..Default::default()
            },
        );
        let m = FakeMonitors::new(Some(Machine::A), vec![]);
        o.set_monitors(m.clone());
        let mut state = both_on();
        let plan = Plan {
            steps: vec![to_hdmi(), PC_OFF],
        };
        o.run(1, &mut state, &plan).await.unwrap_err();
        assert_eq!(
            m.switched(),
            [MonitorInput::Hdmi, MonitorInput::DisplayPort]
        );
        assert!(state.on.b, "power is neither undone nor changed");
    }

    // QA round 2: if the shutdown reply was lost, the Mac took it as failed even though
    // the PC was already shutting down.
    #[tokio::test]
    async fn lost_reply_to_power_off_is_taken_as_done() {
        let (o, _, peer) = mac(
            FakeSteps::default(),
            FakeSteps {
                lost_reply_on: vec![PC_OFF],
                ..Default::default()
            },
        );
        let mut state = both_on();
        let plan = Plan::power_off(&state, Machine::B);
        let report = o.run(1, &mut state, &plan).await.unwrap();
        assert_eq!(report.applied, plan.steps);
        assert!(!state.on.b);
        assert_eq!(peer.steps(), [PC_OFF]);

        // A shutdown that fails for certain stays as it was.
        let (o, _, _) = mac(
            FakeSteps::default(),
            FakeSteps {
                fail_on: vec![PC_OFF],
                ..Default::default()
            },
        );
        let mut state = both_on();
        let before = state.clone();
        o.run(2, &mut state, &plan).await.unwrap_err();
        assert_eq!(state, before);
    }

    #[tokio::test(start_paused = true)]
    async fn a_power_off_that_hangs_is_taken_as_done() {
        let (o, _, _) = mac(
            FakeSteps::default(),
            FakeSteps {
                delays: vec![(PC_OFF, Duration::from_secs(3600))],
                ..Default::default()
            },
        );
        let mut state = both_on();
        let plan = Plan::power_off(&state, Machine::B);
        let start = tokio::time::Instant::now();
        o.run(3, &mut state, &plan).await.unwrap();
        assert_eq!(start.elapsed(), Duration::from_secs(10));
        assert!(!state.on.b);
    }
}
