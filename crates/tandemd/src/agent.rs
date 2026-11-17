//! El estado vivo del agente y lo que contesta a la UI.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use tandem_core::plan::PlanError;
use tandem_core::{
    Abilities, Config, DeskHistory, DeskPatch, DeskStamp, DeskVersion, HistoryEntry, Machine,
    MachineInfo, Plan, State,
};
use tandem_link::qos::LinkQos;
use tandem_link::transport::{Connection, ControlStream};
use tandem_proto::{
    Capabilities, Channel, ControlMsg, DeskCapabilities, HubReport, InputStatus, KeyProbeStatus,
    ModuleId, PairingStatus, PeerStatus, PowerEvent, Reply, ScreenRect, SetupInfo, TxnEvent,
    UiCommand,
};
use tokio::sync::{broadcast, mpsc, watch};

mod env;
mod modules;
mod monitors;
mod setup;

pub use env::ResourceWatch;

use crate::capabilities;
use crate::config::AgentConfig;
use crate::modules::{ModuleContext, ModuleSwitches, Registry};
use crate::monitors::MonitorHub;
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
    /// Prioridad, tope y pausa de los datos, uno por camino (T-101).
    link_qos: LinkQos,
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
    /// Los módulos que existen en esta máquina (T-098).
    registry: Registry,
    /// Los módulos apagados aquí (copia de `Desk.modules` de este lado, que es la fuente) y
    /// los problemas que reportó cada uno en marcha.
    modules: watch::Sender<ModuleSwitches>,
    /// Los cambios de lo que anuncia esta máquina van a la otra juntos (T-098).
    announce: capabilities::Debounce,
    /// El sello del puesto que vale ahora y las últimas versiones ([`crate::desk_sync`]).
    sync: Mutex<SyncClock>,
    /// Avisos de que la otra máquina cambió el puesto (D5), para la UI.
    desk_changes: broadcast::Sender<DeskChanged>,
    /// El puesto de `tandem.toml` se editó a mano: al conocer el archivo se le escribe la
    /// versión nueva.
    hand_edited: bool,
    /// Los lados del archivo no son los que valen (v0.2, o la identidad dijo otra cosa): al
    /// conocer el archivo se reescriben enteros.
    rewrite_sides: bool,
    /// Un cambio del puesto (y de `tandem.toml`) a la vez: cambiar de lado mientras llega el
    /// puesto de la otra no puede mezclar etiquetas viejas y nuevas ni en memoria ni en el
    /// archivo.
    editing: Mutex<()>,
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
    /// Los monitores del puesto: canales, dueños y lo que leyó cada dueño (T-110).
    monitors: Arc<MonitorHub>,
    /// Entornos detectados y recursos en vivo (T-105).
    env: env::EnvState,
}

#[derive(Debug, Clone)]
struct SyncClock {
    stamp: DeskStamp,
    history: DeskHistory,
}

/// La otra máquina cambió el puesto y su versión ganó: lo que dice el aviso «Cambiado desde
/// X · Deshacer» (`AgentToUi::DeskChanged`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskChanged {
    pub by: Machine,
    pub rev: u64,
    pub summary: String,
    /// La versión que había aquí antes.
    pub undo_to: u64,
}

/// Milisegundos desde la época Unix: la fecha de un cambio del puesto.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
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
/// Avisos de cambios del puesto que se guardan para un suscriptor lento.
const DESK_EVENT_BUFFER: usize = 16;

impl Agent {
    /// Arranca desde el estado guardado (o el inicial, si no hay), sin backends de
    /// plataforma: todo paso local responde «no implementado».
    pub fn new(cfg: AgentConfig, store: StateStore) -> Self {
        Self::with_local_steps(cfg, store, Arc::new(Unimplemented))
    }

