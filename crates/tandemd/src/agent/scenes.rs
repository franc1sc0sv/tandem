//! Las escenas en el agente (D6, DESIGN §5): aplicar una como transacción, la escena activa,
//! las escenas por omisión al emparejar y sus atajos globales.
//!
//! Crear, editar y borrar escenas son parches del puesto (`DeskPatch::AddScene`, …): viajan
//! a la otra máquina y se deshacen como cualquier otro cambio (D5). Un monitor nuevo en una
//! escena tiene que ser compatible y saberse a qué entrada va la máquina que muestra.
//!
//! Los pasos de escena los hace la máquina que la aplica: el acomodo es un cambio del puesto
//! y lo hace el agente; el monitor abstracto y la geometría, los monitores reales (T-111):
//! «que el 34" muestre B» es un [`Step::SwitchMonitor`] a la entrada de B, que hace el dueño
//! del monitor con su veredicto, y la geometría espera a que los monitores muestren lo que
//! dice la escena (el motor de entrada saca de la geometría lo que muestra a la otra). La
//! simulación de T-100 puede poner su doble ([`SceneBackend`]).
//!
//! Con los monitores leídos, el agente también presenta cada monitor compatible nuevo a las
//! escenas por omisión ([`tandem_core::scene::introduce_monitor`]) y reconoce la escena que
//! está puesta por la entrada real de cada monitor (al arrancar y cada vez que cambian).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

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

/// No se sabe qué máquina muestra un monitor: sin eso no se podría volver atrás.
pub const UNKNOWN_SHOWS: &str = "no se sabe qué máquina muestra (falta la entrada real o a qué entrada va cada máquina); sin eso no se podría volver atrás";

/// Un doble de los pasos de escena de monitor y geometría (la simulación de T-100). Sin él,
/// el agente usa los monitores reales.
pub trait SceneBackend: Send + Sync + std::fmt::Debug {
    /// Qué máquina muestra `monitor` ahora. Si no se sabe, el monitor no se cambia: no se
    /// podría volver atrás.
    fn shows(&self, monitor: &str) -> Result<Machine, String>;
    /// Que `monitor` muestre `machine`, con su confirmación.
    fn show<'a>(&'a self, monitor: &'a str, machine: Machine) -> StepFuture<'a>;
    /// Que el motor de entrada tome la geometría que dejaron los pasos anteriores: los
    /// monitores de la escena mostrando lo que dice `expected`.
    fn geometry<'a>(&'a self, expected: &'a [(MonitorId, Machine)]) -> StepFuture<'a>;
    /// Si `monitor` puede entrar en una escena mostrando `machine` (el editor).
    fn check(&self, monitor: &str, machine: Machine) -> Result<(), String>;
}

/// Los monitores reales: lo que muestra cada uno sale de la entrada que lee su dueño y del
/// cableado; el cambio lo hace el dueño (el orquestador).
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
                    "no se sabe a qué entrada del monitor {monitor} va {}",
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
            // Siempre se pide, aunque parezca que ya la muestra: en una vuelta atrás lo leído
            // puede ser de antes del cambio que quedó sin veredicto. Los monitores que ya la
            // muestran salen del plan antes de empezar ([`Agent::apply_scene`]).
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
            // El dueño ya dio su veredicto; aquí se espera a verlo en lo que lee (o en lo que
            // manda la otra), que es de donde el motor de entrada saca la geometría. El tiempo
            // máximo lo pone el orquestador.
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
                    return Err("los monitores se cerraron".to_owned());
                }
            }
        })
    }

    fn check(&self, monitor: &str, machine: Machine) -> Result<(), String> {
        self.agent.check_real_monitor(monitor, machine)
    }
}

/// Una escena en curso: su id y quién hace sus pasos.
pub(super) struct SceneRun {
    pub id: String,
    pub steps: Arc<dyn SceneSteps>,
}

/// Los pasos de escena de una transacción del agente.
struct AgentScene {
    agent: Arc<Agent>,
    backend: Arc<dyn SceneBackend>,
    /// Lo que la escena deja en cada monitor.
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
    /// Un doble de los monitores de escena (la simulación de T-100).
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

