//! El estado vivo del agente y lo que contesta a la UI.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use tandem_core::plan::PlanError;
use tandem_core::{Abilities, Config, Machine, MachineInfo, Plan, State};
use tandem_link::transport::{Connection, ControlStream};
use tandem_proto::{
    Capabilities, Channel, ControlMsg, DeskCapabilities, DeskSync, HubReport, InputStatus,
    KeyProbeStatus, PairingStatus, PeerStatus, PowerEvent, Reply, ScreenRect, SetupInfo, TxnEvent,
    UiCommand,
};
use tokio::sync::{broadcast, mpsc, watch};

mod setup;

use crate::capabilities;
use crate::config::{AgentConfig, InputConfig};
use crate::orchestrator::{LocalSteps, Orchestrator, PeerLink, Unimplemented};
use crate::store::{StateStore, StoreError};

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
    /// El lado de esta máquina. Sale de la identidad: cambia una sola vez, si al emparejar
    /// resulta que esta máquina es el otro lado ([`Agent::adopt_side`]).
    me: watch::Sender<Machine>,
    store: StateStore,
    state: watch::Sender<State>,
    /// Leer el estado y guardarlo tiene que ser una sola operación: si no, un
    /// guardado con el estado viejo puede terminar después de uno con el nuevo.
    persisting: Mutex<()>,
    orch: Orchestrator,
    /// Avisos de energía de la otra máquina que no vienen de una transacción.
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
    /// El puesto compartido que vale ahora (el de `tandem.toml` o el que llegó de la otra).
    desk: watch::Sender<Config>,
    /// `tandem.toml`, donde se guarda lo que cambia; sin él, solo en memoria.
    config_file: Mutex<Option<PathBuf>>,
    /// De dónde salió la configuración y en qué puerto escucha el agente.
    setup: Mutex<SetupInfo>,
    /// Lo que puede esta máquina y lo último que anunció la otra (T-084).
    caps: watch::Sender<DeskCapabilities>,
    /// El reloj del puesto compartido ([`crate::desk_sync`]) y el atajo de bloqueo, que
    /// también es del puesto.
    sync: Mutex<SyncClock>,
    /// El puesto de `tandem.toml` se editó a mano: al conocer el archivo se le escribe la
    /// versión nueva.
    hand_edited: bool,
    /// Los lados del archivo no son los que valen (v0.2, o la identidad dijo otra cosa): al
    /// conocer el archivo se reescriben enteros.
    rewrite_sides: bool,
    /// La prueba «presiona una tecla» del asistente.
    key_probe: watch::Sender<KeyProbeStatus>,
    /// Hay un motor de entrada que arma el [`InputStatus`] (su tarea de estado escucha lo que
    /// el usuario declara); sin él, el agente lo actualiza directamente.
    input_engine: std::sync::atomic::AtomicBool,
    /// Dónde está el teclado según la configuración o el asistente; `None`, sin definir.
    keyboard: watch::Sender<Option<Machine>>,
    /// Cuántas veces el usuario pidió los permisos de macOS (T-090): el motor de entrada
    /// escucha el contador y los pide al sistema.
    permission_requests: watch::Sender<u64>,
    /// Este proceso, para anunciarlo en las capacidades.
    process: tandem_proto::ProcessInfo,
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

impl Agent {
    /// Arranca desde el estado guardado (o el inicial, si no hay), sin backends de
    /// plataforma: todo paso local responde «no implementado».
    pub fn new(cfg: AgentConfig, store: StateStore) -> Self {
        Self::with_local_steps(cfg, store, Arc::new(Unimplemented))
    }

    /// Con los backends reales de esta máquina ([`crate::PowerSteps::for_machine`]).
    pub fn with_platform(cfg: AgentConfig, store: StateStore) -> Self {
        let local = Arc::new(crate::power::PowerSteps::for_machine(&cfg));
        Self::with_local_steps(cfg, store, local)
    }

