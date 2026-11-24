//! Scenes in the agent (D6, DESIGN §5): applying one as a transaction, the active scene,
//! the default scenes when pairing and their global shortcuts.
//!
//! Creating, editing and deleting scenes are desk patches (`DeskPatch::AddScene`, ...): they travel
//! to the other machine and are undone like any other change (D5). A new monitor in a
//! scene has to be compatible and it must be known which input the machine it shows goes to.
//!
//! Scene steps are done by the machine that applies it: the arrangement is a desk change
//! and is done by the agent; the abstract monitor and the geometry, the real monitors (T-111):
//! "make the 34" show B" is a [`Step::SwitchMonitor`] to B's input, done by the monitor's
//! owner with its verdict, and the geometry waits for the monitors to show what
//! the scene says (the input engine takes from the geometry what shows the other). T-100's
//! simulation can set its double ([`SceneBackend`]).
//!
//! With the monitors read, the agent also introduces each new compatible monitor to the
//! default scenes ([`tandem_core::scene::introduce_monitor`]) and recognizes the scene that
//! is set by each monitor's real input (on start and every time they change).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use tandem_core::learn_screens;
use tandem_core::scene::{contradicted, detect, introduce_monitor};
use tandem_core::{
    Config, DeskPatch, Machine, MonitorCable, MonitorId, MonitorInput, Plan, Scene, State, Step,
};
use tandem_input::{HidUsage, InputEvent, KeyState};
use tandem_proto::{Reply, UiCommand};
use tokio::sync::{mpsc, watch};

use super::Agent;
use crate::monitors::{inputs, shows};
use crate::orchestrator::{SCENE_ONLY, SceneSteps, StepFuture, describe};

/// It is not known which machine a monitor shows: without that it could not go back.
pub const UNKNOWN_SHOWS: &str = "it is not known which machine is shown (the real input, or which input each machine goes to, is missing); without that it could not go back";

/// A double of the monitor and geometry scene steps (T-100's simulation). Without it,
/// the agent uses the real monitors.
pub trait SceneBackend: Send + Sync + std::fmt::Debug {
    /// Which machine `monitor` shows now. If it is not known, the monitor is not changed: it
    /// could not go back.
    fn shows(&self, monitor: &str) -> Result<Machine, String>;
    /// Make `monitor` show `machine`, with its confirmation.
    fn show<'a>(&'a self, monitor: &'a str, machine: Machine) -> StepFuture<'a>;
    /// Have the input engine take the geometry the previous steps left: the scene's
    /// monitors showing what `expected` says.
    fn geometry<'a>(&'a self, expected: &'a [(MonitorId, Machine)]) -> StepFuture<'a>;
    /// Whether `monitor` can enter a scene showing `machine` (the editor).
    fn check(&self, monitor: &str, machine: Machine) -> Result<(), String>;
}

/// The real monitors: what each shows comes from the input its owner reads and from the
/// cabling; the change is done by the owner (the orchestrator).
#[derive(Debug)]
struct RealMonitors {
    agent: Arc<Agent>,
}

impl RealMonitors {
    fn input_for(&self, monitor: &str, machine: Machine) -> Result<MonitorInput, String> {
        let views = self.agent.monitors();
        let view = views.iter().find(|v| v.id == monitor);
        inputs(&self.agent.desk(), monitor, view)
            .of(machine)
            .ok_or_else(|| {
                format!(
                    "it is not known which input of monitor {monitor} {} goes to",
                    self.agent.machine_name(machine)
                )
            })
    }

    fn now(&self) -> std::collections::BTreeMap<String, Machine> {
        shows(&self.agent.monitors(), &self.agent.desk())
    }
}

impl SceneBackend for RealMonitors {
    fn shows(&self, monitor: &str) -> Result<Machine, String> {
        self.now()
            .get(monitor)
            .copied()
            .ok_or_else(|| UNKNOWN_SHOWS.to_owned())
    }

    fn show<'a>(&'a self, monitor: &'a str, machine: Machine) -> StepFuture<'a> {
        Box::pin(async move {
            // It is always requested, even if it seems it already shows it: on a rollback what was read
            // may predate the change that was left without a verdict. The monitors that already
            // show it leave the plan before starting ([`Agent::apply_scene`]).
            let input = self.input_for(monitor, machine)?;
            let step = Step::SwitchMonitor {
                monitor: monitor.to_owned(),
                input,
            };
            self.agent.orch.dispatch(&step).await
        })
    }

