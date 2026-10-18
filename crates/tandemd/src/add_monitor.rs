//! «Agregar monitor» (T-080): los monitores que ve el bus DDC de la PC, si tienen perfil,
//! y la calibración de los que no (el motor de T-077 con la confirmación de T-078).
//!
//! Al terminar bien:
//! - el perfil se guarda como perfil del usuario (`<datos>/profiles/ddc/<PNP>.toml`, T-079)
//!   y se ofrece para exportarlo;
//! - si el monitor está en la configuración, sus códigos pasan a [`LiveDesk`], que es lo
//!   que leen los pasos de pantalla: el siguiente cambio de modo ya los usa, sin reiniciar
//!   el agente; y se escriben en `tandem.toml` ([`crate::config_edit`]) para el próximo
//!   arranque.
//!
//! Mientras dura una calibración el orquestador queda reservado: un cambio de modo a la
//! vez movería el monitor que se está probando. La calibración corre en un hilo propio
//! con su runtime: cada operación DDC bloquea decenas de ms, y las sondas van por canales
//! que no dependen del runtime.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use tandem_core::{Config, DdcProfile, Input, Machine, MonitorConfig, MonitorId};
use tandem_display::calibrate::{
    self, CalibratedProfile, CalibrationError, CalibrationEvent, CalibrationOptions,
    CalibrationRequest, CancelToken, Sight, State as EngineState, profile,
};
use tandem_display::ddc::profiles::{ProfileDb, ProfileSource};
use tandem_display::ddc::{DdcBus, DdcError, MonitorInfo};
use tandem_proto::{
    CalibrationOutcome, CalibrationStep, CalibrationUpdate, DisplayInfo, DisplayProfile,
    SavedProfile,
};

use crate::agent::{Agent, BUSY};
use crate::calibration::AgentConfirmer;
use crate::config_edit;

/// Lo que la UI recibe en la Mac, que no tiene DDC: las entradas las cambia la PC.
pub const NOT_HERE: &str = "los monitores se calibran desde la PC: abre «Agregar monitor» allí";
pub const ALREADY: &str = "ya hay una calibración en curso";
pub const NOT_RUNNING: &str = "no hay ninguna calibración en curso";

/// Abre el bus DDC para calibrar (en el hilo de la calibración).
pub type CalibrationBus = Arc<dyn Fn() -> Result<Box<dyn DdcBus + Send>, DdcError> + Send + Sync>;

/// Lo que cambia en caliente por encima de `tandem.toml`: los códigos DDC aprendidos al
/// calibrar, el cableado elegido en la UI (T-082) y el puesto que llegó de la otra máquina
/// (T-084). Los pasos de pantalla y los planes lo leen en cada cambio de modo, así que vale
/// sin reiniciar el agente.
#[derive(Debug, Clone, Default)]
pub struct LiveDesk(Arc<RwLock<Overlay>>);

#[derive(Debug, Default)]
struct Overlay {
    /// El puesto entero que reemplaza al de `tandem.toml` (sincronizado con la otra máquina).
    base: Option<Config>,
    learned: BTreeMap<MonitorId, Learned>,
    wiring: BTreeMap<MonitorId, BTreeMap<Machine, Input>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Learned {
    ddc: DdcProfile,
    /// Para encontrarlo en el bus si la configuración no tenía `edid_model`.
    edid_model: Option<String>,
}

impl LiveDesk {
    pub fn learn(&self, monitor: MonitorId, ddc: DdcProfile, edid_model: Option<String>) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .learned
            .insert(monitor, Learned { ddc, edid_model });
    }

    /// El cableado nuevo de `monitor`.
    pub fn rewire(&self, monitor: MonitorId, wiring: BTreeMap<Machine, Input>) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .wiring
            .insert(monitor, wiring);
    }

    /// `monitor` con lo aprendido: los códigos nuevos, el modelo si no tenía y el cableado.
    pub fn apply(&self, monitor: &mut MonitorConfig) {
        let overlay = self.0.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(l) = overlay.learned.get(&monitor.id) {
            monitor.ddc = Some(l.ddc.clone());
            if monitor.edid_model.is_none() {
                monitor.edid_model = l.edid_model.clone();
            }
        }
        if let Some(w) = overlay.wiring.get(&monitor.id) {
            monitor.wiring = w.clone();
        }
    }

    /// Reemplaza el puesto entero (el que llegó de la otra máquina): lo aprendido antes ya
    /// viene en él.
    pub fn replace(&self, desk: Config) {
        let mut overlay = self.0.write().unwrap_or_else(PoisonError::into_inner);
        overlay.base = Some(desk);
        overlay.learned.clear();
        overlay.wiring.clear();
    }

    /// El puesto con lo aprendido.
    pub fn desk(&self, desk: &Config) -> Config {
        let base = self
            .0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .base
            .clone();
        let mut desk = base.unwrap_or_else(|| desk.clone());
        desk.monitors.iter_mut().for_each(|m| self.apply(m));
        desk
    }
}

