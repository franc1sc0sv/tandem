//! El estado vivo del agente y lo que contesta a la UI.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use tandem_core::plan::PlanError;
use tandem_core::{Abilities, Config, Input, Machine, Mode, MonitorId, Plan, State};
use tandem_display::switcher::{ManualEvent, ManualSwitcher};
use tandem_link::transport::{Connection, ControlStream};
use tandem_proto::{
    CalibrationUpdate, Capabilities, Channel, ControlMsg, DeskCapabilities, DeskSync, HubReport,
    InputStatus, PairingStatus, PeerStatus, PowerEvent, ProbeEvent, Reply, ScreenRect, SetupInfo,
    TxnEvent, UiCommand,
};
use tokio::sync::{broadcast, mpsc, watch};

use crate::add_monitor::{LiveDesk, MonitorWizard, NOT_HERE};
use crate::calibration::{DesktopLocator, ProbeBoard};
use crate::capabilities::{self, Seen};
use crate::config::AgentConfig;
use crate::display::DisplaySteps;
use crate::orchestrator::{LocalSteps, Orchestrator, PeerLink, Unimplemented};
use crate::store::{StateStore, StoreError};

/// Lo que la UI recibe cuando pide algo que este agente todavía no sabe hacer.
pub const NOT_IMPLEMENTED: &str = "todavía no implementado";
/// Lo que la UI recibe si pide un cambio con otro en curso.
pub const BUSY: &str = "ya hay un cambio en curso";
/// Lo que la UI recibe si pide emparejar sin la red en marcha.
pub const NO_NETWORK: &str = "la red del agente no está en marcha";

/// Quien sabe emparejar (la red): el agente le pasa los pedidos de la UI y publica
/// el avance con [`Agent::set_pairing_status`]. Cada método contesta enseguida; lo
/// que tarda sigue en segundo plano. `Err` es el motivo para la UI.
pub trait PairingControl: Send + Sync {
    fn start(&self) -> Result<(), String>;
    fn confirm(&self) -> Result<(), String>;
    fn cancel(&self) -> Result<(), String>;
    fn unpair(&self) -> Result<(), String>;
}

#[derive(Debug)]
pub struct Agent {
    cfg: AgentConfig,
    store: StateStore,
    state: watch::Sender<State>,
    /// Leer el estado y guardarlo tiene que ser una sola operación: si no, un
    /// guardado con el estado viejo puede terminar después de uno con el nuevo.
    persisting: Mutex<()>,
    orch: Orchestrator,
    /// Los cambios de entrada que el usuario hace a mano y confirma desde la UI.
    manual: Arc<ManualSwitcher>,
    /// Las sondas de calibración que muestra la UI de esta máquina (T-078).
    probes: ProbeBoard,
    /// Avisos de energía de la PC que no vienen de una transacción.
    power: broadcast::Sender<PowerEvent>,
    /// Con quién está emparejado y por qué enlace lo ve; lo actualiza la red.
    peer_status: watch::Sender<PeerStatus>,
    /// En qué va el emparejamiento; lo actualiza la red.
    pairing_status: watch::Sender<PairingStatus>,
    pairing: Mutex<Option<Arc<dyn PairingControl>>>,
    /// El teclado y mouse como lo ve la UI; lo actualiza [`crate::input`].
    input_status: watch::Sender<InputStatus>,
    /// Lo que el otro agente manda para el teclado y mouse.
    inbox: PeerInbox,
    /// Sube con cada `UpdateInputGeometry`: hay que volver a leer las pantallas.
    geometry: Arc<watch::Sender<u64>>,
    /// Los códigos DDC aprendidos al calibrar, que los pasos de pantalla leen en caliente.
    live: LiveDesk,
    /// «Agregar monitor» (solo en la PC).
    wizard: Mutex<Option<Arc<MonitorWizard>>>,
    /// El avance de la calibración, para la UI.
    calibration: broadcast::Sender<CalibrationUpdate>,
    /// El puesto con lo cambiado en caliente ([`LiveDesk`]), para avisar a la UI.
    desk: watch::Sender<Config>,
    /// `tandem.toml`, donde se guarda lo que se cambia desde la UI; sin él, solo en memoria.
    config_file: Mutex<Option<PathBuf>>,
    /// De dónde salió la configuración y en qué puerto escucha el agente.
    setup: Mutex<SetupInfo>,
    /// Lo que puede esta máquina y lo último que anunció la otra (T-084).
    caps: watch::Sender<DeskCapabilities>,
    /// Los monitores que esta máquina detectó al arrancar; sin detección, los del puesto.
    seen: Mutex<Option<Vec<Seen>>>,
    /// El reloj del puesto compartido ([`crate::desk_sync`]) y el atajo de bloqueo, que
    /// también es del puesto.
    sync: Mutex<SyncClock>,
}

#[derive(Debug, Clone)]
struct SyncClock {
    rev: u64,
    origin: Machine,
    lock_shortcut: Vec<String>,
}

impl std::fmt::Debug for dyn PairingControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairingControl")
    }
}

/// Lo último que mandó el otro agente para el teclado y mouse, y a quién pasarle los
/// streams de entrada y portapapeles que abre.
#[derive(Debug)]
pub struct PeerInbox {
    pub hub: watch::Sender<Option<HubReport>>,
    pub screens: watch::Sender<Option<Vec<ScreenRect>>>,
    channels: Mutex<Option<mpsc::UnboundedSender<IncomingChannel>>>,
}

/// Un stream de entrada o de portapapeles que abrió el otro agente, sobre su conexión
/// (los datagramas del mouse llegan por la misma).
pub struct IncomingChannel {
    pub channel: Channel,
    pub conn: Connection,
    pub stream: ControlStream,
}

impl std::fmt::Debug for IncomingChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncomingChannel")
            .field("channel", &self.channel)
            .field("from", &self.conn.remote_address())
            .finish_non_exhaustive()
    }
}

impl PeerInbox {
    fn new() -> Self {
        PeerInbox {
            hub: watch::Sender::new(None),
            screens: watch::Sender::new(None),
            channels: Mutex::new(None),
        }
    }

    /// Quien atiende los streams de entrada y portapapeles; reemplaza al anterior.
    pub fn set_channels(&self, tx: mpsc::UnboundedSender<IncomingChannel>) {
        *self.channels.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
    }

    /// Entrega el stream; sin nadie que lo atienda se descarta (se cierra).
    pub fn deliver(&self, incoming: IncomingChannel) {
        let tx = self.channels.lock().unwrap_or_else(PoisonError::into_inner);
        match tx.as_ref() {
            Some(tx) => {
                if let Err(e) = tx.send(incoming) {
                    tracing::debug!(canal = ?e.0.channel, "stream del otro agente sin quien lo atienda");
                }
            }
            None => tracing::debug!(
                canal = ?incoming.channel,
                "stream del otro agente sin motor de entrada: se cierra"
            ),
        }
    }
}