    fn geometry<'a>(&'a self, expected: &'a [(MonitorId, Machine)]) -> StepFuture<'a> {
        Box::pin(async move {
            // The owner already gave its verdict; here we wait to see it in what it reads (or in what
            // the other sends), which is where the input engine takes the geometry from. The maximum
            // time is set by the orchestrator.
            let mut views = self.agent.subscribe_monitors();
            loop {
                let now = shows(&views.borrow_and_update(), &self.agent.desk());
                if expected
                    .iter()
                    .all(|(m, machine)| now.get(m) == Some(machine))
                {
                    return Ok(());
                }
                if views.changed().await.is_err() {
                    return Err("the monitors were closed".to_owned());
                }
            }
        })
    }

    fn check(&self, monitor: &str, machine: Machine) -> Result<(), String> {
        self.agent.check_real_monitor(monitor, machine)
    }
}

/// A scene in progress: its id and who does its steps.
pub(super) struct SceneRun {
    pub id: String,
    pub steps: Arc<dyn SceneSteps>,
}

/// The scene steps of an agent transaction.
struct AgentScene {
    agent: Arc<Agent>,
    backend: Arc<dyn SceneBackend>,
    /// What the scene leaves on each monitor.
    expected: Vec<(MonitorId, Machine)>,
}

impl SceneSteps for AgentScene {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
        match step {
            Step::ShowMachine { monitor, machine } => self.backend.show(monitor, *machine),
            Step::Arrange { layout } => {
                let r = self
                    .agent
                    .edit_desk(DeskPatch::Arrangement(*layout))
                    .map(|_| ());
                Box::pin(async move { r })
            }
            Step::Geometry => self.backend.geometry(&self.expected),
            other => {
                let e = format!("{}: {SCENE_ONLY}", describe(other));
                Box::pin(async move { Err(e) })
            }
        }
    }

    fn inverse(&self, step: &Step) -> Result<Option<Step>, String> {
        Ok(match step {
            Step::ShowMachine { monitor, machine } => {
                let before = self
                    .backend
                    .shows(monitor)
                    .map_err(|e| format!("{}: {e}", describe(step)))?;
                (before != *machine).then(|| Step::ShowMachine {
                    monitor: monitor.clone(),
                    machine: before,
                })
            }
            Step::Arrange { layout } => {
                let before = self.agent.desk().arrangement;
                (before != *layout).then_some(Step::Arrange { layout: before })
            }
            _ => None,
        })
    }
}

impl Agent {
    /// A double of the scene monitors (T-100's simulation).
    pub fn set_scene_backend(&self, backend: Arc<dyn SceneBackend>) {
        *self
            .scene_backend
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(backend);
    }