/// El monitor de la configuración que corresponde al `index` del bus: el único cuyo
/// `edid_model` coincide con ese monitor y con ningún otro.
pub fn bind(desk: &Config, monitors: &[MonitorInfo], index: usize) -> Option<MonitorId> {
    let mut found = desk.monitors.iter().filter(|m| {
        let hits: Vec<usize> = monitors
            .iter()
            .filter(|i| m.models().any(|model| i.matches(model)))
            .map(|i| i.index)
            .collect();
        hits == [index]
    });
    let first = found.next()?;
    found.next().is_none().then(|| first.id.clone())
}

/// El servicio de «Agregar monitor» de la PC.
pub struct MonitorWizard {
    open: CalibrationBus,
    /// `<datos>/profiles/ddc/`; sin él no se guardan perfiles.
    user_dir: Option<PathBuf>,
    /// `tandem.toml`; sin él lo aprendido dura hasta reiniciar el agente.
    config: Option<PathBuf>,
    live: LiveDesk,
    options: CalibrationOptions,
    running: Mutex<Option<CancelToken>>,
}

impl std::fmt::Debug for MonitorWizard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MonitorWizard")
            .field("user_dir", &self.user_dir)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// Libera el orquestador y la calibración al terminar, aunque la tarea entre en pánico.
struct Running {
    agent: Arc<Agent>,
    wizard: Arc<MonitorWizard>,
}

impl Drop for Running {
    fn drop(&mut self) {
        *self.wizard.lock() = None;
        self.agent.orchestrator().end();
    }
}

impl MonitorWizard {
    pub fn new(
        open: CalibrationBus,
        user_dir: Option<PathBuf>,
        config: Option<PathBuf>,
        live: LiveDesk,
    ) -> Self {
        MonitorWizard {
            open,
            user_dir,
            config,
            live,
            options: CalibrationOptions::default(),
            running: Mutex::new(None),
        }
    }

    pub fn with_options(mut self, options: CalibrationOptions) -> Self {
        self.options = options;
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<CancelToken>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn profiles(&self) -> ProfileDb {
        let db = ProfileDb::load(self.user_dir.as_deref());
        for e in db.errors() {
            tracing::warn!("perfil DDC ignorado: {e}");
        }
        db
    }

    fn open_bus(&self) -> Result<Box<dyn DdcBus + Send>, String> {
        (self.open)().map_err(|e| format!("no se pudo abrir el bus DDC: {e}"))
    }

    /// Los monitores del bus con su perfil. Bloquea (abre el bus): fuera del runtime.
    pub fn list(&self, desk: &Config) -> Result<Vec<DisplayInfo>, String> {
        let bus = self.open_bus()?;
        let desk = self.live.desk(desk);
        let db = self.profiles();
        let monitors = bus.monitors();
        Ok(monitors
            .iter()
            .map(|info| {
                let monitor = bind(&desk, monitors, info.index);
                let configured = monitor
                    .as_ref()
                    .and_then(|id| desk.monitor(id))
                    .and_then(|m| m.ddc.clone());
                let profile = match (configured, db.find(info)) {
                    (Some(ddc), _) => DisplayProfile::Configured {
                        hdmi: ddc.hdmi,
                        display_port: ddc.display_port,
                    },
                    (None, Some((entry, _))) => DisplayProfile::Known {
                        name: entry.profile.name.clone(),
                        user: matches!(entry.source, ProfileSource::User(_)),
                        hdmi: entry.profile.codes.hdmi,
                        display_port: entry.profile.codes.display_port,
                        toml: entry.profile.to_toml(),
                    },
                    (None, None) => DisplayProfile::None,
                };
                DisplayInfo {
                    index: u32::try_from(info.index).unwrap_or(u32::MAX),
                    name: info.model().to_owned(),
                    pnp_id: info.edid.as_ref().map(|e| e.pnp_id()),
                    monitor,
                    profile,
                }
            })
            .collect())
    }

    /// Arranca la calibración de `display` en segundo plano; el avance va a
    /// [`Agent::subscribe_calibration`]. Tiene que llamarse dentro de un runtime.
    pub fn start(
        self: &Arc<Self>,
        agent: &Arc<Agent>,
        display: u32,
        monitor: Option<MonitorId>,
    ) -> Result<(), String> {
        if agent.machine() != Machine::Pc {
            return Err(NOT_HERE.into());
        }
        if let Some(id) = &monitor
            && agent.desk().monitor(id).is_none()
        {
            return Err(format!("el monitor `{id}` no está en la configuración"));
        }
        let cancel = CancelToken::new();
        {
            let mut running = self.lock();
            if running.is_some() {
                return Err(ALREADY.into());
            }
            if agent.orchestrator().try_begin().is_none() {
                return Err(BUSY.into());
            }
            *running = Some(cancel.clone());
        }
        let guard = Running {
            agent: agent.clone(),
            wizard: self.clone(),
        };
        // Si el hilo no arranca, el cierre se suelta con el guardia y todo queda libre.
        std::thread::Builder::new()
            .name("calibracion-ddc".into())
            .spawn(move || {
                let (agent, wizard) = (guard.agent.clone(), guard.wizard.clone());
                let outcome = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt.block_on(wizard.run(&agent, display, monitor, &cancel)),
                    Err(e) => failed(format!("no se pudo preparar la calibración: {e}")),
                };
                // Libre antes del aviso: quien lo reciba ya puede volver a calibrar.
                drop(guard);
                agent.emit_calibration(CalibrationUpdate::Finished(outcome));
            })
            .map(|_| ())
            .map_err(|e| format!("no se pudo arrancar la calibración: {e}"))
    }