    /// El editor (T-111): cada monitor que `scene` agrega (no estaba en la versión de antes de
    /// esa escena) tiene que ser compatible y saberse a qué entrada va la máquina que muestra.
    /// Los que ya estaban se dejan aunque ahora no se vean (un monitor apagado).
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
            r.map_err(|e| format!("«{}»: {e}", scene.name))?;
        }
        Ok(())
    }

    fn check_real_monitor(&self, monitor: &str, machine: Machine) -> Result<(), String> {
        let views = self.monitors();
        let Some(view) = views.iter().find(|v| v.id == monitor) else {
            return Err(format!("no se ve el monitor {monitor}"));
        };
        if !view.compat.is_compatible() {
            return Err(format!(
                "el {} no es compatible: se muestra, pero no entra en escenas",
                view.name
            ));
        }
        inputs(&self.desk(), monitor, Some(view))
            .of(machine)
            .map(|_| ())
            .ok_or_else(|| {
                format!(
                    "no se sabe a qué entrada del {} va {}",
                    view.name,
                    self.machine_name(machine)
                )
            })
    }

    /// Aplicar la escena `id` (`UiCommand::ApplyScene`): encender lo que necesite → cada
    /// monitor → el acomodo → la geometría, con vuelta atrás. Al terminar bien queda como la
    /// escena activa del estado.
    pub(super) fn apply_scene(self: &Arc<Self>, id: &str) -> Reply {
        let desk = self.desk();
        let Some(scene) = desk.scenes.iter().find(|s| s.id == id).cloned() else {
            return Reply::Rejected(format!("la escena `{id}` no existe"));
        };
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
        let what = format!("aplicar la escena «{}»", scene.name);
        self.start_txn(
            &what,
            move |from| {
                let plan = scene.plan(from, desk.arrangement, &abilities)?;
                Ok(skip_shown(plan, |m| backend.shows(m).ok()))
            },
            Some(run),
        )
    }

    /// El puesto cambió: la escena activa puede haber dejado de existir.
    pub(super) fn settle_scene(&self) {
        if let Err(e) = self.set_state(self.state()) {
            tracing::warn!("{e}");
        }
    }

    /// Lo que leen los monitores cambió (T-111): se presentan los compatibles nuevos y la
    /// escena activa sale de la entrada real de cada uno.
    pub(super) fn reconcile_monitors(&self) {
        self.introduce_monitors();
        self.recognize_scene();
    }

    /// Cada monitor compatible del que esta máquina es dueña, sin cableado en el puesto y con
    /// las dos entradas sabidas (por el EDID), se anota y entra en las escenas por omisión. Lo
    /// hace solo el dueño, para que las dos máquinas no lo hagan a la vez.
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
                tracing::info!(monitor = %v.id, "monitor nuevo en las escenas por omisión");
                changed = true;
            }
        }
        if changed {
            self.desk.send_replace(desk);
            self.bump_desk();
        }
    }

    /// La escena activa según los monitores: si alguno muestra otra cosa, deja de serlo; sin
    /// escena activa, la que esté puesta ([`detect`]). Fuera de una transacción: a medias,
    /// los monitores no dicen nada.
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
            tracing::info!(escena = %scene.id, "los monitores ya no muestran la escena activa");
            state.scene = None;
        }
        state.scene = detect(&desk, &state, &now).map(|s| s.id.clone());
        if let Some(id) = &state.scene {
            tracing::info!(escena = %id, "escena reconocida por los monitores");
        }
        if let Err(e) = self.set_state(state) {
            tracing::warn!("{e}");
        }
    }

    /// Los atajos de las escenas para la captura de teclado de esta máquina.
    pub fn scene_shortcuts(&self, fired: mpsc::UnboundedSender<String>) -> SceneShortcuts {
        SceneShortcuts::new(self.subscribe_desk(), fired)
    }

    /// Aplica las escenas cuyos atajos se apretaron (lo que manda [`SceneShortcuts`]).
    pub async fn serve_scene_shortcuts(
        self: Arc<Self>,
        mut fired: mpsc::UnboundedReceiver<String>,
    ) {
        while let Some(id) = fired.recv().await {
            tracing::info!(escena = %id, "atajo de escena");
            if let Reply::Rejected(why) = self.handle(UiCommand::ApplyScene { id }) {
                tracing::warn!("la escena del atajo no se aplicó: {why}");
            }
        }
    }
}

/// Saca del plan los monitores que ya muestran su máquina (`shows`): volver a poner la misma
/// entrada solo apagaría la imagen un momento. Si no queda ningún monitor ni acomodo, la
/// geometría tampoco tiene nada que tomar.
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

/// Las escenas por omisión, si el puesto conoce a las dos máquinas y todavía no tiene
/// ninguna ([`tandem_core::scene::add_defaults`]). `true` si las agregó.
pub(super) fn add_defaults(desk: &mut Config) -> bool {
    tandem_core::scene::add_defaults(desk)
}

/// La escena activa deja de valer si ya no está en el puesto o si se apagó una máquina que
/// necesita.
pub(super) fn settle(state: &mut State, desk: &Config) {
    let holds = state
        .scene
        .as_ref()
        .is_some_and(|id| desk.scenes.iter().any(|s| &s.id == id && s.holds(state)));
    if !holds {
        state.scene = None;
    }
}