    fn scene_backend(self: &Arc<Self>) -> Arc<dyn SceneBackend> {
        self.scene_backend
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| {
                Arc::new(RealMonitors {
                    agent: self.clone(),
                })
            })
    }

    /// The editor (T-111): each monitor `scene` adds (it was not in the version before
    /// that scene) has to be compatible and it must be known which input the machine it shows goes to.
    /// The ones that were already there are kept even if they are not seen now (a monitor that is off).
    pub(super) fn check_scene_monitors(&self, scene: &Scene) -> Result<(), String> {
        let desk = self.desk();
        let before: BTreeSet<&(MonitorId, Machine)> = desk
            .scenes
            .iter()
            .filter(|s| s.id == scene.id)
            .flat_map(|s| s.monitors.iter())
            .collect();
        let double = self
            .scene_backend
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for entry in scene.monitors.iter().filter(|e| !before.contains(e)) {
            let (monitor, machine) = entry;
            let r = match &double {
                Some(d) => d.check(monitor, *machine),
                None => self.check_real_monitor(monitor, *machine),
            };
            r.map_err(|e| format!("\"{}\": {e}", scene.name))?;
        }
        Ok(())
    }

    fn check_real_monitor(&self, monitor: &str, machine: Machine) -> Result<(), String> {
        let views = self.monitors();
        let Some(view) = views.iter().find(|v| v.id == monitor) else {
            return Err(format!("monitor {monitor} is not seen"));
        };
        if !view.compat.is_compatible() {
            return Err(format!(
                "{} is not compatible: it is shown, but it does not enter scenes",
                view.name
            ));
        }
        inputs(&self.desk(), monitor, Some(view))
            .of(machine)
            .map(|_| ())
            .ok_or_else(|| {
                format!(
                    "it is not known which input of {} {} goes to",
                    view.name,
                    self.machine_name(machine)
                )
            })
    }

    /// Apply scene `id` (`UiCommand::ApplyScene`): turn on what it needs -> each
    /// monitor -> the arrangement -> the geometry, with rollback. On success it becomes the
    /// state's active scene.
    pub(super) fn apply_scene(self: &Arc<Self>, id: &str) -> Reply {
        let desk = self.desk();
        let Some(scene) = desk.scenes.iter().find(|s| s.id == id).cloned() else {
            return Reply::Rejected(format!("scene `{id}` does not exist"));
        };
        // Preflight (T-349): a monitor step to a machine whose cable is unplugged would leave a
        // black screen. Refused before anything is done, so there is nothing to roll back.
        if let Some(why) = scene
            .monitors
            .iter()
            .find_map(|(m, machine)| self.unplugged(m, *machine))
        {
            return Reply::Rejected(format!("scene \"{}\" cannot be applied: {why}", scene.name));
        }
        let abilities = self.abilities();
        let backend = self.scene_backend();
        let run = SceneRun {
            id: scene.id.clone(),
            steps: Arc::new(AgentScene {
                agent: self.clone(),
                backend: backend.clone(),
                expected: scene.monitors.clone(),
            }),
        };
        let what = format!("apply scene \"{}\"", scene.name);
        self.start_txn(
            &what,
            move |from| {
                let plan = scene.plan(from, desk.arrangement, &abilities)?;
                Ok(skip_shown(plan, |m| backend.shows(m).ok()))
            },
            Some(run),
        )
    }

    /// The desk changed: the active scene may have ceased to exist.
    pub(super) fn settle_scene(&self) {
        if let Err(e) = self.set_state(self.state()) {
            tracing::warn!("{e}");
        }
    }

    /// What the monitors read changed (T-111): the new compatible ones are introduced and the
    /// active scene comes from each one's real input.
    pub(super) fn reconcile_monitors(&self) {
        self.monitors.set_desk(&self.desk());
        self.introduce_monitors();
        self.learn_screens();
        self.recognize_scene();
    }

    /// This machine's end of each cable gets the screen the monitor is here (T-340), from the
    /// EDID identity of the monitor (channel or not) and of each screen. An engine id that no
    /// longer matches (the screens were renumbered) is replaced; a name a person gave is kept. Not with `[[screens]]` declared
    /// for this machine: their ids are the user's, not the `screen-N` the engine reads.
    fn learn_screens(&self) {
        let me = self.machine();
        let links = self.monitors.screen_links();
        let pending = |d: &Config| {
            d.screens_of(me).next().is_none()
                && d.cables.iter().any(|c| {
                    links
                        .get(&c.monitor)
                        .is_some_and(|s| tandem_core::screen_outdated(c, me, s))
                })
        };
        if links.is_empty() || !pending(&self.desk()) {
            return;
        }
        let _editing = self.edit();
        let mut desk = self.desk();
        if !pending(&desk) || !learn_screens(&mut desk.cables, me, &links) {
            return;
        }
        if let Err(e) = desk.validate() {
            tracing::warn!("the learned screens would leave the desk invalid: {e}");
            return;
        }
        tracing::info!(?links, "learned which screen each monitor is");
        self.desk.send_replace(desk);
        self.bump_desk();
    }

    /// Each compatible monitor this machine owns, without cabling in the desk and with
    /// both inputs known (from the EDID), is recorded and enters the default scenes. Only
    /// the owner does it, so the two machines do not do it at once.
    fn introduce_monitors(&self) {
        let me = self.machine();
        let views = self.monitors();
        let paired = |d: &Config| Machine::ALL.iter().all(|&m| d.machines.get(m).is_some());
        let pending: Vec<_> = {
            let desk = self.desk();
            if !paired(&desk) {
                return;
            }
            views
                .iter()
                .filter(|v| v.owner == Some(me) && v.compat.is_compatible())
                .filter(|v| desk.cable(&v.id).is_none())
                .filter(|v| inputs(&desk, &v.id, Some(v)).complete())
                .collect()
        };
        if pending.is_empty() {
            return;
        }
        let _editing = self.edit();
        let mut desk = self.desk();
        let now = shows(&views, &desk);
        let mut changed = false;
        for v in pending {
            let found = inputs(&desk, &v.id, Some(v));
            let mut cable = MonitorCable::new(v.id.clone());
            cable.a.input = found.a;
            cable.b.input = found.b;
            if introduce_monitor(&mut desk, cable, now.get(&v.id).copied()) {
                tracing::info!(monitor = %v.id, "new monitor in the default scenes");
                changed = true;
            }
        }
        if changed {
            self.desk.send_replace(desk);
            self.bump_desk();
        }
    }

    /// The active scene according to the monitors: if any shows something else, it stops being so; without an
    /// active scene, the one that is set ([`detect`]). Outside a transaction: midway,
    /// the monitors say nothing.
    fn recognize_scene(&self) {
        if !self.orch.is_idle() {
            return;
        }
        let desk = self.desk();
        let now = shows(&self.monitors(), &desk);
        if now.is_empty() {
            return;
        }
        let mut state = self.state();
        let active = state
            .scene
            .as_ref()
            .and_then(|id| desk.scenes.iter().find(|s| &s.id == id));
        if let Some(scene) = active {
            if !contradicted(scene, &now) {
                return;
            }
            tracing::info!(scene = %scene.id, "the monitors no longer show the active scene");
            state.scene = None;
        }
        state.scene = detect(&desk, &state, &now).map(|s| s.id.clone());
        if let Some(id) = &state.scene {
            tracing::info!(scene = %id, "scene recognized by the monitors");
        }
        if let Err(e) = self.set_state(state) {
            tracing::warn!("{e}");
        }
    }

    /// The scenes' shortcuts for this machine's keyboard capture.
    pub fn scene_shortcuts(&self, fired: mpsc::UnboundedSender<String>) -> SceneShortcuts {
        SceneShortcuts::new(self.subscribe_desk(), fired)
    }

    /// Applies the scenes whose shortcuts were pressed (what [`SceneShortcuts`] sends).
    pub async fn serve_scene_shortcuts(
        self: Arc<Self>,
        mut fired: mpsc::UnboundedReceiver<String>,
    ) {
        while let Some(id) = fired.recv().await {
            tracing::info!(scene = %id, "scene shortcut");
            if let Reply::Rejected(why) = self.handle(UiCommand::ApplyScene { id }) {
                tracing::warn!("the shortcut's scene was not applied: {why}");
            }
        }
    }
}