    pub fn cancel(&self) -> Result<(), String> {
        match self.lock().as_ref() {
            Some(cancel) => {
                cancel.cancel();
                Ok(())
            }
            None => Err(NOT_RUNNING.into()),
        }
    }

    async fn run(
        &self,
        agent: &Arc<Agent>,
        display: u32,
        monitor: Option<MonitorId>,
        cancel: &CancelToken,
    ) -> CalibrationOutcome {
        let mut bus = match self.open_bus() {
            Ok(bus) => bus,
            Err(e) => return failed(e),
        };
        let index = usize::try_from(display).unwrap_or(usize::MAX);
        let Some(info) = bus.monitors().get(index).cloned() else {
            return failed(format!(
                "ya no hay un monitor {display} en el bus: vuelve a buscar"
            ));
        };
        let desk = agent.desk();
        let monitor = monitor.or_else(|| bind(&desk, bus.monitors(), index));
        let config = monitor.as_ref().and_then(|id| desk.monitor(id)).cloned();
        let (home, start) = inputs(config.as_ref(), &agent.state().owner);

        agent.emit_calibration(CalibrationUpdate::Started {
            display,
            name: info.model().to_owned(),
            monitor: monitor.clone(),
            start,
        });
        let db = self.profiles();
        let req = CalibrationRequest {
            monitor: index,
            start,
            known: db.brand_profiles(),
            options: self.options.clone(),
        };
        let confirmer = AgentConfirmer::new(agent.clone(), home);
        let result = calibrate::calibrate(bus.as_mut(), &req, &confirmer, cancel, &mut |e| {
            if let Some(step) = step(e) {
                agent.emit_calibration(CalibrationUpdate::Progress(step));
            }
        })
        .await;
        match result {
            Ok(p) => {
                let saved = self.keep(&db, &info, config.as_ref(), &p);
                // El perfil es parte del puesto compartido: la otra máquina también lo guarda.
                if config.is_some() {
                    agent.desk_changed();
                }
                CalibrationOutcome::Saved(saved)
            }
            Err(CalibrationError::Cancelled) => CalibrationOutcome::Cancelled,
            Err(e) => failed(e.to_string()),
        }
    }

    /// Guarda el perfil del usuario y, si el monitor está en la configuración, le pone
    /// los códigos en caliente y en `tandem.toml`.
    fn keep(
        &self,
        db: &ProfileDb,
        info: &MonitorInfo,
        monitor: Option<&MonitorConfig>,
        p: &CalibratedProfile,
    ) -> SavedProfile {
        let mut notes = Vec::new();
        let profile = p.to_profile(profile::today());
        let file = match (&self.user_dir, &profile) {
            (Some(dir), Some(profile)) => match db.save_user(dir, profile) {
                Ok(path) => Some(path.display().to_string()),
                Err(e) => {
                    notes.push(format!("no se pudo guardar el perfil: {e}"));
                    None
                }
            },
            (None, _) => {
                notes.push("no se guardó el perfil: no hay directorio de datos".into());
                None
            }
            (Some(_), None) => {
                notes.push("el monitor no dio EDID: sin id PnP no se guarda como perfil".into());
                None
            }
        };
        let (hdmi, display_port) = match p.ddc_profile() {
            Some(d) => (d.hdmi, d.display_port),
            None => (0, 0),
        };
        if let Some(m) = monitor {
            notes.extend(self.update_config(m, hdmi, display_port, info));
        } else {
            notes.push(
                "este monitor no está en tandem.toml: agrégalo con su `edid_model` para que los cambios de modo lo usen".into(),
            );
        }
        SavedProfile {
            name: profile
                .as_ref()
                .map(|p| p.name.clone())
                .unwrap_or_else(|| info.model().to_owned()),
            hdmi,
            display_port,
            attempts: u32::try_from(p.attempts).unwrap_or(u32::MAX),
            monitor: monitor.map(|m| m.id.clone()),
            file,
            toml: profile.map(|p| p.to_toml()),
            notes,
        }
    }

    /// Los códigos para `m`: en caliente siempre, y en `tandem.toml` si se puede.
    /// Devuelve lo que no salió.
    fn update_config(
        &self,
        m: &MonitorConfig,
        hdmi: u8,
        display_port: u8,
        info: &MonitorInfo,
    ) -> Option<String> {
        let ddc = DdcProfile { hdmi, display_port };
        // Sin `edid_model` el cambio de modo no lo encontraría en el bus; el id PnP es lo
        // más preciso que hay (los nombres se repiten entre modelos).
        let model = m.edid_model.is_none().then(|| {
            info.edid
                .as_ref()
                .map(|e| e.pnp_id())
                .unwrap_or_else(|| info.model().to_owned())
        });
        self.live.learn(m.id.clone(), ddc.clone(), model.clone());
        let path = self.config.as_ref()?;
        match config_edit::set_monitor_ddc(path, &m.id, &ddc, model.as_deref()) {
            Ok(()) => None,
            Err(e) => {
                tracing::warn!("no se pudo actualizar la configuración: {e}");
                Some(format!(
                    "no se pudo actualizar tandem.toml ({e}): los códigos valen hasta reiniciar el agente"
                ))
            }
        }
    }