/// Avisos de energía que se guardan para un suscriptor lento.
const POWER_EVENT_BUFFER: usize = 16;
/// Avance de una calibración guardado para una UI lenta (cada código son unos pocos).
const CALIBRATION_BUFFER: usize = 128;

impl Agent {
    /// Arranca desde el estado guardado (o el inicial, si no hay), sin backends de
    /// plataforma: todo paso local responde «no implementado».
    pub fn new(cfg: AgentConfig, store: StateStore) -> Self {
        Self::with_local_steps(cfg, store, Arc::new(Unimplemented))
    }

    /// Como [`Agent::new`], con los pasos que esta máquina sabe hacer.
    pub fn with_local_steps(
        cfg: AgentConfig,
        store: StateStore,
        local: Arc<dyn LocalSteps>,
    ) -> Self {
        Self::with_backends(cfg, store, local, Arc::new(ManualSwitcher::new()))
    }

    /// Con los backends reales de esta máquina ([`DisplaySteps::platform`]).
    pub fn with_platform(cfg: AgentConfig, store: StateStore) -> Self {
        let manual = Arc::new(ManualSwitcher::new());
        let geometry = Arc::new(watch::Sender::new(0));
        let live = LiveDesk::default();
        let local = Arc::new(
            DisplaySteps::platform(&cfg, manual.clone(), geometry.clone()).with_live(live.clone()),
        );
        let locator = tandem_display::desktop::native(cfg.machine)
            .map(|d| Arc::new(DesktopLocator(d)) as Arc<dyn crate::calibration::ScreenLocator>);
        let mut agent = Self::with_backends(cfg, store, local, manual);
        agent.geometry = geometry;
        agent.probes = ProbeBoard::new(locator);
        agent.live = live;
        agent
    }

    /// `manual` es el backend que `local` usa para los cambios a mano: sus pedidos
    /// llegan a la UI y las respuestas de la UI vuelven a él.
    pub fn with_backends(
        cfg: AgentConfig,
        store: StateStore,
        local: Arc<dyn LocalSteps>,
        manual: Arc<ManualSwitcher>,
    ) -> Self {
        let mut state = store.load_or_initial(&cfg.desk, cfg.machine);
        // Esta máquina siempre está prendida para sí misma (DESIGN §3.7); la otra, hasta
        // que los enlaces digan otra cosa, como quedó.
        state.on.set(cfg.machine, true);
        // Lo guardado es de un arranque anterior: desde entonces los monitores pudieron
        // cambiar a mano y ninguno lo dice (T-085). Hasta «Aplicar modo» o que el usuario
        // diga lo que muestra cada uno, queda sin confirmar.
        state.forget();
        let orch = Orchestrator::new(cfg.machine, local, cfg.orchestrator.timeouts());
        let input = InputStatus::new(cfg.input.keyboard);
        let here = capabilities::local(&cfg, &cfg.desk, None, &input);
        Agent {
            store,
            state: watch::Sender::new(state),
            persisting: Mutex::new(()),
            orch,
            manual,
            // Sin escritorio que consultar, la UI busca el monitor por su nombre.
            probes: ProbeBoard::new(None),
            power: broadcast::channel(POWER_EVENT_BUFFER).0,
            // Hasta que la red diga otra cosa, el agente trabaja solo.
            peer_status: watch::Sender::new(PeerStatus::Unpaired),
            pairing_status: watch::Sender::new(PairingStatus::Idle),
            pairing: Mutex::new(None),
            input_status: watch::Sender::new(input),
            inbox: PeerInbox::new(),
            geometry: Arc::new(watch::Sender::new(0)),
            live: LiveDesk::default(),
            wizard: Mutex::new(None),
            calibration: broadcast::channel(CALIBRATION_BUFFER).0,
            desk: watch::Sender::new(cfg.desk.clone()),
            config_file: Mutex::new(None),
            setup: Mutex::new(SetupInfo {
                config_file: None,
                generated: false,
                port: None,
            }),
            caps: watch::Sender::new(DeskCapabilities { here, peer: None }),
            seen: Mutex::new(None),
            sync: Mutex::new(SyncClock {
                rev: cfg.desk_rev,
                origin: cfg.desk_origin.unwrap_or(cfg.machine),
                lock_shortcut: cfg.input.lock_shortcut.clone(),
            }),
            cfg,
        }
    }

    /// El archivo de la configuración, donde se guarda lo que se cambia desde la UI.
    pub fn set_config_file(&self, path: PathBuf) {
        self.setup
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .config_file = Some(path.display().to_string());
        *self
            .config_file
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(path);
    }

    /// El agente generó la configuración al arrancar (T-082): la UI lo dice.
    pub fn set_generated(&self, generated: bool) {
        self.setup
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .generated = generated;
    }

    /// El puerto real de la red (puede no ser el de la config, si estaba ocupado).
    pub fn set_port(&self, port: u16) {
        self.setup
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .port = Some(port);
    }