/// Removes from the plan the monitors that already show their machine (`shows`): setting the same
/// input again would only blank the image for a moment. If no monitor or arrangement is left, the
/// geometry has nothing to take either.
fn skip_shown(mut plan: Plan, shows: impl Fn(&str) -> Option<Machine>) -> Plan {
    plan.steps.retain(|s| match s {
        Step::ShowMachine { monitor, machine } => shows(monitor) != Some(*machine),
        _ => true,
    });
    let changes = plan
        .steps
        .iter()
        .any(|s| matches!(s, Step::ShowMachine { .. } | Step::Arrange { .. }));
    if !changes {
        plan.steps.retain(|s| *s != Step::Geometry);
    }
    plan
}

/// The default scenes, if the desk knows both machines and does not yet have
/// any ([`tandem_core::scene::add_defaults`]). `true` if it added them.
pub(super) fn add_defaults(desk: &mut Config) -> bool {
    tandem_core::scene::add_defaults(desk)
}

/// The active scene ceases to hold if it is no longer in the desk or if a machine was turned off that
/// needs it.
pub(super) fn settle(state: &mut State, desk: &Config) {
    let holds = state
        .scene
        .as_ref()
        .is_some_and(|id| desk.scenes.iter().any(|s| &s.id == id && s.holds(state)));
    if !holds {
        state.scene = None;
    }
}