    /// Usa el perfil que `display` ya tiene en la base, sin calibrar. Bloquea (abre el
    /// bus): fuera del runtime.
    pub fn use_profile(
        &self,
        agent: &Agent,
        display: u32,
        monitor: Option<MonitorId>,
    ) -> Result<(), String> {
        if self.lock().is_some() {
            return Err(ALREADY.into());
        }
        let bus = self.open_bus()?;
        let index = usize::try_from(display).unwrap_or(usize::MAX);
        let info =
            bus.monitors().get(index).cloned().ok_or_else(|| {
                format!("ya no hay un monitor {display} en el bus: vuelve a buscar")
            })?;
        let db = self.profiles();
        let (entry, _) = db
            .find(&info)
            .ok_or_else(|| format!("{} no tiene perfil en la base: calíbralo", info.model()))?;
        let desk = agent.desk();
        let id = monitor
            .or_else(|| bind(&desk, bus.monitors(), index))
            .ok_or_else(|| {
                format!(
                    "{} no corresponde a ningún monitor de tandem.toml: elige cuál es",
                    info.model()
                )
            })?;
        let m = desk
            .monitor(&id)
            .ok_or_else(|| format!("el monitor `{id}` no está en la configuración"))?;
        let c = &entry.profile.codes;
        let note = self.update_config(m, c.hdmi, c.display_port, &info);
        agent.desk_changed();
        match note {
            Some(note) => Err(note),
            None => Ok(()),
        }
    }
}

fn failed(reason: String) -> CalibrationOutcome {
    CalibrationOutcome::Failed { reason }
}

/// La entrada de la PC en el monitor (sus sondas las muestra esta UI) y la que tiene
/// ahora (de donde parte y adonde vuelve la calibración). Sin configuración, el monitor
/// se supone en el DisplayPort de la PC: es desde donde se calibra.
fn inputs(monitor: Option<&MonitorConfig>, owner: &BTreeMap<MonitorId, Machine>) -> (Input, Input) {
    let Some(m) = monitor else {
        return (Input::DisplayPort, Input::DisplayPort);
    };
    let home = m.input_for(Machine::Pc).unwrap_or(Input::DisplayPort);
    let start = owner
        .get(&m.id)
        .and_then(|who| m.input_for(*who))
        .unwrap_or(home);
    (home, start)
}

/// El evento del motor tal como lo dibuja la UI.
fn step(e: &CalibrationEvent) -> Option<CalibrationStep> {
    let n = |v: usize| u32::try_from(v).unwrap_or(u32::MAX);
    Some(match e {
        CalibrationEvent::State(EngineState::Reading) => CalibrationStep::Reading,
        CalibrationEvent::State(EngineState::UserSwitch { input }) => {
            CalibrationStep::UserSwitch { input: *input }
        }
        CalibrationEvent::State(EngineState::Testing { input }) => {
            CalibrationStep::Testing { input: *input }
        }
        CalibrationEvent::Trying {
            input,
            candidate,
            attempt,
            of,
        } => CalibrationStep::Trying {
            input: *input,
            code: candidate.code,
            attempt: n(*attempt),
            of: n(*of),
        },
        CalibrationEvent::Seen { input, code, sight } => CalibrationStep::Seen {
            input: *input,
            code: *code,
            seen: *sight == Sight::Input(*input),
        },
        CalibrationEvent::Returned { code } => CalibrationStep::Returned { code: *code },
        CalibrationEvent::Found { input, code } => CalibrationStep::Found {
            input: *input,
            code: *code,
        },
        CalibrationEvent::State(EngineState::Verifying) => CalibrationStep::Verifying,
        CalibrationEvent::Verified { input, code } => CalibrationStep::Verified {
            input: *input,
            code: *code,
        },
        // El final lo dice `Finished`; lo leído no se dibuja, y la falta de respuesta se
        // ve en la pregunta («¿sigo?»).
        CalibrationEvent::State(_)
        | CalibrationEvent::Declared(_)
        | CalibrationEvent::Reported { .. }
        | CalibrationEvent::NoAnswer { .. } => {
            return None;
        }
    })
}