    pub fn setup(&self) -> SetupInfo {
        self.setup
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// El puesto que vale ahora: el de la configuración con lo cambiado en caliente.
    pub fn desk(&self) -> Config {
        self.live.desk(&self.cfg.desk)
    }

    /// Avisa cada vez que cambia el puesto.
    pub fn subscribe_desk(&self) -> watch::Receiver<Config> {
        self.desk.subscribe()
    }

    /// Cambia qué entrada del monitor va a cada máquina: se guarda en `tandem.toml` y vale
    /// desde el siguiente cambio de modo. No mueve nada: el monitor sigue en la entrada en
    /// la que está. Con un cambio de modo en curso se rechaza.
    fn set_wiring(&self, monitor: MonitorId, wiring: BTreeMap<Machine, Input>) -> Reply {
        if self.desk().monitor(&monitor).is_none() {
            return Reply::Rejected(format!(
                "el monitor `{monitor}` no está en la configuración"
            ));
        }
        if wiring.is_empty() {
            return Reply::Rejected(format!(
                "{monitor} tiene que ir cableado al menos a una máquina"
            ));
        }
        let inputs: Vec<Input> = wiring.values().copied().collect();
        if inputs.len() == 2 && inputs[0] == inputs[1] {
            return Reply::Rejected(format!(
                "{monitor}: las dos máquinas no pueden ir a la misma entrada"
            ));
        }
        if self.orch.try_begin().is_none() {
            return Reply::Rejected(BUSY.into());
        }
        let file = self
            .config_file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let saved = match file {
            Some(path) => crate::config_edit::set_monitor_wiring(&path, &monitor, &wiring)
                .map_err(|e| format!("no se guardó el cableado: {e}")),
            None => Ok(()),
        };
        if saved.is_ok() {
            tracing::info!(%monitor, ?wiring, "cableado cambiado desde la UI");
            self.live.rewire(monitor, wiring);
            // Es del puesto: la otra máquina lo guarda también (T-084).
            self.desk_changed();
        }
        self.orch.end();
        done(saved)
    }

    /// El puesto compartido como se le manda a la otra máquina.
    pub fn desk_sync(&self) -> DeskSync {
        let clock = self.clock();
        DeskSync {
            rev: clock.rev,
            origin: clock.origin,
            desk: self.desk(),
            lock_shortcut: clock.lock_shortcut,
        }
    }

    fn clock(&self) -> SyncClock {
        self.sync
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn config_path(&self) -> Option<PathBuf> {
        self.config_file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// El puesto cambió aquí (cableado, perfil DDC): versión nueva, a `tandem.toml`, a la
    /// UI y a la otra máquina.
    pub fn desk_changed(&self) {
        let next = {
            let mut clock = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
            let next = crate::desk_sync::bump(
                clock.rev,
                self.cfg.machine,
                self.desk(),
                clock.lock_shortcut.clone(),
            );
            clock.rev = next.rev;
            clock.origin = next.origin;
            next
        };
        self.save_desk(&next);
        self.desk.send_replace(next.desk.clone());
        self.refresh_capabilities();
        self.orch.peer().send(ControlMsg::Desk(next));
    }

    /// El puesto que mandó la otra máquina: si es más nuevo (o, recién generados los
    /// dos, se unen), queda aquí, en caliente y en `tandem.toml`.
    pub fn receive_desk(&self, theirs: DeskSync) {
        let mine = self.desk_sync();
        let Some(next) = crate::desk_sync::receive(&mine, &theirs) else {
            return;
        };
        tracing::info!(
            rev = next.rev,
            origen = %next.origin,
            monitores = next.desk.monitors.len(),
            "puesto actualizado desde la otra máquina"
        );
        {
            let mut clock = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
            clock.rev = next.rev;
            clock.origin = next.origin;
            clock.lock_shortcut = next.lock_shortcut.clone();
        }
        self.live.replace(next.desk.clone());
        self.save_desk(&next);
        // Los monitores nuevos se suponen donde los dejaría el estado inicial.
        let state = crate::store::reconcile(self.state(), &next.desk);
        if let Err(e) = self.set_state(state) {
            tracing::warn!("{e}");
        }
        self.desk.send_if_modified(|d| {
            let changed = *d != next.desk;
            *d = next.desk;
            changed
        });
        self.refresh_capabilities();
    }

    fn save_desk(&self, sync: &DeskSync) {
        if let Some(path) = self.config_path()
            && let Err(e) = crate::config_edit::set_desk(&path, sync)
        {
            tracing::warn!("no se guardó el puesto en {}: {e}", path.display());
        }
    }

    /// Lo que puede cada máquina, para la UI.
    pub fn capabilities(&self) -> DeskCapabilities {
        self.caps.borrow().clone()
    }

    /// Avisa cada vez que cambia lo que puede alguna de las dos.
    pub fn subscribe_capabilities(&self) -> watch::Receiver<DeskCapabilities> {
        self.caps.subscribe()
    }

    /// Los monitores que esta máquina detectó (al arrancar, `daemon`).
    pub fn set_seen_monitors(&self, seen: Vec<Seen>) {
        *self.seen.lock().unwrap_or_else(PoisonError::into_inner) = Some(seen);
        self.refresh_capabilities();
    }

    /// Vuelve a armar lo que puede esta máquina; si cambió, se lo dice a la UI y a la otra.
    pub fn refresh_capabilities(&self) {
        let seen = self
            .seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let here = capabilities::local(
            &self.cfg,
            &self.desk(),
            seen.as_deref(),
            &self.input_status(),
        );
        let changed = self.caps.send_if_modified(|c| {
            let changed = c.here != here;
            c.here = here.clone();
            changed
        });
        if changed {
            self.orch.peer().send(ControlMsg::Capabilities(here));
        }
    }

    /// Lo que anunció la otra máquina.
    pub fn set_peer_capabilities(&self, peer: Capabilities) {
        if peer.machine != self.cfg.machine.other() {
            tracing::warn!(
                "anuncio de capacidades de {} por el enlace de la otra máquina: se ignora",
                peer.machine
            );
            return;
        }
        self.caps.send_if_modified(|c| {
            let changed = c.peer.as_ref() != Some(&peer);
            c.peer = Some(peer);
            changed
        });
    }

    /// Lo que el plan puede pedirle a cada máquina ([`capabilities::abilities`]).
    pub fn abilities(&self) -> Abilities {
        let caps = self.caps.borrow();
        capabilities::abilities(&self.desk(), &caps.here, caps.peer.as_ref())
    }

    /// Los códigos DDC aprendidos en caliente que leen los pasos de pantalla de
    /// [`Agent::with_platform`] (con otros pasos, los que se le hayan dado a ellos).
    pub fn live_desk(&self) -> &LiveDesk {
        &self.live
    }

    /// Quién atiende «Agregar monitor» (en la PC, al arrancar).
    pub fn set_wizard(&self, wizard: Arc<MonitorWizard>) {
        *self.wizard.lock().unwrap_or_else(PoisonError::into_inner) = Some(wizard);
    }

    fn wizard(&self) -> Result<Arc<MonitorWizard>, String> {
        self.wizard
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| NOT_HERE.to_owned())
    }

    /// El avance de las calibraciones, para la UI.
    pub fn subscribe_calibration(&self) -> broadcast::Receiver<CalibrationUpdate> {
        self.calibration.subscribe()
    }

    pub(crate) fn emit_calibration(&self, update: CalibrationUpdate) {
        // Sin UI conectada no hay a quién avisar.
        let _ = self.calibration.send(update);
    }

    /// Como [`Agent::handle`], pero lo que bloquea (abrir el bus DDC) va fuera del
    /// runtime. Es lo que usa la IPC.
    pub async fn respond(self: &Arc<Self>, cmd: UiCommand) -> Reply {
        if matches!(cmd, UiCommand::ListDisplays | UiCommand::UseProfile { .. }) {
            let me = self.clone();
            return tokio::task::spawn_blocking(move || me.handle(cmd))
                .await
                .unwrap_or_else(|e| Reply::Rejected(format!("el bus DDC terminó mal: {e}")));
        }
        self.handle(cmd)
    }

    /// El teclado y mouse, para la UI.
    pub fn input_status(&self) -> InputStatus {
        self.input_status.borrow().clone()
    }

    /// Avisa cada vez que cambia el teclado y mouse.
    pub fn subscribe_input(&self) -> watch::Receiver<InputStatus> {
        self.input_status.subscribe()
    }

    pub fn set_input_status(&self, status: InputStatus) {
        let changed = self.input_status.send_if_modified(|s| {
            let changed = *s != status;
            *s = status;
            changed
        });
        // El hub y los permisos son parte de lo que anuncia esta máquina.
        if changed {
            self.refresh_capabilities();
        }
    }

    /// Lo que el otro agente manda para el teclado y mouse.
    pub fn inbox(&self) -> &PeerInbox {
        &self.inbox
    }

    /// Cambia con cada paso `UpdateInputGeometry` hecho en esta máquina.
    pub fn subscribe_geometry(&self) -> watch::Receiver<u64> {
        self.geometry.subscribe()
    }

    pub fn orchestrator(&self) -> &Orchestrator {
        &self.orch
    }

    /// Conexión con el otro agente para los pasos que le tocan. Lo primero que va por
    /// ella es lo que puede esta máquina y su copia del puesto (T-084).
    pub fn set_peer(&self, peer: Arc<dyn PeerLink>) {
        self.orch.set_peer(peer.clone());
        peer.send(ControlMsg::Capabilities(self.capabilities().here));
        peer.send(ControlMsg::Desk(self.desk_sync()));
    }

    /// Avance de las transacciones, para la UI.
    pub fn subscribe_txns(&self) -> broadcast::Receiver<TxnEvent> {
        self.orch.subscribe()
    }

    /// Pedidos de cambio manual de entrada, para la UI.
    pub fn subscribe_manual(&self) -> broadcast::Receiver<ManualEvent> {
        self.manual.subscribe()
    }

    /// Las sondas de calibración de esta máquina: las muestra su UI.
    pub fn probes(&self) -> &ProbeBoard {
        &self.probes
    }

    /// Sondas de calibración que mostrar o quitar, para la UI.
    pub fn subscribe_probes(&self) -> broadcast::Receiver<ProbeEvent> {
        self.probes.subscribe()
    }

    /// Avisos de energía de la otra máquina (por ejemplo, que se la dio por apagada), para la UI.
    pub fn subscribe_power(&self) -> broadcast::Receiver<PowerEvent> {
        self.power.subscribe()
    }

    /// El par y el enlace con él, para la UI.
    pub fn peer_status(&self) -> PeerStatus {
        self.peer_status.borrow().clone()
    }

    /// Avisa cada vez que cambia el par o su enlace.
    pub fn subscribe_peer(&self) -> watch::Receiver<PeerStatus> {
        self.peer_status.subscribe()
    }

    pub fn set_peer_status(&self, status: PeerStatus) {
        self.peer_status.send_if_modified(|s| {
            let changed = *s != status;
            *s = status;
            changed
        });
    }

    /// Quién atiende los pedidos de emparejamiento (la red, al arrancar).
    pub fn set_pairing(&self, control: Arc<dyn PairingControl>) {
        *self.pairing.lock().unwrap_or_else(|e| e.into_inner()) = Some(control);
    }

    pub fn pairing_status(&self) -> PairingStatus {
        self.pairing_status.borrow().clone()
    }

    /// Avisa con cada paso del emparejamiento.
    pub fn subscribe_pairing(&self) -> watch::Receiver<PairingStatus> {
        self.pairing_status.subscribe()
    }

    pub fn set_pairing_status(&self, status: PairingStatus) {
        self.pairing_status.send_if_modified(|s| {
            let changed = *s != status;
            *s = status;
            changed
        });
    }

    fn pair(&self, f: impl FnOnce(&dyn PairingControl) -> Result<(), String>) -> Reply {
        let control = self
            .pairing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match control {
            Some(c) => match f(c.as_ref()) {
                Ok(()) => Reply::Done,
                Err(why) => Reply::Rejected(why),
            },
            None => Reply::Rejected(NO_NETWORK.into()),
        }
    }

    pub(crate) fn emit_power(&self, event: PowerEvent) {
        // Sin UI conectada no hay a quién avisar, y eso no es un error.
        let _ = self.power.send(event);
    }

    pub fn machine(&self) -> Machine {
        self.cfg.machine
    }

    pub fn config(&self) -> &AgentConfig {
        &self.cfg
    }

    pub fn store_path(&self) -> &std::path::Path {
        self.store.path()
    }

    pub fn state(&self) -> State {
        self.state.borrow().clone()
    }

    /// Avisa cada vez que el estado cambia.
    pub fn subscribe(&self) -> watch::Receiver<State> {
        self.state.subscribe()
    }

    /// Publica el estado nuevo y lo guarda.
    ///
    /// Se publica aunque falle el guardado: el cambio ya ocurrió en el puesto y la UI
    /// tiene que verlo; el error se devuelve para registrarlo.
    pub fn set_state(&self, mut new: State) -> Result<(), StoreError> {
        // Venga de donde venga (la otra máquina, una transacción), esta está prendida.
        new.on.set(self.cfg.machine, true);
        let changed = self.state.send_if_modified(|s| {
            if *s == new {
                false
            } else {
                *s = new;
                true
            }
        });
        if changed { self.persist() } else { Ok(()) }
    }

    pub fn persist(&self) -> Result<(), StoreError> {
        // Si otro hilo entró en pánico guardando, el candado no protege datos: se sigue.
        let _guard = self.persisting.lock().unwrap_or_else(|e| e.into_inner());
        let state = self.state.borrow().clone();
        self.store.save(&state)
    }

    /// Contesta un pedido de la UI.
    ///
    /// Los cambios de modo y de monitor arrancan una transacción en segundo plano y
    /// se contestan con [`Reply::Accepted`]; el avance llega como [`TxnEvent`]. Tiene
    /// que llamarse dentro de un runtime de tokio.
    pub fn handle(self: &Arc<Self>, cmd: UiCommand) -> Reply {
        match cmd {
            UiCommand::GetState => Reply::State(self.state()),
            UiCommand::GetDesk => Reply::Desk(self.desk()),
            UiCommand::GetSetup => Reply::Setup(self.setup()),
            UiCommand::GetCapabilities => Reply::Capabilities(Box::new(self.capabilities())),
            UiCommand::SetWiring { monitor, wiring } => self.set_wiring(monitor, wiring),
            UiCommand::GetPeer => Reply::Peer(self.peer_status()),
            UiCommand::GetPairing => Reply::Pairing(self.pairing_status()),
            UiCommand::StartPairing => self.pair(|p| p.start()),
            UiCommand::ConfirmPairing => self.pair(|p| p.confirm()),
            UiCommand::CancelPairing => self.pair(|p| p.cancel()),
            UiCommand::Unpair => self.pair(|p| p.unpair()),
            UiCommand::GetInput => Reply::Input(self.input_status()),
            UiCommand::SetMode(mode) => self.start(mode),
            UiCommand::ApplyMode(mode) => self.apply(mode),
            UiCommand::SetShown(shows) => self.set_shown(shows),
            UiCommand::SetPower { machine, on } => self.start_power(machine, on),
            UiCommand::MoveMonitor { monitor, to } => {
                if self.desk().monitor(&monitor).is_none() {
                    return Reply::Rejected(format!(
                        "el monitor `{monitor}` no está en la configuración"
                    ));
                }
                let mut assign = self.state().owner;
                assign.insert(monitor, to);
                self.start(Mode::Shared(assign))
            }
            // La preferencia de la pantalla integrada no es un modo: llega con su tarea.
            UiCommand::SetBuiltin(_) => Reply::Rejected(NOT_IMPLEMENTED.into()),
            UiCommand::AnswerManualSwitch { id, done } => match self.manual.answer(id, done) {
                Ok(()) => Reply::Done,
                Err(e) => Reply::Rejected(e.to_string()),
            },
            UiCommand::AnswerProbe { id, answer } => match self.probes.answer(id, answer) {
                Ok(()) => Reply::Done,
                Err(e) => Reply::Rejected(e.to_string()),
            },
            // Abren el bus DDC, que bloquea: la IPC los pide con `respond`.
            UiCommand::ListDisplays => match self.wizard().and_then(|w| w.list(&self.desk())) {
                Ok(displays) => Reply::Displays(displays),
                Err(e) => Reply::Rejected(e),
            },
            UiCommand::UseProfile { display, monitor } => done(
                self.wizard()
                    .and_then(|w| w.use_profile(self, display, monitor)),
            ),
            UiCommand::StartCalibration { display, monitor } => {
                done(self.wizard().and_then(|w| w.start(self, display, monitor)))
            }
            UiCommand::CancelCalibration => done(self.wizard().and_then(|w| w.cancel())),
        }
    }

    fn start(self: &Arc<Self>, mode: Mode) -> Reply {
        let abilities = self.abilities();
        self.start_plan(&format!("{mode:?}"), move |cfg, from| {
            Plan::to_mode(cfg, from, &mode, &abilities)
        })
    }

    /// «Aplicar modo» (T-085): la entrada de todos los monitores, crea Tandem lo que crea.
    fn apply(self: &Arc<Self>, mode: Mode) -> Reply {
        let abilities = self.abilities();
        self.start_plan(&format!("aplicar {mode:?}"), move |cfg, from| {
            Plan::apply_mode(cfg, from, &mode, &abilities)
        })
    }

    /// Lo que el usuario dice que muestra cada monitor (T-085): se corrige el estado sin
    /// mandar ninguna entrada, cada escritorio sigue a lo corregido ([`Plan::follow`]) y el
    /// estado final llega a la otra máquina como el de cualquier transacción.
    fn set_shown(self: &Arc<Self>, shows: BTreeMap<MonitorId, Option<Machine>>) -> Reply {
        if shows.is_empty() {
            return Reply::Rejected("no se dijo lo que muestra ningún monitor".into());
        }
        let desk = self.desk();
        for (id, shown) in &shows {
            let Some(m) = desk.monitor(id) else {
                return Reply::Rejected(format!("el monitor `{id}` no está en la configuración"));
            };
            if let Some(machine) = shown
                && m.input_for(*machine).is_none()
            {
                return Reply::Rejected(format!(
                    "{} no está cableado a la {}: no puede mostrarla",
                    m.label,
                    capabilities::label(*machine)
                ));
            }
        }
        tracing::info!(?shows, "el usuario dice lo que muestra cada monitor");
        self.start_txn("lo que muestra cada monitor", move |cfg, from| {
            let mut to = from.clone();
            for (id, shown) in &shows {
                to.record_shown(id, *shown);
            }
            let plan = Plan::follow(cfg, from, &to);
            Ok((to, plan))
        })
    }

    /// Encender es solo despertar `machine`; apagar le saca antes todos los monitores
    /// ([`Plan::power_off`]). Pedir lo que ya está es una transacción vacía.
    fn start_power(self: &Arc<Self>, machine: Machine, on: bool) -> Reply {
        let name = capabilities::label(machine);
        let what = if on {
            format!("encender la {name}")
        } else {
            format!("apagar la {name}")
        };
        let abilities = self.abilities();
        self.start_plan(&what, move |cfg, from| {
            if on {
                Plan::power_on(from, machine, &abilities)
            } else {
                Plan::power_off(cfg, from, machine, &abilities)
            }
        })
    }

    fn start_plan(
        self: &Arc<Self>,
        what: &str,
        plan_for: impl FnOnce(&tandem_core::Config, &State) -> Result<Plan, PlanError>,
    ) -> Reply {
        self.start_txn(what, |cfg, from| Ok((from.clone(), plan_for(cfg, from)?)))
    }

    /// Como [`Agent::start_plan`], pero `prepare` también da el estado del que parte la
    /// transacción (el corregido por el usuario, en [`Agent::set_shown`]).
    fn start_txn(
        self: &Arc<Self>,
        what: &str,
        prepare: impl FnOnce(&tandem_core::Config, &State) -> Result<(State, Plan), PlanError>,
    ) -> Reply {
        let Some(txn) = self.orch.try_begin() else {
            return Reply::Rejected(BUSY.into());
        };
        let busy = Busy(self.clone());
        let desk = self.desk();
        let (from, plan) = match prepare(&desk, &self.state()) {
            Ok(p) => p,
            Err(e) => return Reply::Rejected(e.to_string()),
        };
        tracing::info!(txn, what, steps = plan.steps.len(), "cambio de modo");
        tokio::spawn(async move {
            let agent = busy.0.clone();
            let mut state = from;
            let outcome = agent.orch.run(txn, &desk, &mut state, &plan).await;
            match &outcome {
                Ok(_) => tracing::info!(txn, "cambio de modo aplicado"),
                Err(e) => tracing::warn!(txn, "cambio de modo fallido: {e}"),
            }
            // Primero el estado, después el aviso: quien lo reciba ya ve el estado nuevo.
            if let Err(e) = agent.set_state(state.clone()) {
                tracing::warn!("{e}");
            }
            // Libre antes del aviso: quien lo reciba ya puede pedir el siguiente cambio.
            drop(busy);
            agent.orch.finish(txn, &state, &outcome);
        });
        Reply::Accepted { txn }
    }
}

fn done(r: Result<(), String>) -> Reply {
    match r {
        Ok(()) => Reply::Done,
        Err(why) => Reply::Rejected(why),
    }
}

/// Libera el orquestador al terminar la transacción, aunque la tarea entre en pánico.
struct Busy(Arc<Agent>);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.orch.end();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use std::time::Duration;

    use tandem_core::{BuiltinDisplay, Step};

    use super::*;
    use crate::config::fixtures;
    use crate::orchestrator::STEP_NOT_IMPLEMENTED;
    use crate::orchestrator::tests::{FakePeer, FakeSteps};

    pub fn agent(dir: &Path) -> Agent {
        Agent::new(
            fixtures::desk_with_wol(),
            StateStore::new(dir.join("state.json")),
        )
    }

    #[test]
    fn starts_from_initial_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        assert_eq!(a.machine(), Machine::Mac);
        assert_eq!(a.state(), State::initial(&a.config().desk, Machine::Mac));
    }