/// Una tecla, con los modificadores derecho e izquierdo como una sola y las tres teclas que
/// macOS entrega como F13–F15 como las de PC (como el atajo de bloqueo).
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

/// Qué escena dispara cada combinación de teclas, puro: lleva lo presionado y dice qué
/// escena se completó con la última tecla. Las teclas siguen su camino (como el atajo de
/// bloqueo): el atajo no se come nada.
#[derive(Debug, Default, Clone)]
pub struct ShortcutMatcher {
    bindings: Vec<(String, BTreeSet<HidUsage>)>,
    pressed: BTreeSet<HidUsage>,
}

impl ShortcutMatcher {
    /// Los atajos de las escenas del puesto. Uno con una tecla desconocida se ignora.
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

    /// Cambia los atajos sin olvidar lo que está presionado.
    pub fn rebind(&mut self, desk: &Config) {
        self.bindings = Self::for_desk(desk).bindings;
    }

    /// Una tecla presionada o soltada; `Some(id)` si completó el atajo de esa escena. La
    /// repetición automática no vuelve a disparar.
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
                // Exactamente esas teclas: ⌃⌥1 no dispara con ⌃⌥⇧1.
                self.bindings
                    .iter()
                    .find(|(_, keys)| keys.contains(&key) && *keys == self.pressed)
                    .map(|(id, _)| id.clone())
            }
        }
    }
}

/// Los atajos de escena dentro de la captura de teclado: miran cada tecla física de esta
/// máquina (antes de reasignar modificadores) y mandan el id de la escena a aplicar. Siguen al
/// puesto en caliente.
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

    /// Lo capturado, tal como llega de la captura.
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
            // Sin nadie que aplique (el motor se está cerrando) no hay nada que hacer.
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
        // Sin nada que cambiar, tampoco la geometría; sin saber qué muestra, se pide.
        let all = |_: &str| Some(Machine::B);
        assert_eq!(skip_shown(plan.clone(), all).steps, plan.steps[..1]);
        assert_eq!(skip_shown(plan.clone(), |_| None), plan);
    }

    #[test]
    fn the_active_scene_stops_holding() {
        let d = desk();
        let mut s = State::initial(Machine::A);
        s.on.b = true;
        s.scene = Some("trabajo".into());
        settle(&mut s, &d);
        assert_eq!(s.scene.as_deref(), Some("trabajo"));
        // Se apagó la otra: «Trabajo» necesita las dos.
        s.on.b = false;
        settle(&mut s, &d);
        assert_eq!(s.scene, None);
        // «Solo A» sigue valiendo con la otra apagada; borrada, no.
        s.scene = Some("solo-a".into());
        settle(&mut s, &d);
        assert_eq!(s.scene.as_deref(), Some("solo-a"));
        settle(&mut s, &Config::default());
        assert_eq!(s.scene, None);
    }

    #[test]
    fn a_shortcut_fires_once_with_exactly_its_keys() {
        let mut m = ShortcutMatcher::for_desk(&desk());
        assert_eq!(m.key(CTRL, KeyState::Pressed), None);
        assert_eq!(m.key(ALT, KeyState::Pressed), None);
        assert_eq!(m.key(ONE, KeyState::Pressed).as_deref(), Some("trabajo"));
        // La repetición automática no vuelve a aplicar.
        assert_eq!(m.key(ONE, KeyState::Pressed), None);
        assert_eq!(m.key(ONE, KeyState::Released), None);
        assert_eq!(m.key(TWO, KeyState::Pressed).as_deref(), Some("solo-a"));
        m.key(TWO, KeyState::Released);
        // Con una tecla de más, no.
        m.key(SHIFT, KeyState::Pressed);
        assert_eq!(m.key(ONE, KeyState::Pressed), None);
        for k in [SHIFT, ONE, ALT, CTRL] {
            m.key(k, KeyState::Released);
        }
        // El Ctrl derecho vale como el izquierdo, y el orden no importa.
        m.key(ONE, KeyState::Pressed);
        m.key(ALT, KeyState::Pressed);
        assert_eq!(m.key(RCTRL, KeyState::Pressed).as_deref(), Some("trabajo"));
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
        assert_eq!(rx.try_recv().as_deref(), Ok("solo-a"));
        keys.observe(&release(TWO));
        // El usuario le quita el atajo a «Solo A» desde la otra máquina.
        desk_tx.send_modify(|d| d.scenes[1].shortcut.clear());
        keys.observe(&press(TWO));
        assert!(rx.try_recv().is_err());
    }
}