    /// Como [`Agent::new`], con los pasos que esta máquina sabe hacer.
    pub fn with_local_steps(
        cfg: AgentConfig,
        store: StateStore,
        local: Arc<dyn LocalSteps>,
    ) -> Self {
        let me = cfg.machine;
        let mut state = store.load_or_initial(me);
        // Esta máquina siempre está prendida para sí misma (DESIGN §3.7); la otra, hasta
        // que los enlaces digan otra cosa, como quedó.
        state.on.set(me, true);
        let orch = Orchestrator::new(me, local, cfg.orchestrator.timeouts());
        let mut input = InputStatus::new(cfg.input.keyboard());
        input.defined = cfg.input.keyboard_defined();
        let process = capabilities::current_process();
        // Un puesto editado a mano en `tandem.toml` es un cambio de esta máquina: versión
        // nueva, para que la otra lo adopte y no lo pise con el suyo.
        let hand_edited = crate::desk_sync::edited_by_hand(
            &cfg.desk,
            &cfg.input.lock_shortcut,
            cfg.desk_sum.as_deref(),
            &InputConfig::default().lock_shortcut,
        );
        let info = local_info(&cfg, me, &cfg.desk);
        let here = capabilities::local(&cfg, me, info, &input, &process);
        let sync = if hand_edited {
            tracing::info!("el puesto de tandem.toml cambió a mano: se le manda a la otra máquina");
            SyncClock {
                rev: cfg.desk_rev + 1,
                origin: me,
                lock_shortcut: cfg.input.lock_shortcut.clone(),
            }
        } else {
            SyncClock {
                rev: cfg.desk_rev,
                origin: cfg.desk_origin.unwrap_or(me),
                lock_shortcut: cfg.input.lock_shortcut.clone(),
            }
        };
        Agent {
            me: watch::Sender::new(me),
            rewrite_sides: cfg.rewrite_sides,
            store,
            state: watch::Sender::new(state),
            persisting: Mutex::new(()),
            orch,
            power: broadcast::channel(POWER_EVENT_BUFFER).0,
            // Hasta que la red diga otra cosa, el agente trabaja solo.
            peer_status: watch::Sender::new(PeerStatus::Unpaired),
            pairing_status: watch::Sender::new(PairingStatus::Idle),
            pairing: Mutex::new(None),
            input_status: watch::Sender::new(input),
            inbox: PeerInbox::new(),
            desk: watch::Sender::new(cfg.desk.clone()),
            config_file: Mutex::new(None),
            setup: Mutex::new(SetupInfo {
                config_file: None,
                generated: false,
                port: None,
            }),
            caps: watch::Sender::new(DeskCapabilities { here, peer: None }),
            sync: Mutex::new(sync),
            hand_edited,
            key_probe: watch::Sender::new(KeyProbeStatus::default()),
            keyboard: watch::Sender::new(cfg.input.keyboard),
            input_engine: std::sync::atomic::AtomicBool::new(false),
            permission_requests: watch::Sender::new(0),
            process,
            cfg,
        }
    }

    /// El archivo de la configuración, donde se guarda lo que cambia. Si el puesto se editó
    /// a mano, se le escribe ya la versión nueva.
    pub fn set_config_file(&self, path: PathBuf) {
        self.setup
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .config_file = Some(path.display().to_string());
        *self
            .config_file
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(path);
        if self.rewrite_sides {
            self.save_sides();
        } else if self.hand_edited {
            self.save_desk(&self.desk_sync());
        }
    }

    /// Todo lo que nombra un lado, al archivo (ver [`crate::config_edit::with_sides`]).
    fn save_sides(&self) {
        if let Some(path) = self.config_path()
            && let Err(e) = crate::config_edit::set_sides(
                &path,
                self.machine(),
                self.keyboard_declared(),
                &self.desk_sync(),
            )
        {
            tracing::warn!("no se guardaron los lados en {}: {e}", path.display());
        }
    }

    /// Esta máquina resultó ser el lado `side` (la identidad del par recién emparejado lo
    /// dice, DECISIONS A1). Si ya lo era, nada; si no, se cambian los lados de todo lo que
    /// los nombra (estado, puesto, teclado, capacidades), se guarda y se avisa: el motor de
    /// entrada vuelve a arrancar con el lado nuevo.
    pub fn adopt_side(&self, side: Machine) {
        if self.machine() == side {
            return;
        }
        tracing::info!(lado = %side, "por las claves, esta máquina es el otro lado");
        {
            let mut clock = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
            clock.origin = clock.origin.other();
        }
        self.desk
            .send_modify(|d| *d = std::mem::take(d).swap_sides());
        self.keyboard.send_modify(|k| *k = k.map(Machine::other));
        self.key_probe
            .send_modify(|p| p.heard = p.heard.map(Machine::other));
        self.input_status
            .send_modify(|s| s.server = s.server.other());
        self.orch.set_machine(side);
        self.me.send_replace(side);
        let state = self.state().swapped();
        if let Err(e) = self.set_state(state) {
            tracing::warn!("{e}");
        }
        self.caps.send_modify(|c| c.peer = None);
        self.refresh_capabilities();
        self.save_sides();
    }