    /// Con los backends reales de esta máquina ([`crate::PowerSteps::for_machine`] y el
    /// escaneo de entornos de [`tandem_env::Host::system`]).
    pub fn with_platform(cfg: AgentConfig, store: StateStore) -> Self {
        let local = Arc::new(crate::power::PowerSteps::for_machine(&cfg));
        Self::with_local_steps(cfg, store, local)
            .with_env_host(Arc::new(tandem_env::Host::system()))
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
        let monitors = Arc::new(MonitorHub::new(me));
        orch.set_monitors(monitors.clone());
        let mut input = InputStatus::new(cfg.input.keyboard());
        input.defined = cfg.input.keyboard_defined();
        let process = capabilities::current_process();
        // Un puesto editado a mano en `tandem.toml` es un cambio de esta máquina: versión
        // nueva, para que la otra lo adopte y no lo pise con el suyo. Uno del formato de antes
        // de T-099 tiene una huella vieja: se reescribe sin contarlo como cambio, salvo que
        // haya pasado al puesto algo que era de esta máquina (revisión de #74).
        let hand_edited = cfg.local_to_desk
            || (!cfg.legacy_desk
                && crate::desk_sync::edited_by_hand(&cfg.desk, cfg.desk_sum.as_deref()));
        let info = local_info(&cfg, me, &cfg.desk);
        let registry = Registry::for_os(cfg.os);
        let mut switches = modules::initial_switches(&cfg);
        // Hasta que arranque con un bus ([`Agent::start_monitors`]).
        switches.set_problem(
            tandem_proto::ModuleId::Monitors,
            Some(crate::monitors::NO_DDC.into()),
        );
        let modules = registry.snapshot(&ModuleContext {
            os: info.os,
            input: &input,
            power: &capabilities::power(&cfg, info.os),
            switches: &switches,
        });
        let here = capabilities::local(&cfg, me, info, &input, &process, modules);
        let saved = DeskStamp {
            rev: cfg.desk_rev,
            at: cfg.desk_at,
            by: cfg.desk_origin.unwrap_or(me),
        };
        let stamp = if hand_edited {
            tracing::info!("el puesto de tandem.toml cambió a mano: se le manda a la otra máquina");
            saved.next(me, now_millis())
        } else {
            saved
        };
        let sync = SyncClock {
            stamp,
            history: DeskHistory::new(DeskVersion {
                stamp,
                desk: cfg.desk.clone(),
            }),
        };
        let link_qos = LinkQos::default();
        link_qos.set_prefs(cfg.desk.data);
        Agent {
            link_qos,
            me: watch::Sender::new(me),
            editing: Mutex::new(()),
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
            registry,
            modules: watch::Sender::new(switches),
            announce: capabilities::Debounce::new(capabilities::ANNOUNCE_DEBOUNCE),
            sync: Mutex::new(sync),
            desk_changes: broadcast::channel(DESK_EVENT_BUFFER).0,
            hand_edited,
            key_probe: watch::Sender::new(KeyProbeStatus::default()),
            keyboard: watch::Sender::new(cfg.input.keyboard),
            input_engine: std::sync::atomic::AtomicBool::new(false),
            permission_requests: watch::Sender::new(0),
            process,
            monitors,
            // Sin backends no se encuentra nada; [`Agent::with_env_host`] pone el real.
            env: env::EnvState::new(Arc::new(tandem_env::Host::none())),
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
        let _editing = self.edit();
        if self.rewrite_sides || self.cfg.legacy_desk {
            self.save_sides();
        } else if self.hand_edited {
            self.save_desk(&self.desk_sync());
        }
    }

    fn edit(&self) -> std::sync::MutexGuard<'_, ()> {
        self.editing.lock().unwrap_or_else(PoisonError::into_inner)
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
        let _editing = self.edit();
        if self.machine() == side {
            return;
        }
        tracing::info!(lado = %side, "por las claves, esta máquina es el otro lado");
        let synced = {
            let mut clock = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
            clock.stamp = clock.stamp.swapped();
            clock.history = std::mem::take(&mut clock.history).swapped();
            clock.stamp.rev > 0
        };
        self.desk.send_modify(|d| {
            let arrangement = d.arrangement;
            *d = std::mem::take(d).swap_sides();
            // Un acomodo que nunca se sincronizó no es un lugar físico que conservar: es el
            // que había con los lados nuevos. Darlo vuelta dejaría a las dos instalaciones
            // nuevas con A a la derecha. Sin acomodo declarado no hay nada que dar vuelta.
            if !synced {
                d.arrangement = arrangement;
            }
        });
        self.record_current();
        self.keyboard.send_modify(|k| *k = k.map(Machine::other));
        self.key_probe
            .send_modify(|p| p.heard = p.heard.map(Machine::other));
        self.input_status
            .send_modify(|s| s.server = s.server.other());
        self.orch.set_machine(side);
        self.monitors.set_me(side);
        self.me.send_replace(side);
        let state = self.state().swapped();
        if let Err(e) = self.set_state(state) {
            tracing::warn!("{e}");
        }
        self.caps.send_modify(|c| c.peer = None);
        self.sync_monitor_peer();
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
        let _editing = self.edit();
        let mut desk = self.desk();
        let me = self.machine();
        let local = local_info(&self.cfg, me, &desk);
        let mine = learn(&mut desk, me, &local);
        let theirs = learn(&mut desk, peer, info);
        // Los colores se asignan al conocerse y no se repiten (D17).
        let colors = desk.machines.distinct_colors();
        if mine || theirs || colors {
            self.desk.send_replace(desk);
            self.bump_desk();
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

    /// Prioridad, tope y pausa de los datos con la otra máquina, uno por camino y compartido
    /// por todas las conexiones de ese camino (T-101). La red se lo pone a cada conexión y le
    /// lleva el `[data]` del puesto. Quien mide la latencia de la entrada (T-093) la anota
    /// con `conn.qos().record_input_latency(..)` o `link_qos().get(camino)`, en las dos
    /// máquinas.
    pub fn link_qos(&self) -> &LinkQos {
        &self.link_qos
    }

    /// Avisa cada vez que cambia el puesto.
    pub fn subscribe_desk(&self) -> watch::Receiver<Config> {
        self.desk.subscribe()
    }

    /// Cambia el puesto desde esta máquina (el editor del acomodo, T-096): se valida, sube
    /// la versión, va a `tandem.toml`, a la UI y a la otra máquina.
    pub fn set_desk(&self, desk: Config) -> Result<(), String> {
        desk.validate().map_err(|e| e.to_string())?;
        let _editing = self.edit();
        if self.desk() == desk {
            return Ok(());
        }
        self.desk.send_replace(desk);
        self.bump_desk();
        Ok(())
    }

    /// Un cambio del puesto pedido por la UI (`UiCommand::EditDesk`): se aplica, sube la
    /// versión y va a `tandem.toml`, a la UI y a la otra máquina. Devuelve el puesto que
    /// queda, o por qué no se aplicó.
    pub fn edit_desk(&self, patch: DeskPatch) -> Result<Config, String> {
        if let DeskPatch::Shortcuts(s) = &patch {
            crate::config::shortcut_keys(&s.lock).map_err(|e| e.to_string())?;
        }
        let _editing = self.edit();
        let current = self.desk_sync();
        let next = current
            .edit(patch, self.machine(), now_millis())
            .map_err(|e| e.to_string())?;
        if let Some(next) = next {
            self.commit_local(next);
        }
        Ok(self.desk())
    }

    /// Los interruptores de los módulos de esta máquina salen del puesto (`Desk.modules` de
    /// su lado, D4): se copian cada vez que el puesto cambia, venga de donde venga. Lo que
    /// aquí no se puede encender ni apagar se ignora.
    fn sync_switches(&self) {
        let me = self.machine();
        let desk = self.desk();
        self.modules.send_if_modified(|s| {
            let mut changed = false;
            for id in tandem_proto::ModuleId::ALL {
                if self.registry.check_switch(id).is_err() {
                    continue;
                }
                let on = desk.modules.get(me, id.as_str()) != Some(false);
                if s.set(id, on) {
                    tracing::info!(modulo = ?id, encendido = on, "módulo cambiado");
                    changed = true;
                }
            }
            changed
        });
    }

    /// «Deshacer» (`UiCommand::UndoDesk`): vuelve a la versión `to_rev` del historial con un
    /// cambio nuevo, que gana como cualquier otro.
    pub fn undo_desk(&self, to_rev: u64) -> Result<Config, String> {
        let _editing = self.edit();
        let current = self.desk_sync();
        let next = {
            let clock = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
            clock
                .history
                .undo(&current, to_rev, self.machine(), now_millis())
                .map_err(|e| e.to_string())?
        };
        if let Some(next) = next {
            tracing::info!(a = to_rev, rev = next.stamp.rev, "puesto deshecho");
            self.commit_local(next);
        }
        Ok(self.desk())
    }

    /// Las últimas versiones del puesto, de la más nueva a la más vieja.
    pub fn desk_history(&self) -> Vec<HistoryEntry> {
        self.sync
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .history
            .entries()
    }

    /// Avisa cada vez que la otra máquina cambia el puesto.
    pub fn subscribe_desk_changes(&self) -> broadcast::Receiver<DeskChanged> {
        self.desk_changes.subscribe()
    }

    /// El puesto compartido con su sello, como se le manda a la otra máquina.
    pub fn desk_sync(&self) -> DeskVersion {
        DeskVersion {
            stamp: self.stamp(),
            desk: self.desk(),
        }
    }

    fn stamp(&self) -> DeskStamp {
        self.sync
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .stamp
    }

    /// Anota en el historial la versión que vale ahora.
    fn record_current(&self) {
        let current = self.desk_sync();
        self.sync
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .history
            .record(current);
    }

    fn config_path(&self) -> Option<PathBuf> {
        self.config_file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// El puesto cambió aquí: versión nueva, a `tandem.toml`, a la UI y a la otra máquina.
    pub fn desk_changed(&self) {
        let _editing = self.edit();
        self.bump_desk();
    }

    /// [`Agent::desk_changed`] con el cambio ya tomado ([`Agent::edit`]): el puesto que
    /// vale en memoria pasa a ser una versión nueva de esta máquina.
    fn bump_desk(&self) {
        let next = self
            .desk_sync()
            .change(self.desk(), self.machine(), now_millis());
        self.commit_local(next);
    }

    /// Una versión nueva hecha aquí: queda en memoria, en el historial, en `tandem.toml`, en
    /// la UI y en la otra máquina. Se llama con el cambio tomado ([`Agent::edit`]).
    fn commit_local(&self, next: DeskVersion) {
        {
            let mut clock = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
            clock.stamp = next.stamp;
            clock.history.record(next.clone());
        }
        self.save_desk(&next);
        self.desk.send_if_modified(|d| {
            let changed = *d != next.desk;
            d.clone_from(&next.desk);
            changed
        });
        self.orch.peer().send(ControlMsg::Desk(Box::new(next)));
        self.sync_switches();
        // El nombre y el color de esta máquina (o sus módulos) pueden haber cambiado.
        self.refresh_capabilities();
    }

    /// El puesto que mandó la otra máquina: si gana (D5), queda aquí, en caliente y en
    /// `tandem.toml`, y la UI recibe el aviso con lo que cambió. El motor de entrada
    /// recalcula la geometría con él.
    pub fn receive_desk(&self, theirs: DeskVersion) {
        // Un atajo con teclas que esta versión no conoce no se podría guardar.
        if let Err(e) = crate::config::shortcut_keys(&theirs.desk.shortcuts.lock) {
            tracing::warn!("puesto de la otra máquina ignorado: {e}");
            return;
        }
        let editing = self.edit();
        let mine = self.desk_sync();
        let Some(next) = mine.receive(&theirs) else {
            return;
        };
        tracing::info!(
            rev = next.stamp.rev,
            origen = %next.stamp.by,
            pantallas = next.desk.screens.len(),
            "puesto actualizado desde la otra máquina"
        );
        {
            let mut clock = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
            clock.stamp = next.stamp;
            clock.history.record(next.clone());
        }
        self.save_desk(&next);
        let summary = mine.desk.summary_of_changes(&next.desk);
        let by = next.stamp.by;
        let rev = next.stamp.rev;
        self.desk.send_if_modified(|d| {
            let changed = *d != next.desk;
            *d = next.desk;
            changed
        });
        drop(editing);
        self.sync_switches();
        if let Some(summary) = summary
            && by != self.machine()
        {
            // Sin UI conectada no hay a quién avisar, y eso no es un error.
            let _ = self.desk_changes.send(DeskChanged {
                by,
                rev,
                summary,
                undo_to: mine.stamp.rev,
            });
        }
        // El nombre y el color de esta máquina pueden venir de la otra.
        self.refresh_capabilities();
    }

    /// Lo que esta máquina dice de sí misma: su sistema y, del puesto, su nombre y color.
    pub fn local_info(&self) -> MachineInfo {
        local_info(&self.cfg, self.machine(), &self.desk.borrow())
    }

    /// El puesto al archivo, junto con el lado de ahora: los dos van siempre en la misma
    /// escritura. Se llama con el cambio tomado ([`Agent::edit`]).
    fn save_desk(&self, sync: &DeskVersion) {
        if let Some(path) = self.config_path()
            && let Err(e) = crate::config_edit::set_desk(&path, self.machine(), sync)
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

    /// Vuelve a armar lo que puede esta máquina; si cambió, se lo dice a la UI enseguida y a
    /// la otra con los cambios de [`capabilities::ANNOUNCE_DEBOUNCE`] juntos.
    pub fn refresh_capabilities(&self) {
        let here = self.local_capabilities();
        let changed = self.caps.send_if_modified(|c| {
            let changed = c.here != here;
            c.here = here;
            changed
        });
        if changed {
            let peer = self.orch.peer();
            let caps = self.caps.subscribe();
            // Cada pedido lleva el enlace de ahora y vale el último de la ventana; si el enlace
            // cambia sin otro cambio, el nuevo ya recibió el anuncio ([`Agent::set_peer`]).
            self.announce.schedule(move || {
                let here = caps.borrow().here.clone();
                peer.send(ControlMsg::Capabilities(Box::new(here)));
            });
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
        self.sync_monitor_peer();
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
        peer.send(ControlMsg::Desk(Box::new(self.desk_sync())));
        peer.send(ControlMsg::Capabilities(Box::new(self.capabilities().here)));
        // Lo que leyó de sus monitores, si es dueña de alguno: el enlace anterior pudo
        // perder el último informe.
        let reports = self.monitors.reports_here();
        if !reports.is_empty() {
            peer.send(ControlMsg::Monitors(reports));
        }
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
        if !changed {
            return Ok(());
        }
        // Si la otra se apagó (o volvió), sus monitores cambian de dueño.
        self.sync_monitor_peer();
        self.persist()
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
            UiCommand::EditDesk {
                patch:
                    DeskPatch::Module {
                        machine,
                        module,
                        on,
                    },
            } => match ModuleId::parse(module.trim()) {
                Some(id) => match self.switch_module(machine, id, on) {
                    Reply::Done => Reply::Desk(self.desk()),
                    other => other,
                },
                None => Reply::Rejected(format!("«{module}» no es un módulo de Tandem")),
            },
            UiCommand::EditDesk { patch } => match self.edit_desk(patch) {
                Ok(desk) => Reply::Desk(desk),
                Err(why) => Reply::Rejected(why),
            },
            UiCommand::UndoDesk { to_rev } => match self.undo_desk(to_rev) {
                Ok(desk) => Reply::Desk(desk),
                Err(why) => Reply::Rejected(why),
            },
            UiCommand::GetDeskHistory => Reply::DeskHistory(self.desk_history()),
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
            UiCommand::SetModule {
                machine,
                module,
                on,
            } => self.switch_module(machine, module, on),
            UiCommand::GetMonitors => Reply::Monitors(self.monitors()),
            UiCommand::SwitchMonitor { monitor, input } => {
                self.start_switch_monitor(monitor, input)
            }
            UiCommand::Rescan { machine } => self.rescan_machine(machine),
            // La suscripción es de la conexión: la atiende la IPC ([`Agent::watch_resources`]).
            UiCommand::WatchResources { .. } => {
                Reply::Rejected("los recursos se piden por una conexión de la IPC".into())
            }
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
///
/// Si el puesto que ganó no la trae (la otra lo escribió antes de conocerla), vale la de
/// `tandem.toml` al arrancar: el nombre que eligió el usuario no se pierde.
fn local_info(cfg: &AgentConfig, me: Machine, desk: &Config) -> MachineInfo {
    if let Some(saved) = desk.machines.get(me) {
        return MachineInfo {
            os: cfg.os,
            name: saved.name.clone(),
            color: saved.color.clone(),
        };
    }
    let fresh = MachineInfo::new(me, cfg.os, capabilities::host_name());
    match cfg.desk.machines.get(cfg.machine) {
        Some(first) => MachineInfo {
            name: first.name.clone(),
            // Un color por omisión es el del lado: con el lado nuevo, el nuevo.
            color: if tandem_core::DEFAULT_COLORS.contains(&first.color.as_str()) {
                fresh.color
            } else {
                first.color.clone()
            },
            os: cfg.os,
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
        let mut changes = a.subscribe_desk_changes();
        let before = a.desk_sync().stamp.rev;
        let mut theirs = a.desk_sync();
        theirs.stamp = DeskStamp {
            rev: 5,
            at: 77,
            by: Machine::B,
        };
        theirs.desk.screens = vec![screen(Machine::B, "34", 3440)];
        theirs.desk.arrangement = Some(tandem_core::Arrangement {
            a_side: Side::Right,
            align: tandem_core::Align::Start,
        });
        a.receive_desk(theirs.clone());
        assert!(rx.has_changed().unwrap());
        assert_eq!(a.desk(), theirs.desk);
        assert_eq!(a.desk_sync(), theirs);
        let saved = AgentConfig::load(&file).unwrap();
        assert_eq!(saved.desk, theirs.desk);
        assert_eq!((saved.desk_rev, saved.desk_at), (5, 77));
        // La UI recibe el aviso con lo que cambió y a qué versión volver.
        assert_eq!(
            changes.try_recv().unwrap(),
            DeskChanged {
                by: Machine::B,
                rev: 5,
                summary: "Acomodo y pantallas".into(),
                undo_to: before,
            }
        );
        // Uno más viejo no cambia nada.
        let mut old = theirs.clone();
        old.stamp.rev = 2;
        old.desk.screens.clear();
        a.receive_desk(old);
        assert_eq!(a.desk(), theirs.desk);
        assert!(changes.try_recv().is_err());
    }

    #[test]
    fn a_local_desk_change_bumps_the_version_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        let rev = a.desk_sync().stamp.rev;
        let mut desk = a.desk();
        desk.screens.push(screen(Machine::A, "laptop", 1512));
        a.set_desk(desk.clone()).unwrap();
        assert_eq!(a.desk(), desk);
        assert_eq!(a.desk_sync().stamp.rev, rev + 1);
        assert_eq!(a.desk_sync().stamp.by, Machine::A);
        assert!(a.desk_sync().stamp.at > 0, "con fecha");
        // Lo mismo otra vez no sube la versión; uno inválido se rechaza.
        a.set_desk(desk.clone()).unwrap();
        assert_eq!(a.desk_sync().stamp.rev, rev + 1);
        desk.screens.push(screen(Machine::A, "laptop", 1512));
        assert!(a.set_desk(desk).is_err());
        assert_eq!(a.desk_sync().stamp.rev, rev + 1);
    }

    /// DESIGN §4.4: la UI edita con parches, puede deshacer a cualquier versión del
    /// historial y ver el historial; lo inválido se rechaza con el motivo.
    #[tokio::test]
    async fn the_ui_edits_undoes_and_lists_the_desk() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        std::fs::write(&file, crate::autoconfig::generate(Machine::A)).unwrap();
        let a = Arc::new(agent(dir.path()));
        a.set_config_file(file.clone());
        let start = a.desk_sync();
        let arr = tandem_core::Arrangement {
            a_side: Side::Up,
            align: tandem_core::Align::End,
        };
        let Reply::Desk(d) = a.handle(UiCommand::EditDesk {
            patch: DeskPatch::Arrangement(Some(arr)),
        }) else {
            panic!("sin puesto");
        };
        assert_eq!(d.arrangement, Some(arr));
        let lock = tandem_core::Shortcuts {
            lock: vec!["Bloq".into()],
        };
        assert!(matches!(
            a.handle(UiCommand::EditDesk {
                patch: DeskPatch::Shortcuts(lock)
            }),
            Reply::Rejected(why) if why.contains("Bloq")
        ));
        assert!(matches!(
            a.handle(UiCommand::EditDesk {
                patch: DeskPatch::Machine {
                    side: Machine::B,
                    name: Some("PC".into()),
                    color: None,
                }
            }),
            Reply::Rejected(_)
        ));
        let prefs = tandem_core::InputPrefs {
            drag_files: false,
            ..tandem_core::InputPrefs::default()
        };
        a.handle(UiCommand::EditDesk {
            patch: DeskPatch::Prefs(prefs),
        });
        assert_eq!(a.desk_sync().stamp.rev, start.stamp.rev + 2);
        let saved = AgentConfig::load(&file).unwrap();
        assert!(!saved.desk.prefs.drag_files);
        assert_eq!(saved.desk.arrangement, Some(arr));

        let Reply::DeskHistory(h) = a.handle(UiCommand::GetDeskHistory) else {
            panic!("sin historial");
        };
        let revs: Vec<u64> = h.iter().map(|e| e.stamp.rev).collect();
        assert_eq!(
            revs,
            [start.stamp.rev + 2, start.stamp.rev + 1, start.stamp.rev]
        );
        assert_eq!(h[0].summary, "Teclado y mouse");

        // Deshacer hasta el principio: exactamente aquel puesto, como versión nueva.
        let Reply::Desk(back) = a.handle(UiCommand::UndoDesk {
            to_rev: start.stamp.rev,
        }) else {
            panic!("no deshizo");
        };
        assert_eq!(back, start.desk);
        assert_eq!(a.desk_sync().stamp.rev, start.stamp.rev + 3);
        assert_eq!(AgentConfig::load(&file).unwrap().desk, start.desk);
        assert!(matches!(
            a.handle(UiCommand::UndoDesk { to_rev: 999 }),
            Reply::Rejected(_)
        ));
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
        assert_eq!(a.desk_sync().stamp.rev, 5);
        assert_eq!(a.desk_sync().stamp.by, Machine::B);
        a.set_config_file(file.clone());
        let saved = AgentConfig::load(&file).unwrap();
        assert_eq!(saved.desk_rev, 5);
        assert!(saved.desk_sum.is_some());
        // Al volver a arrancar sin tocar nada, ya no es una edición.
        let again = Agent::new(saved, StateStore::new(dir.path().join("state.json")));
        assert_eq!(again.desk_sync().stamp.rev, 5);
        // Un archivo recién generado tampoco.
        let fresh = AgentConfig::from_toml(&crate::autoconfig::generate(Machine::B)).unwrap();
        let fresh = Agent::new(fresh, StateStore::new(dir.path().join("other.json")));
        assert_eq!(fresh.desk_sync().stamp.rev, 0);
    }

    /// Revisión de #74: el portapapeles apagado de antes de T-099 era de esta máquina y ahora
    /// está en el puesto. Es un cambio suyo: sube la versión (con su lado y fecha) para que la
    /// otra lo adopte, y queda guardado así.
    #[test]
    fn a_local_setting_moved_to_the_desk_bumps_the_version() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        std::fs::write(
            &file,
            "machine = \"b\"\ndesk_rev = 3\ndesk_origin = \"a\"\n\n[input]\nclipboard = false\n",
        )
        .unwrap();
        let cfg = AgentConfig::load(&file).unwrap();
        assert!(cfg.local_to_desk);
        let a = Agent::new(cfg, StateStore::new(dir.path().join("state.json")));
        let stamp = a.desk_sync().stamp;
        assert_eq!((stamp.rev, stamp.by), (4, Machine::B));
        assert!(stamp.at > 0);
        assert_eq!(a.desk().modules.get(Machine::B, "clipboard"), Some(false));
        a.set_config_file(file.clone());
        let saved = AgentConfig::load(&file).unwrap();
        assert_eq!((saved.desk_rev, saved.desk_origin), (4, Some(Machine::B)));
        assert!(!saved.local_to_desk && !saved.legacy_desk);
        // Al volver a arrancar ya no es un cambio.
        let again = Agent::new(saved, StateStore::new(dir.path().join("state.json")));
        assert_eq!(again.desk_sync().stamp.rev, 4);
    }

    /// Un `tandem.toml` de antes de T-099 (atajo en `[input]`, desplazamiento en el acomodo)
    /// se reescribe en el formato nuevo sin contar como un cambio de nadie.
    #[test]
    fn a_desk_from_before_t099_is_rewritten_without_a_new_version() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        std::fs::write(
            &file,
            "machine = \"a\"\ndesk_rev = 3\ndesk_origin = \"b\"\ndesk_sum = \"0123456789abcdef\"\n\n[input]\nlock_shortcut = [\"KeyL\"]\n\n[arrangement]\na_side = \"left\"\noffset = 0\n",
        )
        .unwrap();
        let cfg = AgentConfig::load(&file).unwrap();
        let a = Agent::new(cfg, StateStore::new(dir.path().join("state.json")));
        assert_eq!(a.desk_sync().stamp.rev, 3);
        a.set_config_file(file.clone());
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            !text.contains("lock_shortcut") && !text.contains("offset"),
            "{text}"
        );
        let saved = AgentConfig::load(&file).unwrap();
        assert!(!saved.legacy_desk);
        assert_eq!((saved.desk_rev, saved.desk_origin), (3, Some(Machine::B)));
        assert_eq!(saved.desk.shortcuts.lock, ["KeyL"]);
        assert_eq!(saved.desk.arrangement, None);
        let again = Agent::new(saved, StateStore::new(dir.path().join("state.json")));
        assert_eq!(again.desk_sync().stamp.rev, 3);
    }

    fn caps_of(side: Machine, os: tandem_core::Os, name: &str) -> Capabilities {
        let cfg = fixtures::desk();
        capabilities::local(
            &cfg,
            side,
            MachineInfo::new(side, os, Some(name.into())),
            &InputStatus::new(Machine::A),
            &tandem_proto::ProcessInfo::default(),
            Vec::new(),
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
        let rev = a.desk_sync().stamp.rev;
        a.set_peer_capabilities(caps_of(Machine::B, tandem_core::Os::MacOs, "Air"));
        let desk = a.desk();
        let theirs = desk.machines.get(Machine::B).unwrap();
        assert_eq!(
            (theirs.os, theirs.name.as_str()),
            (tandem_core::Os::MacOs, "Air")
        );
        assert_eq!(desk.machines.get(Machine::A), Some(&a.local_info()));
        assert_eq!(a.desk_sync().stamp.rev, rev + 1, "un solo cambio");
        assert_eq!(a.machine_name(Machine::B), "Air");
        // Otro anuncio con otro nombre no lo pisa (el nombre es del usuario); otro sistema
        // sí se anota.
        a.set_peer_capabilities(caps_of(Machine::B, tandem_core::Os::MacOs, "Otro"));
        assert_eq!(a.machine_name(Machine::B), "Air");
        assert_eq!(a.desk_sync().stamp.rev, rev + 1);
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
        // El acomodo de omisión no se da vuelta (A sigue a la izquierda de B).
        let mut want = before.clone().swap_sides();
        want.arrangement = before.arrangement;
        assert_eq!(a.desk(), want);
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

    /// Revisión del PR #68: cambiar de lado mientras llegan el puesto y el anuncio de la otra
    /// escribía `tandem.toml` a la vez, y el archivo podía quedar con un lado y las pantallas
    /// etiquetadas desde el otro (al rearrancar, los lados se daban vuelta dos veces).
    #[test]
    fn changing_side_while_the_peer_talks_keeps_the_file_coherent() {
        for round in 0..30 {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("tandem.toml");
            std::fs::write(
                &file,
                "machine = \"a\"\n\n[[screens]]\nmachine = \"a\"\nid = \"laptop\"\nx = 0\ny = 0\nwidth = 1512\nheight = 982\n",
            )
            .unwrap();
            let a = Agent::new(
                AgentConfig::load(&file).unwrap(),
                StateStore::new(dir.path().join("state.json")),
            );
            a.set_config_file(file.clone());
            // Lo que manda la otra (que es A): su puesto, con las pantallas de las dos.
            let mut theirs = a.desk_sync();
            theirs.stamp.rev = 9;
            theirs.stamp.by = Machine::A;
            theirs.desk.screens = vec![
                screen(Machine::A, "torre", 2560),
                screen(Machine::B, "laptop", 1512),
            ];
            let barrier = std::sync::Barrier::new(3);
            std::thread::scope(|s| {
                s.spawn(|| {
                    barrier.wait();
                    a.adopt_side(Machine::B);
                });
                s.spawn(|| {
                    barrier.wait();
                    a.receive_desk(theirs.clone());
                });
                s.spawn(|| {
                    barrier.wait();
                    a.set_peer_capabilities(caps_of(Machine::A, tandem_core::Os::Windows, "Torre"));
                });
            });
            let saved = AgentConfig::load(&file).unwrap();
            assert_eq!(saved.machine, a.machine(), "vuelta {round}");
            assert_eq!(saved.desk, a.desk(), "vuelta {round}");
            // Al rearrancar con el lado de las claves no hay un segundo cambio de lados.
            assert_eq!(saved.clone().with_side(Machine::B).desk, saved.desk);
        }
    }

    /// Revisión del PR #68: dos instalaciones nuevas (acomodo por omisión) no quedan con A a
    /// la derecha porque una de las dos cambió de lado.
    #[test]
    fn the_default_arrangement_is_not_flipped_when_changing_side() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        assert_eq!(a.desk().arrangement, None);
        a.adopt_side(Machine::B);
        assert_eq!(a.desk().arrangement, None);
        // Uno declarado y ya sincronizado sí se conserva físicamente.
        let dir = tempfile::tempdir().unwrap();
        let b = agent(dir.path());
        let mut desk = b.desk();
        desk.arrangement = Some(tandem_core::Arrangement {
            a_side: Side::Right,
            align: tandem_core::Align::Start,
        });
        b.set_desk(desk).unwrap();
        b.adopt_side(Machine::B);
        let arr = b.desk().arrangement.unwrap();
        assert_eq!(
            (arr.a_side, arr.align),
            (Side::Left, tandem_core::Align::Start)
        );
    }
}