    fn swapped() -> BTreeMap<Machine, Input> {
        [
            (Machine::Pc, Input::Hdmi),
            (Machine::Mac, Input::DisplayPort),
        ]
        .into()
    }

    /// T-082: el cableado elegido en la UI queda en `tandem.toml`, se avisa y el siguiente
    /// cambio de modo ya pide la entrada nueva.
    #[tokio::test]
    async fn wiring_from_the_ui_is_saved_and_used_by_the_next_plan() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        std::fs::write(&file, fixtures::DESK).unwrap();
        let a = Arc::new(agent(dir.path()));
        a.set_config_file(file.clone());
        a.set_generated(true);
        let mut desk = a.subscribe_desk();
        let reply = a.handle(UiCommand::SetWiring {
            monitor: "m32".into(),
            wiring: swapped(),
        });
        assert_eq!(reply, Reply::Done);
        assert!(desk.has_changed().unwrap());
        assert_eq!(
            desk.borrow_and_update()
                .monitor(&"m32".into())
                .unwrap()
                .wiring,
            swapped()
        );
        let Reply::Desk(d) = a.handle(UiCommand::GetDesk) else {
            panic!("sin puesto")
        };
        assert_eq!(d.monitor(&"m32".into()).unwrap().wiring, swapped());
        let saved = AgentConfig::load(&file).unwrap();
        assert_eq!(saved.desk.monitor(&"m32".into()).unwrap().wiring, swapped());
        assert_eq!(
            a.handle(UiCommand::GetSetup),
            Reply::Setup(SetupInfo {
                config_file: Some(file.display().to_string()),
                generated: true,
                port: None,
            })
        );