/// A key, with the right and left modifiers as one and the three keys that
/// macOS delivers as F13-F15 as the PC ones (like the lock shortcut).
fn canonical(usage: HidUsage) -> HidUsage {
    const MAC_ALIASES: [(u16, u16); 3] = [(0x68, 0x46), (0x69, 0x47), (0x6A, 0x48)];
    if usage.page != tandem_input::event::PAGE_KEYBOARD {
        return usage;
    }
    let id = match usage.id {
        0xE4..=0xE7 => usage.id - 4,
        id => MAC_ALIASES
            .iter()
            .find_map(|&(mac, pc)| (mac == id).then_some(pc))
            .unwrap_or(id),
    };
    HidUsage::key(id)
}

/// Which scene each key combination triggers, pure: it carries what is pressed and says which
/// scene was completed with the last key. Keys continue on their way (like the
/// lock shortcut): the shortcut eats nothing.
#[derive(Debug, Default, Clone)]
pub struct ShortcutMatcher {
    bindings: Vec<(String, BTreeSet<HidUsage>)>,
    pressed: BTreeSet<HidUsage>,
}

impl ShortcutMatcher {
    /// The desk scenes' shortcuts. One with an unknown key is ignored.
    pub fn for_desk(desk: &Config) -> Self {
        let bindings = desk
            .scenes
            .iter()
            .filter(|s| !s.shortcut.is_empty())
            .filter_map(|s| {
                let keys = crate::config::shortcut_keys(&s.shortcut).ok()?;
                Some((s.id.clone(), keys.into_iter().map(canonical).collect()))
            })
            .collect();
        ShortcutMatcher {
            bindings,
            pressed: BTreeSet::new(),
        }
    }

    /// Changes the shortcuts without forgetting what is pressed.
    pub fn rebind(&mut self, desk: &Config) {
        self.bindings = Self::for_desk(desk).bindings;
    }

    /// A key pressed or released; `Some(id)` if it completed that scene's shortcut.
    /// Auto-repeat does not trigger again.
    pub fn key(&mut self, usage: HidUsage, state: KeyState) -> Option<String> {
        let key = canonical(usage);
        match state {
            KeyState::Released => {
                self.pressed.remove(&key);
                None
            }
            KeyState::Pressed => {
                if !self.pressed.insert(key) {
                    return None;
                }
                // Exactly those keys: ⌃⌥1 does not trigger with ⌃⌥⇧1.
                self.bindings
                    .iter()
                    .find(|(_, keys)| keys.contains(&key) && *keys == self.pressed)
                    .map(|(id, _)| id.clone())
            }
        }
    }
}

/// The scene shortcuts inside the keyboard capture: they watch each physical key of this
/// machine (before remapping modifiers) and send the id of the scene to apply. They follow the
/// desk live.
#[derive(Clone)]
pub struct SceneShortcuts(Arc<Mutex<Shortcuts>>);

struct Shortcuts {
    desk: watch::Receiver<Config>,
    matcher: ShortcutMatcher,
    fired: mpsc::UnboundedSender<String>,
}

impl SceneShortcuts {
    pub fn new(mut desk: watch::Receiver<Config>, fired: mpsc::UnboundedSender<String>) -> Self {
        let matcher = ShortcutMatcher::for_desk(&desk.borrow_and_update());
        SceneShortcuts(Arc::new(Mutex::new(Shortcuts {
            desk,
            matcher,
            fired,
        })))
    }