/// El bus real de la PC (en Windows; en los demás sistemas no hay DDC todavía).
pub fn platform_bus() -> CalibrationBus {
    #[cfg(windows)]
    {
        Arc::new(|| {
            tandem_display::windows::ddc::WindowsDdc::open()
                .map(|b| Box::new(b) as Box<dyn DdcBus + Send>)
        })
    }
    #[cfg(not(windows))]
    {
        Arc::new(|| Err(DdcError::Unsupported))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tandem_core::{BuiltinDisplay, Mode};
    use tandem_display::calibrate::sim::{Showing, SimMonitor, samsung_32};
    use tandem_display::ddc::Edid;
    use tandem_display::ddc::fake::{FakeDdc, FakeMonitor};
    use tandem_display::desktop::{DesktopControl, DesktopError};
    use tandem_display::switcher::{DdcSwitcher, ManualSwitcher, PerMonitor};
    use tandem_proto::{ProbeAnswer, ProbeAsk, ProbeEvent, ProbePlace, Reply, UiCommand};
    use tokio::sync::broadcast;
    use tokio::sync::broadcast::error::RecvError;

    use super::*;
    use crate::agent::tests::until_outcome;
    use crate::config::AgentConfig;
    use crate::display::DisplaySteps;
    use crate::orchestrator::tests::{FakePeer, FakeSteps};
    use crate::store::StateStore;

    const PC: &str = r#"# La PC del puesto.
machine = "pc"

[[monitors]]
id = "m34"
label = '34"'
edid_model = "LC34G55T"
wiring = { pc = "displayport", mac = "hdmi" }
ddc = { hdmi = 0x06, display_port = 0x0F }

# El 32", todavía sin códigos.
[[monitors]]
id = "m32"
label = '32"'
edid_model = "Odyssey G5"
wiring = { pc = "displayport", mac = "hdmi" }

# Para despertar a la Mac (dormida al arrancar): el plan la enciende si muestra algo.
[power.wol]
mac = "9c:6b:00:1a:2b:ff"
"#;

    /// Sin esperas: el usuario simulado contesta enseguida.
    fn fast() -> CalibrationOptions {
        CalibrationOptions {
            confirm_timeout: Duration::from_secs(2),
            user_switch_timeout: Duration::from_secs(2),
            settle: Duration::ZERO,
            write_attempts: 1,
            retry_gap: Duration::ZERO,
        }
    }

    fn info(manufacturer: &str, product_code: u16, name: &str) -> FakeMonitor {
        FakeMonitor {
            info: Some(MonitorInfo {
                index: 0,
                description: "Monitor PnP genérico".into(),
                device: String::new(),
                edid: Some(Edid {
                    manufacturer: manufacturer.into(),
                    product_code,
                    serial_number: 0,
                    name: Some(name.into()),
                    serial_text: None,
                    size_cm: None,
                }),
            }),
            ..Default::default()
        }
    }

    fn three_monitors() -> CalibrationBus {
        Arc::new(|| {
            Ok(Box::new(FakeDdc::new(vec![
                info("SAM", 0x711A, "LC34G55T"),
                info("SAM", 0x7489, "Odyssey G5"),
                info("DEL", 0x4321, "U2720Q"),
            ])) as Box<dyn DdcBus + Send>)
        })
    }

    fn sim_bus(sim: &SimMonitor) -> CalibrationBus {
        let sim = sim.clone();
        Arc::new(move || Ok(Box::new(sim.clone()) as Box<dyn DdcBus + Send>))
    }

    /// El escritorio de la PC simulado: todo sale bien.
    struct Desk;

    impl DesktopControl for Desk {
        fn attach(&self, _: &MonitorConfig) -> Result<(), DesktopError> {
            Ok(())
        }
        fn detach(&self, _: &MonitorConfig) -> Result<(), DesktopError> {
            Ok(())
        }
        fn set_builtin(&self, _: BuiltinDisplay) -> Result<(), DesktopError> {
            Ok(())
        }
    }

    struct Pc {
        agent: Arc<Agent>,
        config: PathBuf,
        profiles: PathBuf,
        manual: Arc<ManualSwitcher>,
        _dir: tempfile::TempDir,
    }

    /// El agente de la PC con el monitor simulado: los cambios de entrada van por DDC al
    /// mismo monitor que se calibra; lo de la Mac lo hace un par simulado.
    fn pc(bus: CalibrationBus, sim: Option<&SimMonitor>) -> Pc {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("tandem.toml");
        std::fs::write(&config, PC).unwrap();
        let cfg = AgentConfig::load(&config).unwrap();
        let live = LiveDesk::default();
        let manual = Arc::new(ManualSwitcher::new());
        let mut local = DisplaySteps::new(Machine::Pc, cfg.desk.clone())
            .with_desktop(Arc::new(Desk))
            .with_others(Arc::new(FakeSteps::default()))
            .with_live(live.clone());
        if let Some(sim) = sim {
            let sim = sim.clone();
            let ddc = DdcSwitcher::with_retries(
                Arc::new(move || Ok(Box::new(sim.clone()) as Box<dyn DdcBus>)),
                1,
                Duration::ZERO,
            );
            local = local.with_switcher(Arc::new(PerMonitor {
                ddc: Arc::new(ddc),
                manual: manual.clone(),
            }));
        }
        let agent = Arc::new(Agent::with_backends(
            cfg,
            StateStore::new(dir.path().join("state.json")),
            Arc::new(local),
            manual.clone(),
        ));
        agent.set_peer(Arc::new(FakePeer(Arc::new(FakeSteps::default()))));
        let profiles = dir.path().join("profiles").join("ddc");
        agent.set_wizard(Arc::new(
            MonitorWizard::new(bus, Some(profiles.clone()), Some(config.clone()), live)
                .with_options(fast()),
        ));
        Pc {
            agent,
            config,
            profiles,
            manual,
            _dir: dir,
        }
    }

    /// El usuario frente a la UI de la PC (sin la otra máquina, la PC pregunta todo):
    /// cambia la entrada desde el menú cuando se le pide y contesta mirando el monitor.
    /// `back_only`: solo contesta lo de volver a `start`, como alguien que no sigue.
    fn user(agent: &Arc<Agent>, sim: &SimMonitor, back_only: Option<Input>) {
        let mut rx = agent.subscribe_probes();
        let (agent, sim) = (agent.clone(), sim.clone());
        tokio::spawn(async move {
            loop {
                let event = match rx.recv().await {
                    Ok(e) => e,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return,
                };
                let ProbeEvent::Show { id, probe, place } = event else {
                    continue;
                };
                assert!(
                    matches!(place, ProbePlace::Monitor { .. } | ProbePlace::Question),
                    "{place:?}"
                );
                if let Some(start) = back_only
                    && probe.input != start
                {
                    continue;
                }
                if probe.ask == ProbeAsk::UserSwitch {
                    sim.lock().showing = Showing::Input(probe.input);
                }
                // En la pregunta dice qué ve; en la sonda a pantalla completa hace clic si
                // la ve (en el 32" de la PC, la PC va por DisplayPort).
                let answer = match (place, sim.showing()) {
                    (ProbePlace::Question, Showing::Input(Input::DisplayPort)) => {
                        Some(ProbeAnswer::Machine(Machine::Pc))
                    }
                    (ProbePlace::Question, Showing::Input(Input::Hdmi)) => {
                        Some(ProbeAnswer::Machine(Machine::Mac))
                    }
                    (ProbePlace::Question, Showing::Black(_)) => Some(ProbeAnswer::Black),
                    (_, showing) if showing == Showing::Input(probe.input) => {
                        Some(ProbeAnswer::Seen)
                    }
                    _ => None,
                };
                if let Some(answer) = answer {
                    let _ = agent.probes().answer(id, answer);
                }
            }
        });
    }

    async fn finished(rx: &mut broadcast::Receiver<CalibrationUpdate>) -> Vec<CalibrationUpdate> {
        let mut seen = Vec::new();
        loop {
            let u = tokio::time::timeout(Duration::from_secs(20), rx.recv())
                .await
                .expect("la calibración no terminó")
                .unwrap();
            let end = matches!(u, CalibrationUpdate::Finished(_));
            seen.push(u);
            if end {
                return seen;
            }
        }
    }

    fn saved(updates: &[CalibrationUpdate]) -> SavedProfile {
        match updates.last() {
            Some(CalibrationUpdate::Finished(CalibrationOutcome::Saved(s))) => s.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn lists_each_monitor_with_its_profile_and_its_place_in_the_config() {
        let pc = pc(three_monitors(), None);
        let Reply::Displays(list) = pc.agent.respond(UiCommand::ListDisplays).await else {
            panic!("se esperaban los monitores");
        };
        assert_eq!(list.len(), 3);
        assert_eq!(
            list[0],
            DisplayInfo {
                index: 0,
                name: "LC34G55T".into(),
                pnp_id: Some("SAM711A".into()),
                monitor: Some("m34".into()),
                profile: DisplayProfile::Configured {
                    hdmi: 0x06,
                    display_port: 0x0F
                },
            }
        );
        // El 32" no tiene códigos en la configuración pero sí en la base del repo.
        assert_eq!(list[1].monitor, Some("m32".into()));
        assert!(
            matches!(
                &list[1].profile,
                DisplayProfile::Known { user: false, hdmi: 0x05, display_port: 0x0F, toml, .. }
                    if toml.contains("SAM7489")
            ),
            "{:?}",
            list[1].profile
        );
        assert_eq!(
            (list[2].monitor.clone(), list[2].profile.clone()),
            (None, DisplayProfile::None)
        );
    }

    #[tokio::test]
    async fn only_the_pc_calibrates_and_one_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let mac = Arc::new(crate::agent::tests::agent(dir.path()));
        for cmd in [
            UiCommand::ListDisplays,
            UiCommand::CancelCalibration,
            UiCommand::StartCalibration {
                display: 0,
                monitor: None,
            },
        ] {
            assert_eq!(mac.respond(cmd).await, Reply::Rejected(NOT_HERE.into()));
        }

        let pc = pc(three_monitors(), None);
        let a = &pc.agent;
        assert_eq!(
            a.handle(UiCommand::CancelCalibration),
            Reply::Rejected(NOT_RUNNING.into())
        );
        assert_eq!(
            a.handle(UiCommand::StartCalibration {
                display: 0,
                monitor: Some("m99".into())
            }),
            Reply::Rejected("el monitor `m99` no está en la configuración".into())
        );
        // Con un cambio de modo en curso no se calibra.
        let txn = a.orchestrator().try_begin().unwrap();
        assert_eq!(
            a.handle(UiCommand::StartCalibration {
                display: 0,
                monitor: None
            }),
            Reply::Rejected(BUSY.into())
        );
        let _ = txn;
        a.orchestrator().end();
    }

    // T-080: el perfil calibrado se usa en el siguiente cambio de modo, sin reiniciar el
    // agente; y queda guardado para el próximo arranque.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_calibrated_profile_is_used_by_the_next_mode_change() {
        // El 32" está en la Mac (HDMI), como en el estado inicial.
        let sim = SimMonitor::new(samsung_32(), Input::Hdmi);
        let pc = pc(sim_bus(&sim), Some(&sim));
        let a = &pc.agent;
        user(a, &sim, None);
        let mut manual = pc.manual.subscribe();
        let mut rx = a.subscribe_calibration();

        assert_eq!(
            a.respond(UiCommand::StartCalibration {
                display: 0,
                monitor: None
            })
            .await,
            Reply::Done
        );
        // Mientras calibra, ni otro arranque ni un cambio de modo.
        assert_eq!(
            a.handle(UiCommand::StartCalibration {
                display: 0,
                monitor: None
            }),
            Reply::Rejected(ALREADY.into())
        );
        assert_eq!(
            a.handle(UiCommand::SetMode(Mode::SoloPc)),
            Reply::Rejected(BUSY.into())
        );
        let updates = finished(&mut rx).await;
        assert_eq!(
            updates[0],
            CalibrationUpdate::Started {
                display: 0,
                name: "Odyssey G5".into(),
                monitor: Some("m32".into()),
                start: Input::Hdmi,
            }
        );
        assert!(
            updates.contains(&CalibrationUpdate::Progress(CalibrationStep::Found {
                input: Input::DisplayPort,
                code: 0x0F
            }))
        );
        let s = saved(&updates);
        assert_eq!(
            (s.hdmi, s.display_port, s.monitor.clone()),
            (0x05, 0x0F, Some("m32".into()))
        );
        assert!(s.notes.is_empty(), "{:?}", s.notes);
        // Terminó en la entrada de partida.
        assert_eq!(sim.showing(), Showing::Input(Input::Hdmi));

        // Perfil del usuario y tandem.toml, para el próximo arranque.
        let file = pc.profiles.join("SAM7489.toml");
        assert_eq!(s.file, Some(file.display().to_string()));
        assert!(s.toml.as_deref().is_some_and(|t| t.contains("hdmi = 0x05")));
        let db = ProfileDb::load(Some(&pc.profiles));
        assert!(matches!(
            db.get("SAM7489").unwrap().source,
            ProfileSource::User(_)
        ));
        let text = std::fs::read_to_string(&pc.config).unwrap();
        assert!(text.contains("# El 32\", todavía sin códigos."), "{text}");
        let cfg = AgentConfig::from_toml(&text).unwrap();
        assert_eq!(
            cfg.desk.monitor(&"m32".into()).unwrap().ddc,
            Some(DdcProfile {
                hdmi: 0x05,
                display_port: 0x0F
            })
        );

        // Sin reiniciar: pasar el 32" a la PC lo cambia por DDC con el código nuevo, sin
        // pedirlo a mano. Con lo que muestra cada monitor confirmado, para que solo se
        // mande el 32" (T-085).
        a.set_state(a.state().confirmed()).unwrap();
        let before = sim.writes().len();
        let mut txns = a.subscribe_txns();
        let r = a.handle(UiCommand::MoveMonitor {
            monitor: "m32".into(),
            to: Machine::Pc,
        });
        assert!(matches!(r, Reply::Accepted { .. }), "{r:?}");
        let events = until_outcome(&mut txns).await;
        assert!(
            matches!(
                events.last(),
                Some(tandem_proto::TxnEvent::Committed { .. })
            ),
            "{events:?}"
        );
        assert_eq!(sim.writes()[before..], [0x0F]);
        assert_eq!(sim.showing(), Showing::Input(Input::DisplayPort));
        assert!(manual.try_recv().is_err(), "no se pidió el cambio a mano");
        assert_eq!(a.state().owner.get(&"m32".into()), Some(&Machine::Pc));

        // La lista ya lo muestra configurado.
        let Reply::Displays(list) = a.respond(UiCommand::ListDisplays).await else {
            panic!()
        };
        assert_eq!(
            list[0].profile,
            DisplayProfile::Configured {
                hdmi: 0x05,
                display_port: 0x0F
            }
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_leaves_the_monitor_where_it_was_and_frees_the_desk() {
        let sim = SimMonitor::new(samsung_32(), Input::Hdmi);
        let pc = pc(sim_bus(&sim), Some(&sim));
        let a = &pc.agent;
        // El usuario no pasa el monitor a la otra entrada; solo confirma que sigue en HDMI.
        user(a, &sim, Some(Input::Hdmi));
        let mut rx = a.subscribe_calibration();
        assert_eq!(
            a.handle(UiCommand::StartCalibration {
                display: 0,
                monitor: Some("m32".into())
            }),
            Reply::Done
        );
        loop {
            let u = rx.recv().await.unwrap();
            if u == CalibrationUpdate::Progress(CalibrationStep::UserSwitch {
                input: Input::DisplayPort,
            }) {
                break;
            }
        }
        assert_eq!(a.handle(UiCommand::CancelCalibration), Reply::Done);
        let updates = finished(&mut rx).await;
        assert_eq!(
            updates.last(),
            Some(&CalibrationUpdate::Finished(CalibrationOutcome::Cancelled))
        );
        assert!(sim.writes().is_empty());
        assert_eq!(sim.showing(), Showing::Input(Input::Hdmi));
        // Nada guardado y el orquestador libre.
        assert!(!pc.profiles.join("SAM7489.toml").exists());
        assert_eq!(std::fs::read_to_string(&pc.config).unwrap(), PC);
        assert!(a.orchestrator().try_begin().is_some());
    }

    #[tokio::test]
    async fn a_known_profile_can_be_used_without_calibrating() {
        let pc = pc(three_monitors(), None);
        let a = &pc.agent;
        // El monitor 2 no está en la configuración ni en la base.
        assert!(matches!(
            a.respond(UiCommand::UseProfile {
                display: 2,
                monitor: None
            })
            .await,
            Reply::Rejected(why) if why.contains("no tiene perfil")
        ));
        assert_eq!(
            a.respond(UiCommand::UseProfile {
                display: 1,
                monitor: None
            })
            .await,
            Reply::Done
        );
        let cfg = AgentConfig::load(&pc.config).unwrap();
        assert_eq!(
            cfg.desk.monitor(&"m32".into()).unwrap().ddc,
            Some(DdcProfile {
                hdmi: 0x05,
                display_port: 0x0F
            })
        );
        let Reply::Displays(list) = a.respond(UiCommand::ListDisplays).await else {
            panic!()
        };
        assert!(matches!(list[1].profile, DisplayProfile::Configured { .. }));
    }

    #[test]
    fn a_monitor_is_bound_only_when_its_model_is_unambiguous() {
        let cfg = AgentConfig::from_toml(PC).unwrap();
        let infos: Vec<MonitorInfo> = [
            info("SAM", 0x711A, "LC34G55T"),
            info("SAM", 0x7489, "Odyssey G5"),
            info("SAM", 0x7489, "Odyssey G5"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, m)| MonitorInfo {
            index: i,
            ..m.info.unwrap()
        })
        .collect();
        assert_eq!(bind(&cfg.desk, &infos, 0), Some("m34".into()));
        // Dos 32" iguales: no se adivina cuál es cuál.
        assert_eq!(bind(&cfg.desk, &infos, 1), None);
        assert_eq!(bind(&cfg.desk, &infos[..2], 1), Some("m32".into()));
        assert_eq!(bind(&cfg.desk, &infos, 7), None);
    }

    #[test]
    fn learned_codes_go_over_the_config_and_fill_a_missing_model() {
        let cfg = AgentConfig::from_toml(PC).unwrap();
        let live = LiveDesk::default();
        let codes = DdcProfile {
            hdmi: 0x05,
            display_port: 0x0F,
        };
        live.learn("m32".into(), codes.clone(), Some("SAM7489".into()));
        live.learn("m34".into(), codes.clone(), Some("ignorado".into()));
        let desk = live.desk(&cfg.desk);
        let m32 = desk.monitor(&"m32".into()).unwrap();
        assert_eq!(m32.ddc, Some(codes.clone()));
        // Ya tenía modelo: se conserva.
        assert_eq!(m32.edid_model.as_deref(), Some("Odyssey G5"));
        let mut bare = cfg.desk.monitor(&"m32".into()).unwrap().clone();
        bare.edid_model = None;
        live.apply(&mut bare);
        assert_eq!(bare.edid_model.as_deref(), Some("SAM7489"));
        assert_eq!(desk.monitor(&"m34".into()).unwrap().ddc, Some(codes));
    }

    #[test]
    fn calibration_starts_where_the_monitor_is_now() {
        let cfg = AgentConfig::from_toml(PC).unwrap();
        let m = cfg.desk.monitor(&"m32".into()).unwrap();
        let mut owner = BTreeMap::new();
        assert_eq!(
            inputs(Some(m), &owner),
            (Input::DisplayPort, Input::DisplayPort)
        );
        owner.insert("m32".into(), Machine::Mac);
        assert_eq!(inputs(Some(m), &owner), (Input::DisplayPort, Input::Hdmi));
        assert_eq!(
            inputs(None, &owner),
            (Input::DisplayPort, Input::DisplayPort)
        );
    }

    #[test]
    fn engine_events_reach_the_ui_without_the_readings() {
        assert_eq!(
            step(&CalibrationEvent::State(EngineState::Reading)),
            Some(CalibrationStep::Reading)
        );
        assert_eq!(step(&CalibrationEvent::Declared(None)), None);
        assert_eq!(step(&CalibrationEvent::State(EngineState::Done)), None);
        assert_eq!(
            step(&CalibrationEvent::Returned { code: 0x0F }),
            Some(CalibrationStep::Returned { code: 0x0F })
        );
    }
}