        let mut txns = a.subscribe_txns();
        assert!(matches!(
            a.handle(UiCommand::SetMode(Mode::SoloPc)),
            Reply::Accepted { .. }
        ));
        let events = until_outcome(&mut txns).await;
        let TxnEvent::Started { steps, .. } = &events[0] else {
            panic!("{events:?}")
        };
        let input_of = |id: &str| {
            steps.iter().find_map(|s| match s {
                Step::SwitchInput { monitor, input, .. } if monitor.0 == id => Some(*input),
                _ => None,
            })
        };
        assert_eq!(input_of("m32"), Some(Input::Hdmi));
        assert_eq!(input_of("m34"), Some(Input::DisplayPort));
    }

    #[test]
    fn bad_wiring_is_rejected_and_nothing_changes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        std::fs::write(&file, fixtures::DESK).unwrap();
        let a = Arc::new(agent(dir.path()));
        a.set_config_file(file.clone());
        let same = [(Machine::Pc, Input::Hdmi), (Machine::Mac, Input::Hdmi)].into();
        for (monitor, wiring) in [("m99", swapped()), ("m32", BTreeMap::new()), ("m32", same)] {
            let reply = a.handle(UiCommand::SetWiring {
                monitor: monitor.into(),
                wiring,
            });
            assert!(matches!(reply, Reply::Rejected(_)), "{reply:?}");
        }
        // Sin poder escribir el archivo, tampoco cambia en memoria.
        std::fs::write(&file, "machine = ").unwrap();
        let reply = a.handle(UiCommand::SetWiring {
            monitor: "m32".into(),
            wiring: swapped(),
        });
        assert!(
            matches!(&reply, Reply::Rejected(r) if r.starts_with("no se guardó el cableado")),
            "{reply:?}"
        );
        assert_eq!(a.desk(), a.config().desk);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "machine = ");
    }

    #[tokio::test]
    async fn wiring_waits_for_a_mode_change_in_progress() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        let _txn = a.orchestrator().try_begin().unwrap();
        let reply = a.handle(UiCommand::SetWiring {
            monitor: "m32".into(),
            wiring: swapped(),
        });
        assert_eq!(reply, Reply::Rejected(BUSY.into()));
        a.orchestrator().end();
        // Sin archivo (pruebas, agente sin `tandem.toml`) vale en memoria.
        let reply = a.handle(UiCommand::SetWiring {
            monitor: "m32".into(),
            wiring: swapped(),
        });
        assert_eq!(reply, Reply::Done);
        assert_eq!(a.desk().monitor(&"m32".into()).unwrap().wiring, swapped());
    }

    #[test]
    fn get_state_replies_with_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        assert_eq!(a.handle(UiCommand::GetState), Reply::State(a.state()));
    }

    #[test]
    fn starts_unpaired_and_reports_the_peer() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        assert_eq!(
            a.handle(UiCommand::GetPeer),
            Reply::Peer(PeerStatus::Unpaired)
        );
        let mut rx = a.subscribe_peer();
        let paired = PeerStatus::Paired {
            name: "PC".into(),
            link: None,
        };
        a.set_peer_status(paired.clone());
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), paired);
        // Lo mismo otra vez no es un cambio.
        a.set_peer_status(paired.clone());
        assert!(!rx.has_changed().unwrap());
        assert_eq!(a.handle(UiCommand::GetPeer), Reply::Peer(paired));
    }

    #[test]
    fn pairing_without_network_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        assert_eq!(
            a.handle(UiCommand::GetPairing),
            Reply::Pairing(PairingStatus::Idle)
        );
        for cmd in [
            UiCommand::StartPairing,
            UiCommand::ConfirmPairing,
            UiCommand::CancelPairing,
            UiCommand::Unpair,
        ] {
            assert_eq!(a.handle(cmd), Reply::Rejected(NO_NETWORK.into()));
        }
    }

    /// Anota lo que se le pide; `confirm` falla para ver que el motivo llega a la UI.
    #[derive(Default)]
    struct FakePairing(Mutex<Vec<&'static str>>);

    impl PairingControl for FakePairing {
        fn start(&self) -> Result<(), String> {
            self.0.lock().unwrap().push("start");
            Ok(())
        }
        fn confirm(&self) -> Result<(), String> {
            self.0.lock().unwrap().push("confirm");
            Err("no hay código".into())
        }
        fn cancel(&self) -> Result<(), String> {
            self.0.lock().unwrap().push("cancel");
            Ok(())
        }
        fn unpair(&self) -> Result<(), String> {
            self.0.lock().unwrap().push("unpair");
            Ok(())
        }
    }

    #[test]
    fn pairing_commands_go_to_the_network_and_status_is_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        let fake = Arc::new(FakePairing::default());
        a.set_pairing(fake.clone());
        assert_eq!(a.handle(UiCommand::StartPairing), Reply::Done);
        assert_eq!(
            a.handle(UiCommand::ConfirmPairing),
            Reply::Rejected("no hay código".into())
        );
        assert_eq!(a.handle(UiCommand::CancelPairing), Reply::Done);
        assert_eq!(a.handle(UiCommand::Unpair), Reply::Done);
        assert_eq!(
            *fake.0.lock().unwrap(),
            ["start", "confirm", "cancel", "unpair"]
        );

        let mut rx = a.subscribe_pairing();
        a.set_pairing_status(PairingStatus::Searching);
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), PairingStatus::Searching);
        a.set_pairing_status(PairingStatus::Searching);
        assert!(!rx.has_changed().unwrap());
        assert_eq!(
            a.handle(UiCommand::GetPairing),
            Reply::Pairing(PairingStatus::Searching)
        );
    }

    #[test]
    fn get_desk_replies_with_the_configured_desk() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        assert_eq!(
            a.handle(UiCommand::GetDesk),
            Reply::Desk(a.config().desk.clone())
        );
    }

    fn steps_of(events: &[TxnEvent]) -> Vec<Step> {
        match events.first() {
            Some(TxnEvent::Started { steps, .. }) => steps.clone(),
            other => panic!("se esperaba Started: {other:?}"),
        }
    }

    #[tokio::test]
    async fn power_on_only_wakes_the_pc() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(dir.path(), FakeSteps::default(), FakeSteps::default());
        let mut rx = a.subscribe_txns();
        assert!(matches!(
            a.handle(UiCommand::SetPower {
                machine: Machine::Pc,
                on: true
            }),
            Reply::Accepted { .. }
        ));
        let events = until_outcome(&mut rx).await;
        assert_eq!(
            steps_of(&events),
            [Step::PowerOn {
                machine: Machine::Pc
            }]
        );
        let mut expected = State::initial(&a.config().desk, Machine::Mac);
        expected.on.pc = true;
        assert_eq!(a.state(), expected);

        // Ya encendida, pedirlo otra vez no hace nada.
        a.handle(UiCommand::SetPower {
            machine: Machine::Pc,
            on: true,
        });
        assert!(steps_of(&until_outcome(&mut rx).await).is_empty());
    }

    #[tokio::test]
    async fn power_off_returns_monitors_to_the_mac_first() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(dir.path(), FakeSteps::default(), FakeSteps::default());
        let mut s = a.state().confirmed();
        s.on.pc = true;
        s.owner.insert("m34".into(), Machine::Pc);
        a.set_state(s).unwrap();
        let mut rx = a.subscribe_txns();
        assert!(matches!(
            a.handle(UiCommand::SetPower {
                machine: Machine::Pc,
                on: false
            }),
            Reply::Accepted { .. }
        ));
        let steps = steps_of(&until_outcome(&mut rx).await);
        assert!(matches!(
            steps.first(),
            Some(Step::SwitchInput {
                to: Machine::Mac,
                ..
            })
        ));
        assert_eq!(
            steps.last(),
            Some(&Step::PowerOff {
                machine: Machine::Pc
            })
        );
        assert_eq!(
            a.state(),
            State::initial(&a.config().desk, Machine::Mac).confirmed()
        );
    }

    /// Avisos de una transacción hasta su desenlace.
    pub async fn until_outcome(rx: &mut broadcast::Receiver<TxnEvent>) -> Vec<TxnEvent> {
        let mut events = Vec::new();
        loop {
            let ev = rx.recv().await.unwrap();
            let end = matches!(
                ev,
                TxnEvent::Committed { .. }
                    | TxnEvent::RolledBack { .. }
                    | TxnEvent::Inconsistent { .. }
            );
            events.push(ev);
            if end {
                return events;
            }
        }
    }

    fn with_fakes(dir: &Path, local: FakeSteps, peer: FakeSteps) -> Arc<Agent> {
        let a = Agent::with_local_steps(
            fixtures::desk_with_wol(),
            StateStore::new(dir.join("state.json")),
            Arc::new(local),
        );
        a.set_peer(Arc::new(FakePeer(Arc::new(peer))));
        Arc::new(a)
    }

    // T-085: el estado inicial real del puesto.

    fn switched(steps: &[Step]) -> Vec<MonitorId> {
        steps
            .iter()
            .filter_map(|s| match s {
                Step::SwitchInput { monitor, .. } => Some(monitor.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn without_a_saved_state_and_after_a_restart_nothing_is_confirmed() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        assert!(!a.state().is_confirmed());
        let mut s = a.state().confirmed();
        s.owner.insert("m34".into(), Machine::Pc);
        a.set_state(s).unwrap();
        assert!(a.state().is_confirmed());
        // Otro arranque: lo último que se supo, pero sin confirmar (pudo cambiar a mano).
        let again = agent(dir.path()).state();
        assert_eq!(again.owner[&"m34".into()], Machine::Pc);
        assert!(!again.is_confirmed());
        assert_eq!(again.doubt[&"m34".into()], tandem_core::Doubt::Stale);
    }

    #[tokio::test]
    async fn apply_mode_sends_every_input_and_leaves_the_state_confirmed() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(dir.path(), FakeSteps::default(), FakeSteps::default());
        let mut s = a.state();
        s.on.pc = true;
        a.set_state(s).unwrap();
        let mut rx = a.subscribe_txns();
        assert!(matches!(
            a.handle(UiCommand::ApplyMode(Mode::SoloMac)),
            Reply::Accepted { .. }
        ));
        let events = until_outcome(&mut rx).await;
        assert!(matches!(events.last(), Some(TxnEvent::Committed { .. })));
        assert_eq!(
            switched(&steps_of(&events)),
            ["m34".into(), "m32".into()] as [MonitorId; 2]
        );
        let st = a.state();
        assert!(st.is_confirmed() && st.doubt.is_empty(), "{st:?}");
        assert_eq!(st.owner, Mode::SoloMac.assignment(&a.desk()));
        // Confirmado, «Aplicar modo» igual manda todo (por si alguien tocó el menú).
        a.handle(UiCommand::ApplyMode(Mode::SoloMac));
        assert_eq!(switched(&steps_of(&until_outcome(&mut rx).await)).len(), 2);
    }

    #[tokio::test]
    async fn the_first_mode_change_with_an_unconfirmed_state_sends_every_input() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(dir.path(), FakeSteps::default(), FakeSteps::default());
        let mut rx = a.subscribe_txns();
        let shared = Mode::Shared([("m34".into(), Machine::Pc)].into());
        a.handle(UiCommand::SetMode(shared.clone()));
        let steps = steps_of(&until_outcome(&mut rx).await);
        assert_eq!(
            switched(&steps),
            ["m34".into(), "m32".into()] as [MonitorId; 2]
        );
        assert!(a.state().is_confirmed());
        // El siguiente ya planea sobre lo confirmado: pedir lo mismo no hace nada.
        a.handle(UiCommand::SetMode(shared));
        assert!(steps_of(&until_outcome(&mut rx).await).is_empty());
    }

    #[tokio::test]
    async fn saying_what_a_monitor_shows_corrects_the_state_without_switching() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(dir.path(), FakeSteps::default(), FakeSteps::default());
        let mut s = a.state();
        s.on.pc = true;
        a.set_state(s).unwrap();
        let mut rx = a.subscribe_txns();
        let reply = a.handle(UiCommand::SetShown(
            [("m34".into(), Some(Machine::Pc)), ("m32".into(), None)].into(),
        ));
        assert!(matches!(reply, Reply::Accepted { .. }), "{reply:?}");
        let events = until_outcome(&mut rx).await;
        assert!(matches!(events.last(), Some(TxnEvent::Committed { .. })));
        let steps = steps_of(&events);
        assert!(switched(&steps).is_empty(), "sin DDC: {steps:?}");
        // El 34" pasa al escritorio de la PC y sale del de la Mac.
        assert!(steps.contains(&Step::Attach {
            machine: Machine::Pc,
            monitor: "m34".into()
        }));
        assert!(steps.contains(&Step::Detach {
            machine: Machine::Mac,
            monitor: "m34".into()
        }));
        let st = a.state();
        assert_eq!(st.owner[&"m34".into()], Machine::Pc);
        assert!(st.is_confirmed());
        assert_eq!(st.doubt[&"m32".into()], tandem_core::Doubt::Blank);
        // El 32" en negro recibe su entrada en el próximo cambio.
        a.handle(UiCommand::SetMode(Mode::Shared(
            [("m34".into(), Machine::Pc)].into(),
        )));
        assert_eq!(
            switched(&steps_of(&until_outcome(&mut rx).await)),
            ["m32".into()] as [MonitorId; 1]
        );
    }

    #[tokio::test]
    async fn saying_what_a_monitor_shows_is_checked_first() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        std::fs::write(&file, fixtures::DESK).unwrap();
        let a = Arc::new(agent(dir.path()));
        // El 32" solo a la Mac: no puede mostrar la PC.
        a.handle(UiCommand::SetWiring {
            monitor: "m32".into(),
            wiring: [(Machine::Mac, Input::Hdmi)].into(),
        });
        let before = a.state();
        for shows in [
            BTreeMap::new(),
            [("m99".into(), Some(Machine::Pc))].into(),
            [("m32".into(), Some(Machine::Pc))].into(),
        ] {
            let r = a.handle(UiCommand::SetShown(shows));
            assert!(matches!(r, Reply::Rejected(_)), "{r:?}");
        }
        assert_eq!(a.state(), before);
        let _txn = a.orchestrator().try_begin().unwrap();
        assert_eq!(
            a.handle(UiCommand::SetShown(
                [("m34".into(), Some(Machine::Mac))].into()
            )),
            Reply::Rejected(BUSY.into())
        );
    }

    #[tokio::test]
    async fn without_backends_a_mode_change_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        let mut rx = a.subscribe_txns();
        let Reply::Accepted { txn } = a.handle(UiCommand::SetMode(Mode::SoloPc)) else {
            panic!("se esperaba Accepted");
        };
        let events = until_outcome(&mut rx).await;
        let initial = State::initial(&a.config().desk, Machine::Mac);
        assert!(events.iter().any(|e| matches!(
            e,
            TxnEvent::StepFailed { error, .. } if error.contains(STEP_NOT_IMPLEMENTED)
        )));
        assert_eq!(
            events.last(),
            Some(&TxnEvent::RolledBack {
                txn,
                state: initial.clone()
            })
        );
        assert_eq!(a.state(), initial);
    }

    #[tokio::test]
    async fn move_monitor_is_a_shared_mode() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(dir.path(), FakeSteps::default(), FakeSteps::default());
        let mut s = a.state().confirmed();
        s.on.pc = true;
        a.set_state(s.clone()).unwrap();
        let mut rx = a.subscribe_txns();
        let reply = a.handle(UiCommand::MoveMonitor {
            monitor: "m32".into(),
            to: Machine::Pc,
        });
        assert!(matches!(reply, Reply::Accepted { .. }));
        until_outcome(&mut rx).await;
        s.owner.insert("m32".into(), Machine::Pc);
        assert_eq!(a.state(), s);
        // Y queda guardado; al volver a arrancar es lo último que se supo, sin confirmar.
        let again = agent(dir.path()).state();
        assert_eq!(again.owner, s.owner);
        assert!(!again.is_confirmed());
    }

    #[tokio::test]
    async fn same_mode_commits_without_steps() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        a.set_state(a.state().confirmed()).unwrap();
        let mut rx = a.subscribe_txns();
        let Reply::Accepted { txn } = a.handle(UiCommand::SetMode(Mode::SoloMac)) else {
            panic!("se esperaba Accepted");
        };
        assert_eq!(
            until_outcome(&mut rx).await,
            [
                TxnEvent::Started { txn, steps: vec![] },
                TxnEvent::Committed {
                    txn,
                    state: a.state()
                }
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn one_change_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(
            dir.path(),
            FakeSteps {
                delays: vec![(
                    Step::PowerOn {
                        machine: Machine::Pc,
                    },
                    Duration::from_secs(30),
                )],
                ..Default::default()
            },
            FakeSteps::default(),
        );
        let mut rx = a.subscribe_txns();
        assert!(matches!(
            a.handle(UiCommand::SetMode(Mode::SoloPc)),
            Reply::Accepted { .. }
        ));
        assert_eq!(
            a.handle(UiCommand::SetMode(Mode::SoloMac)),
            Reply::Rejected(BUSY.into())
        );
        until_outcome(&mut rx).await;
        // Terminada la primera, se acepta la siguiente.
        assert!(matches!(
            a.handle(UiCommand::SetMode(Mode::SoloMac)),
            Reply::Accepted { .. }
        ));
    }

    #[tokio::test]
    async fn bad_requests_are_rejected_without_touching_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        let mut rx = a.subscribe_txns();
        for cmd in [
            UiCommand::MoveMonitor {
                monitor: "no-existe".into(),
                to: Machine::Pc,
            },
            UiCommand::SetBuiltin(BuiltinDisplay::Mirror),
        ] {
            assert!(matches!(a.handle(cmd), Reply::Rejected(_)));
        }
        assert_eq!(a.state(), State::initial(&a.config().desk, Machine::Mac));
        assert!(rx.try_recv().is_err());
        // Un rechazo no deja el orquestador ocupado.
        assert!(matches!(
            a.handle(UiCommand::SetMode(Mode::SoloMac)),
            Reply::Accepted { .. }
        ));
    }

    #[test]
    fn set_state_publishes_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        let mut rx = a.subscribe();
        let mut s = a.state();
        s.on.pc = true;
        a.set_state(s.clone()).unwrap();
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), s);
        // Otro agente sobre el mismo directorio arranca con ese estado.
        assert_eq!(agent(dir.path()).state(), s);
    }

    // QA ronda 1: `persist` leía el estado y lo guardaba sin exclusión. Con dos
    // `set_state` a la vez, el que leyó el estado viejo podía escribir último y el
    // archivo quedaba con un estado que ya no era el vigente.
    #[test]
    fn concurrent_set_state_persists_the_final_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        let base = a.state();
        for round in 0..20 {
            std::thread::scope(|scope| {
                for i in 0..8u32 {
                    let mut s = base.clone();
                    s.on.pc = (i + round) % 2 == 0;
                    s.builtin_pref = [
                        BuiltinDisplay::Off,
                        BuiltinDisplay::Mirror,
                        BuiltinDisplay::Extend,
                    ][(i % 3) as usize];
                    let a = &a;
                    scope.spawn(move || a.set_state(s).unwrap());
                }
            });
            let saved = StateStore::new(dir.path().join("state.json"))
                .load()
                .unwrap();
            assert_eq!(saved, Some(a.state()), "ronda {round}");
        }
    }

    #[test]
    fn same_state_does_not_notify() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        let rx = a.subscribe();
        a.set_state(a.state()).unwrap();
        assert!(!rx.has_changed().unwrap());
    }
}
