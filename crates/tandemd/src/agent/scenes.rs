//! Las escenas en el agente (D6, DESIGN §5): aplicar una como transacción, la escena activa,
//! las escenas por omisión al emparejar y sus atajos globales.
//!
//! Crear, editar y borrar escenas son parches del puesto (`DeskPatch::AddScene`, …): viajan
//! a la otra máquina y se deshacen como cualquier otro cambio (D5).
//!
//! Los pasos de escena los hace la máquina que la aplica: el acomodo es un cambio del puesto
//! y lo hace el agente; el monitor abstracto y la geometría, el [`SceneBackend`]. Hasta T-111
//! no hay monitores de escena de verdad ([`NoSceneMonitors`]); la simulación pone su doble.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use tandem_core::{Config, DeskPatch, Machine, State, Step, default_scenes};
use tandem_input::{HidUsage, InputEvent, KeyState};
use tandem_proto::{Reply, UiCommand};
use tokio::sync::{mpsc, watch};

use super::Agent;
use crate::orchestrator::{SCENE_ONLY, SceneSteps, StepFuture, describe};

/// Lo que contesta el paso de monitor de una escena mientras no hay monitores de escena.
pub const NO_SCENE_MONITORS: &str = "los monitores todavía no se pueden asignar a una escena";

/// El monitor abstracto y la geometría de las escenas, vistos por el agente.
pub trait SceneBackend: Send + Sync + std::fmt::Debug {
    /// Qué máquina muestra `monitor` ahora. Si no se sabe, el monitor no se cambia: no se
    /// podría volver atrás.
    fn shows(&self, monitor: &str) -> Result<Machine, String>;
    /// Que `monitor` muestre `machine`, con su confirmación.
    fn show<'a>(&'a self, monitor: &'a str, machine: Machine) -> StepFuture<'a>;
    /// Que el motor de entrada tome la geometría que dejaron los pasos anteriores.
    fn geometry(&self) -> StepFuture<'_>;
}

/// Sin monitores de escena (hasta T-111): una escena con monitores falla en su primer
/// monitor, sin tocar nada. La geometría se cumple sola: el motor de entrada recalcula la
/// suya cada vez que cambian el puesto o el estado (`input::geometry`).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoSceneMonitors;

impl SceneBackend for NoSceneMonitors {
    fn shows(&self, _monitor: &str) -> Result<Machine, String> {
        Err(NO_SCENE_MONITORS.to_owned())
    }

    fn show<'a>(&'a self, _monitor: &'a str, _machine: Machine) -> StepFuture<'a> {
        Box::pin(async { Err(NO_SCENE_MONITORS.to_owned()) })
    }

    fn geometry(&self) -> StepFuture<'_> {
        Box::pin(async { Ok(()) })
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
            Step::Geometry => self.backend.geometry(),
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
    /// Quién hace el monitor abstracto y la geometría de las escenas (la simulación, T-111).
    pub fn set_scene_backend(&self, backend: Arc<dyn SceneBackend>) {
        *self
            .scene_backend
            .write()
            .unwrap_or_else(PoisonError::into_inner) = backend;
    }

    fn scene_backend(&self) -> Arc<dyn SceneBackend> {
        self.scene_backend
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
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
        let run = SceneRun {
            id: scene.id.clone(),
            steps: Arc::new(AgentScene {
                agent: self.clone(),
                backend: self.scene_backend(),
            }),
        };
        let what = format!("aplicar la escena «{}»", scene.name);
        self.start_txn(
            &what,
            move |from| scene.plan(from, desk.arrangement, &abilities),
            Some(run),
        )
    }

    /// El puesto cambió: la escena activa puede haber dejado de existir.
    pub(super) fn settle_scene(&self) {
        if let Err(e) = self.set_state(self.state()) {
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

/// Las escenas por omisión, si el puesto conoce a las dos máquinas y todavía no tiene
/// ninguna. `true` si las agregó.
pub(super) fn add_defaults(desk: &mut Config) -> bool {
    let both = Machine::ALL.iter().all(|&m| desk.machines.get(m).is_some());
    if !both || !desk.scenes.is_empty() {
        return false;
    }
    desk.scenes = default_scenes(&desk.machines, desk.arrangement);
    true
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
    fn defaults_only_once_and_only_with_both_machines() {
        let mut d = desk();
        assert_eq!(d.scenes.len(), 3);
        assert!(!add_defaults(&mut d), "ya tiene escenas");
        let mut half = Config::default();
        half.machines.a = d.machines.a.clone();
        assert!(!add_defaults(&mut half));
        assert!(half.scenes.is_empty());
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