    /// Avisa si esta máquina cambia de lado ([`Agent::adopt_side`]).
    pub fn subscribe_machine(&self) -> watch::Receiver<Machine> {
        self.me.subscribe()
    }

    /// Qué máquina es `side` según el puesto, si ya se sabe.
    pub fn machine_info(&self, side: Machine) -> Option<MachineInfo> {
        self.desk.borrow().machines.get(side).cloned()
    }

    /// El nombre de `side` para los textos: el del puesto o «máquina A».
    pub fn machine_name(&self, side: Machine) -> String {
        self.desk.borrow().machines.name(side)
    }

    /// Anota en el puesto lo que se sabe de las dos máquinas: lo que anunció la otra
    /// (`peer`) y lo de esta, si faltaba o les cambió el sistema. El nombre y el color que ya
    /// estén no se pisan: son del usuario. Es un solo cambio del puesto (una versión).
    ///
    /// Pasa al recibir el anuncio de la otra, que llega después de su puesto
    /// ([`Agent::set_peer`]): así la versión nueva sale de la más alta de las dos y no pisa
    /// un cambio de verdad con uno de estos.
    fn learn_machines(&self, peer: Machine, info: &MachineInfo) {
        let mut desk = self.desk();
        let me = self.machine();
        let local = local_info(&self.cfg, me, &desk);
        let mine = learn(&mut desk, me, &local);
        let theirs = learn(&mut desk, peer, info);
        if mine || theirs {
            self.desk.send_replace(desk);
            self.desk_changed();
        }
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

    /// El puesto compartido que vale ahora.
    pub fn desk(&self) -> Config {
        self.desk.borrow().clone()
    }

    /// Avisa cada vez que cambia el puesto.
    pub fn subscribe_desk(&self) -> watch::Receiver<Config> {
        self.desk.subscribe()
    }

    /// Cambia el puesto desde esta máquina (el editor del acomodo, T-096): se valida, sube
    /// la versión, va a `tandem.toml`, a la UI y a la otra máquina.
    pub fn set_desk(&self, desk: Config) -> Result<(), String> {
        desk.validate().map_err(|e| e.to_string())?;
        if self.desk() == desk {
            return Ok(());
        }
        self.desk.send_replace(desk);
        self.desk_changed();
        Ok(())
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

    /// El puesto cambió aquí: versión nueva, a `tandem.toml`, a la UI y a la otra máquina.
    pub fn desk_changed(&self) {
        let next = {
            let mut clock = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
            let next = crate::desk_sync::bump(
                clock.rev,
                self.machine(),
                self.desk(),
                clock.lock_shortcut.clone(),
            );
            clock.rev = next.rev;
            clock.origin = next.origin;
            next
        };
        self.save_desk(&next);
        self.orch.peer().send(ControlMsg::Desk(next));
    }

    /// El puesto que mandó la otra máquina: si es más nuevo, queda aquí, en caliente y en
    /// `tandem.toml`. El motor de entrada recalcula la geometría con él.
    pub fn receive_desk(&self, theirs: DeskSync) {
        let mine = self.desk_sync();
        let Some(next) = crate::desk_sync::receive(&mine, &theirs) else {
            return;
        };
        tracing::info!(
            rev = next.rev,
            origen = %next.origin,
            pantallas = next.desk.screens.len(),
            "puesto actualizado desde la otra máquina"
        );
        {
            let mut clock = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
            clock.rev = next.rev;
            clock.origin = next.origin;
            clock.lock_shortcut = next.lock_shortcut.clone();
        }
        self.save_desk(&next);
        self.desk.send_if_modified(|d| {
            let changed = *d != next.desk;
            *d = next.desk;
            changed
        });
        // El nombre y el color de esta máquina pueden venir de la otra.
        self.refresh_capabilities();
    }

    /// Lo que esta máquina dice de sí misma: su sistema y, del puesto, su nombre y color.
    pub fn local_info(&self) -> MachineInfo {
        local_info(&self.cfg, self.machine(), &self.desk.borrow())
    }

    fn save_desk(&self, sync: &DeskSync) {
        if let Some(path) = self.config_path()
            && let Err(e) = crate::config_edit::set_desk(&path, sync)
        {
            tracing::warn!("no se guardó el puesto en {}: {e}", path.display());
        }
    }

    /// Avisa cada vez que el usuario pide los permisos de macOS.
    pub fn subscribe_permission_requests(&self) -> watch::Receiver<u64> {
        self.permission_requests.subscribe()
    }

    /// Lo que puede cada máquina, para la UI.
    pub fn capabilities(&self) -> DeskCapabilities {
        self.caps.borrow().clone()
    }

    /// Avisa cada vez que cambia lo que puede alguna de las dos.
    pub fn subscribe_capabilities(&self) -> watch::Receiver<DeskCapabilities> {
        self.caps.subscribe()
    }

    /// Vuelve a armar lo que puede esta máquina; si cambió, se lo dice a la UI y a la otra.
    pub fn refresh_capabilities(&self) {
        let here = capabilities::local(
            &self.cfg,
            self.machine(),
            self.local_info(),
            &self.input_status(),
            &self.process,
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
        if peer.machine != self.machine().other() {
            tracing::warn!(
                "anuncio de capacidades de la máquina {} por el enlace de la otra: se ignora",
                peer.machine
            );
            return;
        }
        self.learn_machines(peer.machine, &peer.info);
        self.caps.send_if_modified(|c| {
            let changed = c.peer.as_ref() != Some(&peer);
            c.peer = Some(peer);
            changed
        });
    }

    /// Lo que el plan puede pedirle a cada máquina ([`capabilities::abilities`]).
    pub fn abilities(&self) -> Abilities {
        let caps = self.caps.borrow();
        capabilities::abilities(&caps.here, caps.peer.as_ref())
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

    pub fn orchestrator(&self) -> &Orchestrator {
        &self.orch
    }

    /// Conexión con el otro agente para los pasos que le tocan. Lo primero que va por
    /// ella es su copia del puesto y después lo que puede esta máquina (T-084): el anuncio
    /// puede cambiar el puesto ([`Agent::learn_machines`]), y para entonces la otra ya
    /// adoptó la versión más nueva.
    pub fn set_peer(&self, peer: Arc<dyn PeerLink>) {
        self.orch.set_peer(peer.clone());
        peer.send(ControlMsg::Desk(self.desk_sync()));
        peer.send(ControlMsg::Capabilities(self.capabilities().here));
    }

    /// Avance de las transacciones, para la UI.
    pub fn subscribe_txns(&self) -> broadcast::Receiver<TxnEvent> {
        self.orch.subscribe()
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
        *self.me.borrow()
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
    /// Se publica aunque falle el guardado: el cambio ya ocurrió y la UI tiene que verlo;
    /// el error se devuelve para registrarlo.
    pub fn set_state(&self, mut new: State) -> Result<(), StoreError> {
        // Venga de donde venga (la otra máquina, una transacción), esta está prendida.
        new.on.set(self.machine(), true);
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
    /// Encender y apagar arrancan una transacción en segundo plano y se contestan con
    /// [`Reply::Accepted`]; el avance llega como [`TxnEvent`]. Tiene que llamarse dentro de
    /// un runtime de tokio.
    pub fn handle(self: &Arc<Self>, cmd: UiCommand) -> Reply {
        match cmd {
            UiCommand::GetState => Reply::State(self.state()),
            UiCommand::GetDesk => Reply::Desk(self.desk()),
            UiCommand::GetSetup => Reply::Setup(self.setup()),
            UiCommand::GetCapabilities => Reply::Capabilities(Box::new(self.capabilities())),
            UiCommand::KeyProbe { on } => {
                self.key_probe(on);
                Reply::Done
            }
            UiCommand::HeardKey => {
                self.heard_key();
                Reply::Done
            }
            UiCommand::SetKeyboard(machine) => {
                self.set_keyboard(machine);
                Reply::Done
            }
            UiCommand::PingPeer => self.ping_peer(),
            UiCommand::GetPeer => Reply::Peer(self.peer_status()),
            UiCommand::GetPairing => Reply::Pairing(self.pairing_status()),
            UiCommand::StartPairing => self.pair(|p| p.start()),
            UiCommand::ConfirmPairing => self.pair(|p| p.confirm()),
            UiCommand::CancelPairing => self.pair(|p| p.cancel()),
            UiCommand::Unpair => self.pair(|p| p.unpair()),
            UiCommand::GetInput => Reply::Input(self.input_status()),
            UiCommand::RequestPermissions => {
                self.permission_requests.send_modify(|n| *n += 1);
                Reply::Done
            }
            UiCommand::SetPower { machine, on } => self.start_power(machine, on),
        }
    }

    /// Encender es solo despertar `machine`; apagar es su apagado limpio. Pedir lo que ya
    /// está es una transacción vacía.
    fn start_power(self: &Arc<Self>, machine: Machine, on: bool) -> Reply {
        let name = self.machine_name(machine);
        let what = if on {
            format!("encender la {name}")
        } else {
            format!("apagar la {name}")
        };
        let abilities = self.abilities();
        self.start_plan(&what, move |from| {
            if on {
                Plan::power_on(from, machine, &abilities)
            } else {
                Ok(Plan::power_off(from, machine))
            }
        })
    }

    fn start_plan(
        self: &Arc<Self>,
        what: &str,
        plan_for: impl FnOnce(&State) -> Result<Plan, PlanError>,
    ) -> Reply {
        let Some(txn) = self.orch.try_begin() else {
            return Reply::Rejected(BUSY.into());
        };
        let busy = Busy(self.clone());
        let from = self.state();
        let plan = match plan_for(&from) {
            Ok(p) => p,
            Err(e) => return Reply::Rejected(e.to_string()),
        };
        tracing::info!(txn, what, steps = plan.steps.len(), "transacción");
        tokio::spawn(async move {
            let agent = busy.0.clone();
            let mut state = from;
            let outcome = agent.orch.run(txn, &mut state, &plan).await;
            match &outcome {
                Ok(_) => tracing::info!(txn, "transacción aplicada"),
                Err(e) => tracing::warn!(txn, "transacción fallida: {e}"),
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

/// Lo que esta máquina dice de sí misma: el sistema siempre es el real; el nombre y el color,
/// los del puesto si ya los tiene (los puede cambiar el usuario) o, si no, el nombre del
/// sistema y el color de su lado.
fn local_info(cfg: &AgentConfig, me: Machine, desk: &Config) -> MachineInfo {
    let fresh = MachineInfo::new(me, cfg.os, capabilities::host_name());
    match desk.machines.get(me) {
        Some(saved) => MachineInfo {
            os: cfg.os,
            name: saved.name.clone(),
            color: saved.color.clone(),
        },
        None => fresh,
    }
}

/// Pone `info` como lo que se sabe de `side` si no se sabía nada o el sistema cambió. El
/// nombre y el color que ya estaban se respetan. `true` si cambió el puesto.
fn learn(desk: &mut Config, side: Machine, info: &MachineInfo) -> bool {
    match desk.machines.get(side) {
        Some(saved) if saved.os == info.os => false,
        Some(saved) => {
            let updated = MachineInfo {
                os: info.os,
                ..saved.clone()
            };
            desk.machines.set(side, Some(updated));
            true
        }
        None => {
            desk.machines.set(side, Some(info.clone()));
            true
        }
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

    use tandem_core::{Scale, ScreenSpec, Side, Step};

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
        assert_eq!(a.machine(), Machine::A);
        assert_eq!(a.state(), State::initial(Machine::A));
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

    /// Avisos de una transacción hasta su desenlace.
    pub async fn until_outcome(rx: &mut broadcast::Receiver<TxnEvent>) -> Vec<TxnEvent> {
        let mut events = Vec::new();
        loop {
            let ev = rx.recv().await.unwrap();
            let end = matches!(ev, TxnEvent::Committed { .. } | TxnEvent::Failed { .. });
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

    #[tokio::test]
    async fn power_on_only_wakes_the_pc() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(dir.path(), FakeSteps::default(), FakeSteps::default());
        let mut rx = a.subscribe_txns();
        assert!(matches!(
            a.handle(UiCommand::SetPower {
                machine: Machine::B,
                on: true
            }),
            Reply::Accepted { .. }
        ));
        let events = until_outcome(&mut rx).await;
        assert_eq!(
            steps_of(&events),
            [Step::PowerOn {
                machine: Machine::B
            }]
        );
        let mut expected = State::initial(Machine::A);
        expected.on.b = true;
        assert_eq!(a.state(), expected);

        // Ya encendida, pedirlo otra vez no hace nada.
        a.handle(UiCommand::SetPower {
            machine: Machine::B,
            on: true,
        });
        assert!(steps_of(&until_outcome(&mut rx).await).is_empty());
    }

    #[tokio::test]
    async fn power_off_is_only_the_clean_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(dir.path(), FakeSteps::default(), FakeSteps::default());
        let mut s = a.state();
        s.on.b = true;
        a.set_state(s).unwrap();
        let mut rx = a.subscribe_txns();
        assert!(matches!(
            a.handle(UiCommand::SetPower {
                machine: Machine::B,
                on: false
            }),
            Reply::Accepted { .. }
        ));
        assert_eq!(
            steps_of(&until_outcome(&mut rx).await),
            [Step::PowerOff {
                machine: Machine::B
            }]
        );
        assert_eq!(a.state(), State::initial(Machine::A));
    }

    #[tokio::test]
    async fn without_backends_a_power_change_fails_and_keeps_the_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        let mut rx = a.subscribe_txns();
        let Reply::Accepted { txn } = a.handle(UiCommand::SetPower {
            machine: Machine::B,
            on: true,
        }) else {
            panic!("se esperaba Accepted");
        };
        let events = until_outcome(&mut rx).await;
        let initial = State::initial(Machine::A);
        assert!(events.iter().any(|e| matches!(
            e,
            TxnEvent::StepFailed { error, .. } if error.contains(STEP_NOT_IMPLEMENTED)
        )));
        assert_eq!(
            events.last(),
            Some(&TxnEvent::Failed {
                txn,
                state: initial.clone()
            })
        );
        assert_eq!(a.state(), initial);
    }

    #[tokio::test]
    async fn a_machine_that_cannot_be_woken_is_rejected_without_touching_state() {
        let dir = tempfile::tempdir().unwrap();
        // Sin `[power.wol]` la Mac no tiene con qué despertar a la PC.
        let a = Arc::new(Agent::new(
            fixtures::desk(),
            StateStore::new(dir.path().join("state.json")),
        ));
        let mut rx = a.subscribe_txns();
        assert!(matches!(
            a.handle(UiCommand::SetPower {
                machine: Machine::B,
                on: true
            }),
            Reply::Rejected(_)
        ));
        assert_eq!(a.state(), State::initial(Machine::A));
        assert!(rx.try_recv().is_err());
        // Un rechazo no deja el orquestador ocupado.
        assert!(a.orchestrator().try_begin().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn one_change_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(
            dir.path(),
            FakeSteps {
                delays: vec![(
                    Step::PowerOn {
                        machine: Machine::B,
                    },
                    Duration::from_secs(30),
                )],
                ..Default::default()
            },
            FakeSteps::default(),
        );
        let mut rx = a.subscribe_txns();
        let on = UiCommand::SetPower {
            machine: Machine::B,
            on: true,
        };
        assert!(matches!(a.handle(on.clone()), Reply::Accepted { .. }));
        assert_eq!(a.handle(on.clone()), Reply::Rejected(BUSY.into()));
        until_outcome(&mut rx).await;
        // Terminada la primera, se acepta la siguiente.
        assert!(matches!(a.handle(on), Reply::Accepted { .. }));
    }

    #[test]
    fn set_state_publishes_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        let mut rx = a.subscribe();
        let mut s = a.state();
        s.on.b = true;
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
                    s.on.b = (i + round) % 2 == 0;
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

    fn screen(machine: Machine, id: &str, width: u32) -> ScreenSpec {
        ScreenSpec {
            machine,
            id: id.into(),
            x: 0,
            y: 0,
            width,
            height: 1440,
            scale: Scale::ONE,
        }
    }

    #[test]
    fn a_desk_from_the_other_machine_is_adopted_saved_and_announced() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        std::fs::write(&file, crate::autoconfig::generate(Machine::A)).unwrap();
        let a = agent(dir.path());
        a.set_config_file(file.clone());
        let rx = a.subscribe_desk();
        let mut theirs = a.desk_sync();
        theirs.rev = 5;
        theirs.origin = Machine::B;
        theirs.desk.screens = vec![screen(Machine::B, "34", 3440)];
        theirs.desk.arrangement.a_side = Side::Right;
        a.receive_desk(theirs.clone());
        assert!(rx.has_changed().unwrap());
        assert_eq!(a.desk(), theirs.desk);
        assert_eq!(a.desk_sync(), theirs);
        let saved = AgentConfig::load(&file).unwrap();
        assert_eq!(saved.desk, theirs.desk);
        assert_eq!(saved.desk_rev, 5);
        // Uno más viejo no cambia nada.
        let mut old = theirs.clone();
        old.rev = 2;
        old.desk.screens.clear();
        a.receive_desk(old);
        assert_eq!(a.desk(), theirs.desk);
    }

    #[test]
    fn a_local_desk_change_bumps_the_version_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        let rev = a.desk_sync().rev;
        let mut desk = a.desk();
        desk.screens.push(screen(Machine::A, "laptop", 1512));
        a.set_desk(desk.clone()).unwrap();
        assert_eq!(a.desk(), desk);
        assert_eq!(a.desk_sync().rev, rev + 1);
        assert_eq!(a.desk_sync().origin, Machine::A);
        // Lo mismo otra vez no sube la versión; uno inválido se rechaza.
        a.set_desk(desk.clone()).unwrap();
        assert_eq!(a.desk_sync().rev, rev + 1);
        desk.screens.push(screen(Machine::A, "laptop", 1512));
        assert!(a.set_desk(desk).is_err());
        assert_eq!(a.desk_sync().rev, rev + 1);
    }

    /// Un `[[screens]]` escrito a mano es un cambio de esta máquina: sube la versión y se
    /// guarda con su huella, para que la otra lo adopte y no lo pise.
    #[test]
    fn a_hand_edited_desk_is_a_change_of_this_machine() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        let mut text = crate::autoconfig::generate(Machine::B).replacen(
            "\nmachine = \"b\"\n",
            "\nmachine = \"b\"\ndesk_rev = 4\ndesk_origin = \"a\"\n",
            1,
        );
        text.push_str(
            "\n[[screens]]\nmachine = \"b\"\nid = \"34\"\nx = 0\ny = 0\nwidth = 3440\nheight = 1440\n",
        );
        std::fs::write(&file, &text).unwrap();
        let cfg = AgentConfig::load(&file).unwrap();
        let a = Agent::new(cfg, StateStore::new(dir.path().join("state.json")));
        assert_eq!(a.desk_sync().rev, 5);
        assert_eq!(a.desk_sync().origin, Machine::B);
        a.set_config_file(file.clone());
        let saved = AgentConfig::load(&file).unwrap();
        assert_eq!(saved.desk_rev, 5);
        assert!(saved.desk_sum.is_some());
        // Al volver a arrancar sin tocar nada, ya no es una edición.
        let again = Agent::new(saved, StateStore::new(dir.path().join("state.json")));
        assert_eq!(again.desk_sync().rev, 5);
        // Un archivo recién generado tampoco.
        let fresh = AgentConfig::from_toml(&crate::autoconfig::generate(Machine::B)).unwrap();
        let fresh = Agent::new(fresh, StateStore::new(dir.path().join("other.json")));
        assert_eq!(fresh.desk_sync().rev, 0);
    }

    fn caps_of(side: Machine, os: tandem_core::Os, name: &str) -> Capabilities {
        let cfg = fixtures::desk();
        capabilities::local(
            &cfg,
            side,
            MachineInfo::new(side, os, Some(name.into())),
            &InputStatus::new(Machine::A),
            &tandem_proto::ProcessInfo::default(),
        )
    }

    /// Lo que anuncia la otra (y lo de esta) queda en el puesto, en una sola versión, y el
    /// nombre que ya estaba no se pisa (DECISIONS A1, D17).
    #[test]
    fn the_announced_machines_are_kept_in_the_desk() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        assert!(
            a.desk().machines.is_empty(),
            "sin otra máquina, nada que guardar"
        );
        let rev = a.desk_sync().rev;
        a.set_peer_capabilities(caps_of(Machine::B, tandem_core::Os::MacOs, "Air"));
        let desk = a.desk();
        let theirs = desk.machines.get(Machine::B).unwrap();
        assert_eq!(
            (theirs.os, theirs.name.as_str()),
            (tandem_core::Os::MacOs, "Air")
        );
        assert_eq!(desk.machines.get(Machine::A), Some(&a.local_info()));
        assert_eq!(a.desk_sync().rev, rev + 1, "un solo cambio");
        assert_eq!(a.machine_name(Machine::B), "Air");
        // Otro anuncio con otro nombre no lo pisa (el nombre es del usuario); otro sistema
        // sí se anota.
        a.set_peer_capabilities(caps_of(Machine::B, tandem_core::Os::MacOs, "Otro"));
        assert_eq!(a.machine_name(Machine::B), "Air");
        assert_eq!(a.desk_sync().rev, rev + 1);
        a.set_peer_capabilities(caps_of(Machine::B, tandem_core::Os::Windows, "Otro"));
        let theirs = a.machine_info(Machine::B).unwrap();
        assert_eq!(
            (theirs.os, theirs.name.as_str()),
            (tandem_core::Os::Windows, "Air")
        );
        // Un anuncio con el lado de esta máquina no es de la otra: se ignora.
        a.set_peer_capabilities(caps_of(Machine::A, tandem_core::Os::MacOs, "Yo"));
        assert_ne!(a.machine_name(Machine::A), "Yo");
    }

    /// El sistema que anuncia esta máquina es el suyo, sea el lado que sea; el nombre y el
    /// color, los del puesto si ya los tiene.
    #[test]
    fn this_machine_announces_its_own_system_and_the_desk_name() {
        for os in tandem_core::Os::ALL {
            let dir = tempfile::tempdir().unwrap();
            let mut cfg = fixtures::desk();
            cfg.machine = Machine::B;
            cfg.os = os;
            cfg.desk.machines.b = Some(MachineInfo {
                os: tandem_core::Os::MacOs,
                name: "Escritorio".into(),
                color: "#5fbfb0".into(),
            });
            let a = Agent::new(cfg, StateStore::new(dir.path().join("s.json")));
            let here = a.capabilities().here;
            assert_eq!(here.machine, Machine::B);
            assert_eq!(here.info.os, os);
            assert_eq!(here.info.name, "Escritorio");
            assert_eq!(here.info.color, "#5fbfb0");
        }
    }

    /// Al emparejar, esta máquina resulta ser el otro lado: todo lo que nombra un lado se da
    /// vuelta, se guarda y sigue diciendo lo mismo.
    #[tokio::test]
    async fn adopting_the_other_side_swaps_everything_that_names_a_side() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        std::fs::write(
            &file,
            "machine = \"a\"\n[input]\nkeyboard = \"a\"\n\n[[screens]]\nmachine = \"a\"\nid = \"laptop\"\nx = 0\ny = 0\nwidth = 1512\nheight = 982\n",
        )
        .unwrap();
        let cfg = AgentConfig::load(&file).unwrap();
        let a = Agent::new(cfg, StateStore::new(dir.path().join("state.json")));
        a.set_config_file(file.clone());
        let side = a.subscribe_machine();
        let before = a.desk();
        a.adopt_side(Machine::A);
        assert!(!side.has_changed().unwrap(), "el mismo lado no cambia nada");

        a.adopt_side(Machine::B);
        assert!(side.has_changed().unwrap());
        assert_eq!(a.machine(), Machine::B);
        assert_eq!(a.desk(), before.swap_sides());
        assert_eq!(a.keyboard_declared(), Some(Machine::B));
        assert!(a.state().is_on(Machine::B));
        assert_eq!(a.capabilities().here.machine, Machine::B);
        assert_eq!(a.input_status().server, Machine::B);
        let saved = AgentConfig::load(&file).unwrap();
        assert_eq!(saved.machine, Machine::B);
        assert_eq!(saved.input.keyboard, Some(Machine::B));
        assert_eq!(
            saved.desk.screens_of(Machine::B).next().unwrap().id,
            "laptop"
        );
        // Los pasos se reparten con el lado nuevo.
        let e = a
            .orchestrator()
            .execute_for_peer(&Step::PowerOff {
                machine: Machine::A,
            })
            .await
            .unwrap_err();
        assert!(e.contains("no le corresponde"), "{e}");
    }
}