    /// What was captured, as it arrives from the capture.
    pub fn observe(&self, event: &InputEvent) {
        let InputEvent::Key { usage, state } = *event else {
            return;
        };
        let mut s = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if s.desk.has_changed().unwrap_or(false) {
            let desk = s.desk.borrow_and_update().clone();
            s.matcher.rebind(&desk);
        }
        if let Some(id) = s.matcher.key(usage, state) {
            // With nobody to apply (the engine is closing) there is nothing to do.
            let _ = s.fired.send(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use tandem_core::{MachineInfo, Os};

    use super::*;

    fn desk() -> Config {
        let mut d = Config::default();
        d.machines.a = Some(MachineInfo::new(Machine::A, Os::MacOs, None));
        d.machines.b = Some(MachineInfo::new(Machine::B, Os::Windows, None));
        assert!(add_defaults(&mut d));
        d
    }

    const CTRL: HidUsage = HidUsage::key(0xE0);
    const RCTRL: HidUsage = HidUsage::key(0xE4);
    const ALT: HidUsage = HidUsage::key(0xE2);
    const SHIFT: HidUsage = HidUsage::key(0xE1);
    const ONE: HidUsage = HidUsage::key(0x1E);
    const TWO: HidUsage = HidUsage::key(0x1F);

    #[test]
    fn monitors_already_showing_their_machine_leave_the_plan() {
        let show = |m: &str, machine| Step::ShowMachine {
            monitor: m.into(),
            machine,
        };
        let plan = Plan {
            steps: vec![
                Step::PowerOn {
                    machine: Machine::B,
                },
                show("34", Machine::B),
                show("27", Machine::B),
                Step::Geometry,
            ],
        };
        let shows = |m: &str| (m == "34").then_some(Machine::B);
        assert_eq!(
            skip_shown(plan.clone(), shows).steps,
            [
                plan.steps[0].clone(),
                show("27", Machine::B),
                Step::Geometry
            ]
        );
        // With nothing to change, neither the geometry; without knowing what it shows, it is requested.
        let all = |_: &str| Some(Machine::B);
        assert_eq!(skip_shown(plan.clone(), all).steps, plan.steps[..1]);
        assert_eq!(skip_shown(plan.clone(), |_| None), plan);
    }

    #[test]
    fn the_active_scene_stops_holding() {
        let d = desk();
        let mut s = State::initial(Machine::A);
        s.on.b = true;
        s.scene = Some("work".into());
        settle(&mut s, &d);
        assert_eq!(s.scene.as_deref(), Some("work"));
        // The other powered off: the scene that uses both needs both.
        s.on.b = false;
        settle(&mut s, &d);
        assert_eq!(s.scene, None);
        // The single-machine scene still holds with the other off; deleted, no.
        s.scene = Some("only-a".into());
        settle(&mut s, &d);
        assert_eq!(s.scene.as_deref(), Some("only-a"));
        settle(&mut s, &Config::default());
        assert_eq!(s.scene, None);
    }

    #[test]
    fn a_shortcut_fires_once_with_exactly_its_keys() {
        let mut m = ShortcutMatcher::for_desk(&desk());
        assert_eq!(m.key(CTRL, KeyState::Pressed), None);
        assert_eq!(m.key(ALT, KeyState::Pressed), None);
        assert_eq!(m.key(ONE, KeyState::Pressed).as_deref(), Some("work"));
        // Auto-repeat does not apply again.
        assert_eq!(m.key(ONE, KeyState::Pressed), None);
        assert_eq!(m.key(ONE, KeyState::Released), None);
        assert_eq!(m.key(TWO, KeyState::Pressed).as_deref(), Some("only-a"));
        m.key(TWO, KeyState::Released);
        // With one more key, no.
        m.key(SHIFT, KeyState::Pressed);
        assert_eq!(m.key(ONE, KeyState::Pressed), None);
        for k in [SHIFT, ONE, ALT, CTRL] {
            m.key(k, KeyState::Released);
        }
        // The right Ctrl counts like the left, and order does not matter.
        m.key(ONE, KeyState::Pressed);
        m.key(ALT, KeyState::Pressed);
        assert_eq!(m.key(RCTRL, KeyState::Pressed).as_deref(), Some("work"));
    }

    #[test]
    fn shortcuts_follow_the_desk_live() {
        let (desk_tx, desk_rx) = watch::channel(desk());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let keys = SceneShortcuts::new(desk_rx, tx);
        let press = |k| InputEvent::Key {
            usage: k,
            state: KeyState::Pressed,
        };
        let release = |k| InputEvent::Key {
            usage: k,
            state: KeyState::Released,
        };
        keys.observe(&press(CTRL));
        keys.observe(&press(ALT));
        keys.observe(&press(TWO));
        assert_eq!(rx.try_recv().as_deref(), Ok("only-a"));
        keys.observe(&release(TWO));
        // The user removes the shortcut from the single-machine scene from the other machine.
        desk_tx.send_modify(|d| d.scenes[1].shortcut.clear());
        keys.observe(&press(TWO));
        assert!(rx.try_recv().is_err());
    }
}
