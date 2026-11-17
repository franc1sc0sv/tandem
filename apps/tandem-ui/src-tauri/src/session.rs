//! Conexión viva con `tandemd` para la escena principal.
//!
//! La ventana no pregunta: la sesión mantiene una conexión abierta, pide el puesto y el
//! estado al conectar y después reenvía lo que el agente empuja (estado, avance de las
//! transacciones, emparejamiento, teclado y mouse). Si el agente no está o se cae, la ventana
//! lo ve como [`Snapshot::Unavailable`] y la sesión reintenta sola.
//!
//! Es genérica sobre el transporte y sobre a quién se le avisa ([`Sink`]): las pruebas
//! usan la IPC en memoria con un agente real; la app, el socket o pipe y los eventos de
//! Tauri.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tandem_core::{Config, Machine, State, Step};
use tandem_proto::{
    AgentToUi, Capabilities, DeskCapabilities, InputPermission, InputStatus, KeyProbeStatus, Link,
    PairingStatus, PeerStatus, PowerEvent, Reply, SetupInfo, TxnEvent, UiCommand,
};
use tandemd::ipc::{IpcClient, IpcError};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Notify, mpsc, oneshot, watch};

use crate::status::{CLOSED, QUERY_TIMEOUT, explain};

/// Cada cuánto se vuelve a intentar si no hay agente. Corto: la ventana tiene que
/// recuperarse sola en cuanto `tandemd` arranca.
pub const RETRY_EVERY: Duration = Duration::from_secs(2);

/// Todo lo que la escena necesita para dibujarse.
// Una sola escena conectada viva a la vez: no vale la pena una caja más.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Snapshot {
    Connecting,
    Connected {
        /// Máquina en la que corre el agente (y esta ventana).
        agent: Machine,
        /// El puesto compartido: el acomodo de pantallas declarado (DESIGN §7).
        desk: Config,
        state: State,
        /// El otro agente: sin par, la ventana avisa que esta máquina trabaja sola.
        peer: PeerView,
        /// En qué va el emparejamiento.
        pairing: PairingView,
        /// El teclado y mouse compartidos: el chip de cada máquina.
        input: InputView,
        /// Lo que puede cada máquina (T-084): la escena se dibuja con esto, no con el
        /// nombre de la máquina.
        caps: Box<CapsView>,
    },
    Unavailable {
        endpoint: Option<String>,
        reason: String,
    },
}

/// [`PeerStatus`] en JSON para el frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PeerView {
    Unpaired,
    Paired {
        name: String,
        /// `None` si ningún enlace ve al otro agente.
        link: Option<LinkView>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkView {
    Usb4,
    Ethernet,
    Wifi,
}

impl From<PeerStatus> for PeerView {
    fn from(p: PeerStatus) -> Self {
        match p {
            PeerStatus::Unpaired => PeerView::Unpaired,
            PeerStatus::Paired { name, link } => PeerView::Paired {
                name,
                link: link.map(|l| match l {
                    Link::Usb4 => LinkView::Usb4,
                    Link::Ethernet => LinkView::Ethernet,
                    Link::Wifi => LinkView::Wifi,
                }),
            },
        }
    }
}

/// [`PairingStatus`] en JSON para el frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PairingView {
    Idle,
    Searching,
    Comparing {
        code: String,
        peer: String,
        confirmed: bool,
    },
    Done {
        peer: String,
    },
    Failed {
        reason: String,
    },
}

impl From<PairingStatus> for PairingView {
    fn from(p: PairingStatus) -> Self {
        match p {
            PairingStatus::Idle => PairingView::Idle,
            PairingStatus::Searching => PairingView::Searching,
            PairingStatus::Comparing {
                code,
                peer,
                confirmed,
            } => PairingView::Comparing {
                code,
                peer,
                confirmed,
            },
            PairingStatus::Done { peer } => PairingView::Done { peer },
            PairingStatus::Failed { reason } => PairingView::Failed { reason },
        }
    }
}

/// [`InputStatus`] en JSON para el frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InputView {
    /// La máquina que tiene el teclado y el mouse.
    pub server: Machine,
    /// Si el hub está en esta máquina; `None` sin detección.
    pub hub_here: Option<bool>,
    /// La sesión con el otro agente está establecida.
    pub session: bool,
    /// El cursor está bloqueado en su máquina.
    pub locked: bool,
    /// Permisos de macOS que faltan: la ventana lo avisa.
    pub missing: Vec<PermissionView>,
    pub problem: Option<String>,
    /// Se sabe dónde está el teclado (hub, configuración o asistente). Si no, la ventana
    /// dice «sin definir» y nunca supone la Mac (T-087).
    pub defined: bool,
}

/// [`DeskCapabilities`] en JSON para el frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapsView {
    /// Esta máquina (siempre encendida para sí misma).
    pub here: MachineView,
    /// La otra, según su último anuncio; `None` si nunca se la vio.
    pub peer: Option<MachineView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MachineView {
    pub machine: Machine,
    /// Nombre, sistema (`macos`/`windows`) y color de acento: la UI no tiene textos fijos
    /// «Mac»/«PC» (T-097).
    pub name: String,
    pub os: tandem_core::Os,
    pub color: String,
    pub hub: Option<bool>,
    pub input: InputCapsView,
    pub power: PowerCapsView,
    pub services: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InputCapsView {
    pub capture: bool,
    pub inject: bool,
    pub missing: Vec<PermissionView>,
    /// El binario al que macOS concede los permisos (T-090).
    pub process: ProcessView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcessView {
    pub exe: String,
    pub pid: u32,
    pub started: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PowerCapsView {
    pub wake_peer: bool,
    pub wakeable: bool,
    pub power_off: bool,
}

fn permission_view(p: InputPermission) -> PermissionView {
    match p {
        InputPermission::Accessibility => PermissionView::Accessibility,
        InputPermission::InputMonitoring => PermissionView::InputMonitoring,
    }
}

impl From<Capabilities> for MachineView {
    fn from(c: Capabilities) -> Self {
        MachineView {
            machine: c.machine,
            name: c.info.name,
            os: c.info.os,
            color: c.info.color,
            hub: c.hub,
            input: InputCapsView {
                capture: c.input.capture,
                inject: c.input.inject,
                missing: c.input.missing.into_iter().map(permission_view).collect(),
                process: ProcessView {
                    exe: c.input.process.exe,
                    pid: c.input.process.pid,
                    started: c.input.process.started,
                },
            },
            power: PowerCapsView {
                wake_peer: c.power.wake_peer,
                wakeable: c.power.wakeable,
                power_off: c.power.power_off,
            },
            services: c.services,
        }
    }
}

impl From<DeskCapabilities> for CapsView {
    fn from(c: DeskCapabilities) -> Self {
        CapsView {
            here: c.here.into(),
            peer: c.peer.map(Into::into),
        }
    }
}

/// Lo que anunciaría un agente de `machine` con el puesto de las pruebas, sin par visto.
#[cfg(test)]
pub fn test_caps(machine: Machine) -> Box<CapsView> {
    let mut cfg =
        tandemd::AgentConfig::from_toml("machine = \"a\"").unwrap_or_else(|e| panic!("{e}"));
    cfg.machine = machine;
    let os = match machine {
        Machine::A => tandem_core::Os::MacOs,
        Machine::B => tandem_core::Os::Windows,
    };
    let here = tandemd::capabilities::local(
        &cfg,
        machine,
        tandem_core::MachineInfo::new(machine, os, None),
        &InputStatus::new(machine),
        &Default::default(),
        Vec::new(),
    );
    Box::new(CapsView {
        here: here.into(),
        peer: None,
    })
}

/// Los mismos nombres que la guía de permisos ([`crate::setup`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionView {
    Accessibility,
    InputMonitoring,
}

impl From<InputStatus> for InputView {
    fn from(s: InputStatus) -> Self {
        InputView {
            server: s.server,
            hub_here: s.hub_here,
            session: s.session,
            locked: s.locked,
            missing: s.missing.into_iter().map(permission_view).collect(),
            problem: s.problem,
            defined: s.defined,
        }
    }
}

/// Avisos que no son estado: se muestran una vez y pasan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Notice {
    Txn {
        event: TxnView,
    },
    /// La prueba «presiona una tecla» del asistente (T-087).
    KeyProbe {
        event: KeyProbeView,
    },
    /// La otra máquina se dio por apagada sin que nadie la apagara (corte de corriente o
    /// suspendida): `reason` se muestra tal cual.
    Power {
        machine: Machine,
        reason: String,
    },
}

/// [`KeyProbeStatus`] en JSON para el frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct KeyProbeView {
    pub armed: bool,
    pub heard: Option<Machine>,
}

impl From<PowerEvent> for Notice {
    fn from(e: PowerEvent) -> Self {
        match e {
            PowerEvent::PeerLost { machine, reason } => Notice::Power { machine, reason },
        }
    }
}

/// [`TxnEvent`] en JSON legible para el frontend (los pasos con su etiqueta `step`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TxnView {
    Started {
        txn: u64,
        steps: Vec<Step>,
    },
    StepStarted {
        txn: u64,
        index: u32,
    },
    StepDone {
        txn: u64,
        index: u32,
    },
    StepFailed {
        txn: u64,
        index: u32,
        error: String,
    },
    Committed {
        txn: u64,
        state: State,
    },
    /// Falló un paso: `state` es lo que quedó hecho.
    Failed {
        txn: u64,
        state: State,
    },
}

impl From<TxnEvent> for TxnView {
    fn from(e: TxnEvent) -> Self {
        match e {
            TxnEvent::Started { txn, steps } => TxnView::Started { txn, steps },
            TxnEvent::StepStarted { txn, index } => TxnView::StepStarted { txn, index },
            TxnEvent::StepDone { txn, index } => TxnView::StepDone { txn, index },
            TxnEvent::StepFailed { txn, index, error } => TxnView::StepFailed { txn, index, error },
            TxnEvent::Committed { txn, state } => TxnView::Committed { txn, state },
            TxnEvent::Failed { txn, state } => TxnView::Failed { txn, state },
        }
    }
}

/// Lo que la ventana puede pedir.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Encender o apagar `machine` (la otra, en la escena).
    SetPower {
        machine: Machine,
        on: bool,
    },
    StartPairing,
    ConfirmPairing,
    CancelPairing,
    Unpair,
    KeyProbe {
        on: bool,
    },
    /// La ventana oyó una tecla durante la prueba.
    HeardKey,
    SetKeyboard {
        machine: Machine,
    },
    PingPeer,
    /// «Pedir permisos» (T-090): macOS muestra su diálogo a nombre de `tandemd`.
    RequestPermissions,
    /// De dónde salió la configuración; contesta [`ActionResult::Setup`].
    GetSetup,
}

impl From<Action> for UiCommand {
    fn from(a: Action) -> Self {
        match a {
            Action::SetPower { machine, on } => UiCommand::SetPower { machine, on },
            Action::StartPairing => UiCommand::StartPairing,
            Action::ConfirmPairing => UiCommand::ConfirmPairing,
            Action::CancelPairing => UiCommand::CancelPairing,
            Action::Unpair => UiCommand::Unpair,
            Action::KeyProbe { on } => UiCommand::KeyProbe { on },
            Action::HeardKey => UiCommand::HeardKey,
            Action::SetKeyboard { machine } => UiCommand::SetKeyboard(machine),
            Action::PingPeer => UiCommand::PingPeer,
            Action::RequestPermissions => UiCommand::RequestPermissions,
            Action::GetSetup => UiCommand::GetSetup,
        }
    }
}

/// La respuesta del agente a una [`Action`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionResult {
    /// Arrancó la transacción `txn`; su avance llega como [`Notice::Txn`].
    Accepted {
        txn: u64,
    },
    Done,
    Rejected {
        reason: String,
    },
    /// No hay agente con quien hablar.
    Unavailable {
        reason: String,
    },
    /// La configuración del agente: archivo, si la generó él y su puerto.
    Setup {
        setup: SetupInfo,
    },
    /// Lo que tardó el ping por el enlace.
    Pong {
        link: LinkView,
        millis: u32,
    },
}

impl From<Reply> for ActionResult {
    fn from(r: Reply) -> Self {
        match r {
            Reply::Accepted { txn } => ActionResult::Accepted { txn },
            Reply::Rejected(reason) => ActionResult::Rejected { reason },
            Reply::Setup(setup) => ActionResult::Setup { setup },
            Reply::Pong { link, millis } => ActionResult::Pong {
                link: match link {
                    Link::Usb4 => LinkView::Usb4,
                    Link::Ethernet => LinkView::Ethernet,
                    Link::Wifi => LinkView::Wifi,
                },
                millis,
            },
            Reply::Done
            | Reply::State(_)
            | Reply::Desk(_)
            | Reply::Peer(_)
            | Reply::Pairing(_)
            | Reply::Input(_)
            | Reply::Capabilities(_)
            | Reply::DeskHistory(_) => ActionResult::Done,
        }
    }
}

/// A quién se le cuenta lo que pasa (en la app, la ventana por eventos de Tauri).
pub trait Sink: Send + Sync + 'static {
    fn snapshot(&self, snapshot: &Snapshot);
    fn notice(&self, notice: &Notice);
}

struct Pending {
    command: UiCommand,
    reply: oneshot::Sender<ActionResult>,
}

impl Pending {
    fn answer(self, r: ActionResult) {
        // Si la ventana ya no espera la respuesta, no hay a quién dársela.
        let _ = self.reply.send(r);
    }
}

/// Lo que la app guarda para hablar con la sesión.
#[derive(Debug, Clone)]
pub struct Handle {
    actions: mpsc::Sender<Pending>,
    snapshot: watch::Receiver<Snapshot>,
    retry: Arc<Notify>,
}

impl Handle {
    /// El último estado conocido, para una ventana que recién se abre.
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.borrow().clone()
    }

    pub async fn act(&self, action: Action) -> ActionResult {
        let (reply, answer) = oneshot::channel();
        let pending = Pending {
            command: action.into(),
            reply,
        };
        if self.actions.send(pending).await.is_err() {
            let reason = match self.snapshot() {
                Snapshot::Unavailable { reason, .. } => reason,
                _ => "la sesión con tandemd terminó".into(),
            };
            return ActionResult::Unavailable { reason };
        }
        answer.await.unwrap_or_else(|_| ActionResult::Unavailable {
            reason: CLOSED.into(),
        })
    }

    /// Reintenta ya, sin esperar a [`RETRY_EVERY`].
    pub fn retry_now(&self) {
        self.retry.notify_one();
    }
}

/// Una sesión que no puede empezar (por ejemplo, sin saber dónde escucha el agente):
/// la ventana muestra la razón y todo pedido se contesta con ella.
pub fn unavailable(endpoint: Option<String>, reason: String) -> Handle {
    let (actions, _) = mpsc::channel(1);
    let (_, snapshot) = watch::channel(Snapshot::Unavailable { endpoint, reason });
    Handle {
        actions,
        snapshot,
        retry: Arc::new(Notify::new()),
    }
}

/// Crea la sesión. El futuro devuelto es la sesión misma: corre hasta que se suelta
/// el último [`Handle`].
///
/// `connect` abre una conexión nueva cada vez que se llama; `endpoint` es solo para
/// decirle al usuario dónde se buscó al agente.
pub fn start<C, F, S>(
    connect: C,
    endpoint: Option<String>,
    sink: Arc<dyn Sink>,
) -> (Handle, impl Future<Output = ()> + Send)
where
    C: FnMut() -> F + Send + 'static,
    F: Future<Output = Result<IpcClient<S>, IpcError>> + Send,
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let (actions_tx, actions) = mpsc::channel(16);
    let (snapshot_tx, snapshot) = watch::channel(Snapshot::Connecting);
    let retry = Arc::new(Notify::new());
    let handle = Handle {
        actions: actions_tx,
        snapshot,
        retry: retry.clone(),
    };
    let runner = Runner {
        endpoint,
        sink,
        snapshot: snapshot_tx,
        actions,
        retry,
    };
    (handle, runner.run(connect))
}

struct Runner {
    endpoint: Option<String>,
    sink: Arc<dyn Sink>,
    snapshot: watch::Sender<Snapshot>,
    actions: mpsc::Receiver<Pending>,
    retry: Arc<Notify>,
}

/// Una conexión recién abierta, con lo necesario para dibujar la escena.
struct Opened<S> {
    client: IpcClient<S>,
    desk: Config,
    state: State,
    peer: PeerView,
    pairing: PairingView,
    input: InputView,
    caps: Box<CapsView>,
}

/// Por qué terminó una conexión.
enum Ended {
    /// Se soltó el último [`Handle`]: la sesión termina.
    Released,
    Lost(String),
}

impl Runner {
    async fn run<C, F, S>(mut self, mut connect: C)
    where
        C: FnMut() -> F,
        F: Future<Output = Result<IpcClient<S>, IpcError>>,
        S: AsyncRead + AsyncWrite + Unpin,
    {
        loop {
            let reason = match self.open(connect()).await {
                Ok(Opened {
                    client,
                    desk,
                    state,
                    peer,
                    pairing,
                    input,
                    caps,
                }) => {
                    let agent = client.agent();
                    self.publish(Snapshot::Connected {
                        agent,
                        desk,
                        state,
                        peer,
                        pairing,
                        input,
                        caps,
                    });
                    match self.serve(client).await {
                        Ended::Released => return,
                        Ended::Lost(reason) => reason,
                    }
                }
                Err(reason) => reason,
            };
            self.publish(Snapshot::Unavailable {
                endpoint: self.endpoint.clone(),
                reason: reason.clone(),
            });
            if !self.wait_retry(&reason).await {
                return;
            }
        }
    }

    fn publish(&self, snapshot: Snapshot) {
        self.sink.snapshot(&snapshot);
        self.snapshot.send_replace(snapshot);
    }

    /// Conecta y pide lo necesario para dibujar la escena, todo con el mismo límite.
    async fn open<F, S>(&self, connect: F) -> Result<Opened<S>, String>
    where
        F: Future<Output = Result<IpcClient<S>, IpcError>>,
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let opening = async {
            let mut client = connect.await?;
            let desk = match client.request(UiCommand::GetDesk).await? {
                Reply::Desk(d) => d,
                other => return Err(IpcError::UnexpectedReply(Box::new(other))),
            };
            let state = client.get_state().await?;
            let peer = match client.request(UiCommand::GetPeer).await? {
                Reply::Peer(p) => p.into(),
                other => return Err(IpcError::UnexpectedReply(Box::new(other))),
            };
            let pairing = match client.request(UiCommand::GetPairing).await? {
                Reply::Pairing(p) => p.into(),
                other => return Err(IpcError::UnexpectedReply(Box::new(other))),
            };
            let input = match client.request(UiCommand::GetInput).await? {
                Reply::Input(i) => i.into(),
                other => return Err(IpcError::UnexpectedReply(Box::new(other))),
            };
            let caps = match client.request(UiCommand::GetCapabilities).await? {
                Reply::Capabilities(c) => Box::new((*c).into()),
                other => return Err(IpcError::UnexpectedReply(Box::new(other))),
            };
            Ok(Opened {
                client,
                desk,
                state,
                peer,
                input,
                pairing,
                caps,
            })
        };
        match tokio::time::timeout(QUERY_TIMEOUT, opening).await {
            Ok(Ok(opened)) => Ok(opened),
            Ok(Err(e)) => Err(explain(&e)),
            Err(_) => Err(timed_out()),
        }
    }

    async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
        &mut self,
        mut client: IpcClient<S>,
    ) -> Ended {
        loop {
            tokio::select! {
                pending = self.actions.recv() => {
                    let Some(p) = pending else { return Ended::Released };
                    let reply = tokio::time::timeout(QUERY_TIMEOUT, client.request(p.command.clone())).await;
                    let lost = match reply {
                        Ok(Ok(reply)) => {
                            p.answer(reply.into());
                            continue;
                        }
                        Ok(Err(e)) => explain(&e),
                        Err(_) => timed_out(),
                    };
                    p.answer(ActionResult::Unavailable { reason: lost.clone() });
                    return Ended::Lost(lost);
                }
                event = client.next_event() => match event {
                    Ok(Some(event)) => self.on_event(event),
                    Ok(None) => return Ended::Lost(CLOSED.into()),
                    Err(e) => return Ended::Lost(explain(&e)),
                },
            }
        }
    }

    fn on_event(&self, event: AgentToUi) {
        match event {
            AgentToUi::State(state) => self.update_state(state),
            AgentToUi::Txn(event) => {
                // El estado final también llega empujado; tomarlo aquí evita que la
                // escena se quede un instante con el de antes al cerrar el progreso.
                if let TxnEvent::Committed { state, .. } | TxnEvent::Failed { state, .. } = &event {
                    self.update_state(state.clone());
                }
                self.sink.notice(&Notice::Txn {
                    event: event.into(),
                });
            }
            AgentToUi::Power(event) => self.sink.notice(&event.into()),
            AgentToUi::Peer(status) => self.update_peer(status.into()),
            AgentToUi::Pairing(status) => self.update_pairing(status.into()),
            AgentToUi::Input(status) => self.update_input(status.into()),
            AgentToUi::Desk(new) => self.update_desk(new),
            AgentToUi::Machines { local, remote } => self.update_caps(
                DeskCapabilities {
                    here: *local,
                    peer: remote.map(|r| *r),
                }
                .into(),
            ),
            AgentToUi::KeyProbe(KeyProbeStatus { armed, heard }) => {
                self.sink.notice(&Notice::KeyProbe {
                    event: KeyProbeView { armed, heard },
                });
            }
            // Solo llegan respuestas dentro de `request`; una suelta no le sirve a nadie.
            AgentToUi::Reply { .. } => {}
            // El puesto nuevo ya llega con `Desk`; el aviso con «Deshacer» lo muestra la UI
            // nueva (T-112), no esta.
            AgentToUi::DeskChanged { .. } => {}
        }
    }

    fn update_state(&self, new: State) {
        self.update(|c| match c {
            Snapshot::Connected { state, .. } if *state != new => {
                *state = new;
                true
            }
            _ => false,
        });
    }

    fn update_desk(&self, new: Config) {
        self.update(|c| match c {
            Snapshot::Connected { desk, .. } if *desk != new => {
                *desk = new;
                true
            }
            _ => false,
        });
    }

    fn update_peer(&self, new: PeerView) {
        self.update(|c| match c {
            Snapshot::Connected { peer, .. } if *peer != new => {
                *peer = new;
                true
            }
            _ => false,
        });
    }

    fn update_pairing(&self, new: PairingView) {
        self.update(|c| match c {
            Snapshot::Connected { pairing, .. } if *pairing != new => {
                *pairing = new;
                true
            }
            _ => false,
        });
    }

    /// Cambia la escena conectada y la publica si `change` dice que cambió.
    fn update(&self, change: impl FnOnce(&mut Snapshot) -> bool) {
        let mut next = self.snapshot.borrow().clone();
        if change(&mut next) {
            self.publish(next);
        }
    }

    /// Lo que puede cada máquina. El lado de esta sale de aquí también: el agente cambia de
    /// lado al emparejar ([`tandemd::Agent::adopt_side`]) y lo anuncia en sus capacidades;
    /// el `Hello` de la conexión solo dice el de cuando se conectó.
    fn update_caps(&self, new: CapsView) {
        self.update(|c| match c {
            Snapshot::Connected { caps, agent, .. }
                if **caps != new || *agent != new.here.machine =>
            {
                *agent = new.here.machine;
                **caps = new;
                true
            }
            _ => false,
        });
    }

    fn update_input(&self, new: InputView) {
        self.update(|c| match c {
            Snapshot::Connected { input, .. } if *input != new => {
                *input = new;
                true
            }
            _ => false,
        });
    }

    /// Espera hasta el próximo intento; mientras, los pedidos se contestan sin agente.
    /// `false` si ya no hay a quién servir.
    async fn wait_retry(&mut self, reason: &str) -> bool {
        let wait = tokio::time::sleep(RETRY_EVERY);
        tokio::pin!(wait);
        loop {
            tokio::select! {
                () = &mut wait => return true,
                () = self.retry.notified() => return true,
                pending = self.actions.recv() => match pending {
                    Some(p) => p.answer(ActionResult::Unavailable { reason: reason.into() }),
                    None => return false,
                },
            }
        }
    }
}

fn timed_out() -> String {
    format!("tandemd no respondió en {} s", QUERY_TIMEOUT.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tandemd::ipc::mem::{self, MemConnector};
    use tandemd::{Agent, AgentConfig, StateStore, daemon};
    use tokio::io::DuplexStream;

    use super::*;

    const CONFIG: &str = r#"
machine = "a"

# Con qué despertar a la PC: sin esto, el plan ni siquiera intenta encenderla.
[power.wol]
mac = "9c:6b:00:1a:2b:ff"
"#;

    #[derive(Default)]
    struct Recorder {
        snapshots: Mutex<Vec<Snapshot>>,
        notices: Mutex<Vec<Notice>>,
        changed: Notify,
    }

    impl Sink for Recorder {
        fn snapshot(&self, s: &Snapshot) {
            self.snapshots.lock().unwrap().push(s.clone());
            self.changed.notify_waiters();
        }
        fn notice(&self, n: &Notice) {
            self.notices.lock().unwrap().push(n.clone());
            self.changed.notify_waiters();
        }
    }

    impl Recorder {
        /// Espera hasta que lo recibido cumpla `done`.
        async fn until(&self, done: impl Fn(&[Snapshot], &[Notice]) -> bool) {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if done(
                    &self.snapshots.lock().unwrap(),
                    &self.notices.lock().unwrap(),
                ) {
                    return;
                }
                changed.await;
            }
        }

        fn last(&self) -> Snapshot {
            self.snapshots.lock().unwrap().last().cloned().unwrap()
        }
    }

    struct Running {
        agent: Arc<Agent>,
        connector: MemConnector,
        stop: tokio::sync::oneshot::Sender<()>,
        task: tokio::task::JoinHandle<Result<(), tandemd::StoreError>>,
    }

    fn agent_in(dir: &Path) -> Running {
        let cfg = AgentConfig::from_toml(CONFIG).unwrap();
        let agent = Arc::new(Agent::new(cfg, StateStore::new(dir.join("state.json"))));
        let (listener, connector) = mem::pair();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(daemon::run(agent.clone(), listener, async {
            let _ = stopped.await;
        }));
        Running {
            agent,
            connector,
            stop,
            task,
        }
    }

    type Connect = Box<
        dyn FnMut() -> std::pin::Pin<
                Box<dyn Future<Output = Result<IpcClient<DuplexStream>, IpcError>> + Send>,
            > + Send,
    >;

    /// Conecta siempre al agente que esté en `slot` en ese momento (o falla si no hay).
    fn via(slot: Arc<Mutex<Option<MemConnector>>>) -> Connect {
        Box::new(move || {
            let c = slot.lock().unwrap().clone();
            Box::pin(async move {
                let c = c.ok_or_else(|| {
                    IpcError::Io(std::io::Error::from(std::io::ErrorKind::NotFound))
                })?;
                IpcClient::handshake(c.connect()?).await
            })
        })
    }

    fn session(slot: &Arc<Mutex<Option<MemConnector>>>) -> (Handle, Arc<Recorder>) {
        let rec = Arc::new(Recorder::default());
        let (handle, run) = start(
            via(slot.clone()),
            Some("/tmp/tandemd.sock".into()),
            rec.clone(),
        );
        tokio::spawn(run);
        (handle, rec)
    }

    fn connected(s: &[Snapshot]) -> bool {
        matches!(s.last(), Some(Snapshot::Connected { .. }))
    }

    fn is_unavailable(s: &[Snapshot]) -> bool {
        matches!(s.last(), Some(Snapshot::Unavailable { .. }))
    }

    #[tokio::test]
    async fn connects_with_desk_and_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;
        assert_eq!(
            handle.snapshot(),
            Snapshot::Connected {
                agent: Machine::A,
                desk: a.agent.config().desk.clone(),
                state: a.agent.state(),
                peer: PeerView::Unpaired,
                pairing: PairingView::Idle,
                input: InputStatus::new(Machine::A).into(),
                caps: Box::new(a.agent.capabilities().into()),
            }
        );
    }

    /// El acomodo cambiado en el agente llega a la escena, y `GetSetup` dice de dónde
    /// salió la configuración.
    #[tokio::test]
    async fn a_desk_change_updates_the_scene() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let file = dir.path().join("tandem.toml");
        std::fs::write(&file, CONFIG).unwrap();
        a.agent.set_config_file(file.clone());
        a.agent.set_generated(true);
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;

        let mut desk = a.agent.desk();
        desk.arrangement = Some(tandem_core::Arrangement {
            a_side: tandem_core::Side::Right,
            align: tandem_core::Align::Center,
        });
        a.agent.set_desk(desk.clone()).unwrap();
        let moved = |s: &Snapshot| matches!(s, Snapshot::Connected { desk: d, .. } if *d == desk);
        rec.until(|snaps, _| snaps.last().is_some_and(moved)).await;
        assert!(moved(&handle.snapshot()));

        let got = handle.act(Action::GetSetup).await;
        assert_eq!(
            got,
            ActionResult::Setup {
                setup: SetupInfo {
                    config_file: Some(file.display().to_string()),
                    generated: true,
                    port: None,
                }
            }
        );
        let v = serde_json::to_value(&got).unwrap();
        assert_eq!(v["kind"], "setup");
        assert_eq!(v["setup"]["generated"], true);
    }

    /// Revisión del PR #68: si el agente cambia de lado (al emparejar), la UI conectada lo
    /// sigue; si no, la bandeja diría el estado de esta misma máquina como si fuera la otra.
    #[tokio::test]
    async fn the_snapshot_follows_a_change_of_side() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;
        assert!(matches!(
            handle.snapshot(),
            Snapshot::Connected {
                agent: Machine::A,
                ..
            }
        ));

        a.agent.adopt_side(Machine::B);
        rec.until(|snaps, _| {
            matches!(
                snaps.last(),
                Some(Snapshot::Connected {
                    agent: Machine::B,
                    ..
                })
            )
        })
        .await;
        let Snapshot::Connected {
            agent, caps, state, ..
        } = handle.snapshot()
        else {
            panic!("se esperaba conectado");
        };
        assert_eq!(agent, Machine::B);
        assert_eq!(caps.here.machine, Machine::B);
        assert!(
            state.is_on(Machine::B),
            "esta máquina, con su lado nuevo, prendida"
        );
    }

    #[tokio::test]
    async fn pushed_pairing_status_updates_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;

        a.agent.set_pairing_status(PairingStatus::Comparing {
            code: "004217".into(),
            peer: "PC".into(),
            confirmed: false,
        });
        let want = PairingView::Comparing {
            code: "004217".into(),
            peer: "PC".into(),
            confirmed: false,
        };
        rec.until(|snaps, _| {
            matches!(snaps.last(), Some(Snapshot::Connected { pairing, .. }) if *pairing == want)
        })
        .await;
        assert!(
            matches!(handle.snapshot(), Snapshot::Connected { pairing, .. } if pairing == want)
        );
        // Sin la red armada, el agente explica por qué no puede emparejar.
        assert_eq!(
            handle.act(Action::StartPairing).await,
            ActionResult::Rejected {
                reason: tandemd::agent::NO_NETWORK.into()
            }
        );
    }

    #[tokio::test]
    async fn pushed_input_status_updates_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;

        let status = InputStatus {
            missing: vec![InputPermission::Accessibility],
            ..InputStatus::new(Machine::B)
        };
        a.agent.set_input_status(status.clone());
        let want = InputView::from(status);
        rec.until(|snaps, _| {
            matches!(snaps.last(), Some(Snapshot::Connected { input, .. }) if *input == want)
        })
        .await;
        assert!(matches!(handle.snapshot(), Snapshot::Connected { input, .. } if input == want));
    }

    #[tokio::test]
    async fn pushed_peer_status_updates_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;

        a.agent.set_peer_status(PeerStatus::Paired {
            name: "PC".into(),
            link: Some(Link::Ethernet),
        });
        let want = PeerView::Paired {
            name: "PC".into(),
            link: Some(LinkView::Ethernet),
        };
        rec.until(|snaps, _| {
            matches!(snaps.last(), Some(Snapshot::Connected { peer, .. }) if *peer == want)
        })
        .await;
        assert!(matches!(handle.snapshot(), Snapshot::Connected { peer, .. } if peer == want));
    }

    #[tokio::test]
    async fn pushed_state_updates_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;

        let mut s = a.agent.state();
        s.on.b = true;
        a.agent.set_state(s.clone()).unwrap();
        let want = s.clone();
        rec.until(|snaps, _| {
            matches!(snaps.last(), Some(Snapshot::Connected { state, .. }) if *state == want)
        })
        .await;
        assert!(matches!(handle.snapshot(), Snapshot::Connected { state, .. } if state == s));
    }

    #[tokio::test]
    async fn a_power_change_reports_each_step() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;

        let r = handle
            .act(Action::SetPower {
                machine: Machine::B,
                on: true,
            })
            .await;
        let ActionResult::Accepted { txn } = r else {
            panic!("se esperaba Accepted: {r:?}");
        };
        // Sin backends, el agente intenta y falla: la ventana ve cada paso.
        rec.until(|_, n| {
            n.iter().any(
                |n| matches!(n, Notice::Txn { event: TxnView::Failed { txn: t, .. } } if *t == txn),
            )
        })
        .await;
        let notices = rec.notices.lock().unwrap().clone();
        let Notice::Txn {
            event: TxnView::Started { steps, .. },
        } = &notices[0]
        else {
            panic!("se esperaba Started: {notices:?}");
        };
        assert_eq!(
            steps.first(),
            Some(&Step::PowerOn {
                machine: Machine::B
            })
        );
        assert!(notices.iter().any(|n| matches!(
            n,
            Notice::Txn {
                event: TxnView::StepFailed { index: 0, .. }
            }
        )));
    }

    #[tokio::test]
    async fn rejections_are_passed_through() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;
        // Sin red no se puede emparejar: el motivo del agente llega tal cual.
        let r = handle.act(Action::ConfirmPairing).await;
        assert_eq!(
            r,
            ActionResult::Rejected {
                reason: tandemd::agent::NO_NETWORK.into()
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn without_agent_explains_and_recovers_when_it_starts() {
        let dir = tempfile::tempdir().unwrap();
        let slot = Arc::new(Mutex::new(None));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| is_unavailable(s)).await;
        let Snapshot::Unavailable { endpoint, reason } = rec.last() else {
            unreachable!()
        };
        assert_eq!(endpoint.as_deref(), Some("/tmp/tandemd.sock"));
        assert!(reason.contains("no está corriendo"), "{reason}");

        // Los pedidos se contestan sin quedarse colgados.
        let r = handle
            .act(Action::SetPower {
                machine: Machine::B,
                on: true,
            })
            .await;
        assert!(matches!(r, ActionResult::Unavailable { .. }), "{r:?}");

        // Arranca el agente: el siguiente intento lo encuentra.
        let a = agent_in(dir.path());
        *slot.lock().unwrap() = Some(a.connector.clone());
        rec.until(|s, _| connected(s)).await;
    }

    #[tokio::test]
    async fn retry_now_does_not_wait() {
        let dir = tempfile::tempdir().unwrap();
        let slot = Arc::new(Mutex::new(None));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| is_unavailable(s)).await;
        let a = agent_in(dir.path());
        *slot.lock().unwrap() = Some(a.connector.clone());
        // Sin el tiempo pausado: si esperara RETRY_EVERY, la prueba tardaría 2 s.
        let started = std::time::Instant::now();
        handle.retry_now();
        rec.until(|s, _| connected(s)).await;
        assert!(started.elapsed() < RETRY_EVERY, "{:?}", started.elapsed());
    }

    #[tokio::test(start_paused = true)]
    async fn agent_going_away_is_noticed_and_reconnected() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (_handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;

        *slot.lock().unwrap() = None;
        a.stop.send(()).unwrap();
        a.task.await.unwrap().unwrap();
        rec.until(|s, _| is_unavailable(s)).await;

        let b = agent_in(dir.path());
        *slot.lock().unwrap() = Some(b.connector.clone());
        rec.until(|s, _| connected(s)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn hung_agent_times_out() {
        // Acepta la conexión y nunca saluda.
        let held = Arc::new(Mutex::new(Vec::new()));
        let tries = Arc::new(AtomicUsize::new(0));
        let (h, t) = (held.clone(), tries.clone());
        let connect = move || {
            t.fetch_add(1, Ordering::SeqCst);
            let (client, server) = tokio::io::duplex(64);
            h.lock().unwrap().push(server);
            IpcClient::handshake(client)
        };
        let rec = Arc::new(Recorder::default());
        let (_handle, run) = start(connect, None, rec.clone());
        tokio::spawn(run);
        rec.until(|s, _| is_unavailable(s)).await;
        let Snapshot::Unavailable { reason, .. } = rec.last() else {
            unreachable!()
        };
        assert_eq!(reason, "tandemd no respondió en 2 s");
        // Y vuelve a intentar.
        rec.until(|s, _| s.len() >= 2).await;
        assert!(tries.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn dropping_the_handle_ends_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let rec = Arc::new(Recorder::default());
        let (handle, run) = start(via(slot), None, rec.clone());
        let task = tokio::spawn(run);
        rec.until(|s, _| connected(s)).await;
        drop(handle);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_session_that_cannot_start_explains_why() {
        let handle = unavailable(None, "sin HOME".into());
        assert_eq!(
            handle.snapshot(),
            Snapshot::Unavailable {
                endpoint: None,
                reason: "sin HOME".into()
            }
        );
        assert_eq!(
            handle
                .act(Action::SetPower {
                    machine: Machine::B,
                    on: true
                })
                .await,
            ActionResult::Unavailable {
                reason: "sin HOME".into()
            }
        );
        handle.retry_now();
    }

    #[test]
    fn json_is_what_the_frontend_reads() {
        let cfg = AgentConfig::from_toml(
            "machine = \"a\"\n[[screens]]\nmachine = \"b\"\nid = \"34\"\nx = 0\ny = 0\nwidth = 3440\nheight = 1440\nscale = 1.5\n",
        )
        .unwrap();
        let snap = Snapshot::Connected {
            agent: Machine::A,
            desk: cfg.desk,
            state: State::initial(Machine::A),
            peer: PeerView::Unpaired,
            pairing: PairingView::Idle,
            input: InputStatus {
                server: Machine::B,
                hub_here: Some(false),
                session: true,
                locked: false,
                missing: vec![InputPermission::InputMonitoring],
                problem: None,
                defined: true,
            }
            .into(),
            caps: test_caps(Machine::A),
        };
        let v = serde_json::to_value(&snap).unwrap();
        assert_eq!(v["kind"], "connected");
        assert_eq!(v["caps"]["here"]["machine"], "a");
        assert_eq!(
            v["caps"]["here"]["power"],
            serde_json::json!({ "wake_peer": false, "wakeable": true, "power_off": false })
        );
        assert_eq!(v["caps"]["peer"], serde_json::Value::Null);
        assert_eq!(
            v["state"],
            serde_json::json!({ "on": { "a": true, "b": false } })
        );
        assert_eq!(
            v["desk"],
            serde_json::json!({
                "machines": { "a": null, "b": null },
                "screens": [{
                    "machine": "b", "id": "34", "x": 0, "y": 0,
                    "width": 3440, "height": 1440, "scale": 1.5
                }],
                "arrangement": null,
                "scenes": [],
                "shortcuts": { "lock": ["ScrollLock"] },
                "prefs": { "cmd_ctrl": true, "drag_files": true },
                "modules": { "a": {}, "b": {} }
            })
        );
        assert_eq!(
            v["input"],
            serde_json::json!({
                "server": "b",
                "hub_here": false,
                "session": true,
                "locked": false,
                "missing": ["input_monitoring"],
                "problem": null,
                "defined": true,
            })
        );
        assert_eq!(v["peer"], serde_json::json!({ "kind": "unpaired" }));
        assert_eq!(v["pairing"], serde_json::json!({ "kind": "idle" }));
        let pairing = |p: PairingStatus| serde_json::to_value(PairingView::from(p)).unwrap();
        assert_eq!(
            pairing(PairingStatus::Comparing {
                code: "012345".into(),
                peer: "PC".into(),
                confirmed: true,
            }),
            serde_json::json!({ "kind": "comparing", "code": "012345", "peer": "PC", "confirmed": true })
        );
        assert_eq!(
            serde_json::to_value(PeerView::from(PeerStatus::Paired {
                name: "PC".into(),
                link: Some(Link::Usb4),
            }))
            .unwrap(),
            serde_json::json!({ "kind": "paired", "name": "PC", "link": "usb4" })
        );
        let n = Notice::Txn {
            event: TxnView::Started {
                txn: 3,
                steps: vec![Step::PowerOn {
                    machine: Machine::B,
                }],
            },
        };
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            serde_json::json!({
                "kind": "txn",
                "event": { "type": "started", "txn": 3, "steps": [
                    { "step": "power_on", "machine": "b" }
                ]}
            })
        );
        let p = Notice::from(PowerEvent::PeerLost {
            machine: Machine::B,
            reason: "la PC dejó de responder".into(),
        });
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            serde_json::json!({
                "kind": "power",
                "machine": "b",
                "reason": "la PC dejó de responder",
            })
        );
        assert_eq!(
            serde_json::to_value(ActionResult::Rejected { reason: "x".into() }).unwrap(),
            serde_json::json!({ "kind": "rejected", "reason": "x" })
        );
    }

    #[test]
    fn actions_are_read_from_the_frontend_json() {
        let read =
            |v: serde_json::Value| UiCommand::from(serde_json::from_value::<Action>(v).unwrap());
        assert_eq!(
            read(serde_json::json!({ "kind": "set_power", "machine": "b", "on": false })),
            UiCommand::SetPower {
                machine: Machine::B,
                on: false
            }
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "key_probe", "on": true })),
            UiCommand::KeyProbe { on: true }
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "set_keyboard", "machine": "a" })),
            UiCommand::SetKeyboard(Machine::A)
        );
        for (kind, cmd) in [
            ("start_pairing", UiCommand::StartPairing),
            ("confirm_pairing", UiCommand::ConfirmPairing),
            ("cancel_pairing", UiCommand::CancelPairing),
            ("unpair", UiCommand::Unpair),
            ("heard_key", UiCommand::HeardKey),
            ("ping_peer", UiCommand::PingPeer),
            ("request_permissions", UiCommand::RequestPermissions),
            ("get_setup", UiCommand::GetSetup),
        ] {
            assert_eq!(read(serde_json::json!({ "kind": kind })), cmd);
        }
        // Lo de los monitores ya no existe: la ventana no puede pedirlo.
        for gone in ["set_mode", "list_displays", "scan_monitors", "answer_probe"] {
            assert!(serde_json::from_value::<Action>(serde_json::json!({ "kind": gone })).is_err());
        }
    }

    #[test]
    fn every_reply_maps_to_a_result() {
        assert_eq!(
            ActionResult::from(Reply::Accepted { txn: 1 }),
            ActionResult::Accepted { txn: 1 }
        );
        assert_eq!(ActionResult::from(Reply::Done), ActionResult::Done);
        assert_eq!(
            ActionResult::from(Reply::Pairing(PairingStatus::Idle)),
            ActionResult::Done
        );
        assert_eq!(
            ActionResult::from(Reply::Rejected("no".into())),
            ActionResult::Rejected {
                reason: "no".into()
            }
        );
        assert_eq!(
            ActionResult::from(Reply::Pong {
                link: Link::Ethernet,
                millis: 3
            }),
            ActionResult::Pong {
                link: LinkView::Ethernet,
                millis: 3
            }
        );
    }

    #[test]
    fn txn_events_keep_their_data() {
        let s = State {
            on: tandem_core::Powered::both(),
        };
        let cases = [
            (
                TxnEvent::StepStarted { txn: 1, index: 2 },
                TxnView::StepStarted { txn: 1, index: 2 },
            ),
            (
                TxnEvent::StepDone { txn: 1, index: 2 },
                TxnView::StepDone { txn: 1, index: 2 },
            ),
            (
                TxnEvent::StepFailed {
                    txn: 1,
                    index: 0,
                    error: "e".into(),
                },
                TxnView::StepFailed {
                    txn: 1,
                    index: 0,
                    error: "e".into(),
                },
            ),
            (
                TxnEvent::Committed {
                    txn: 1,
                    state: s.clone(),
                },
                TxnView::Committed {
                    txn: 1,
                    state: s.clone(),
                },
            ),
            (
                TxnEvent::Failed {
                    txn: 1,
                    state: s.clone(),
                },
                TxnView::Failed { txn: 1, state: s },
            ),
        ];
        for (event, view) in cases {
            assert_eq!(TxnView::from(event), view);
        }
    }

    #[test]
    fn power_event_reaches_the_window_with_its_reason() {
        let rec = Arc::new(Recorder::default());
        let (snapshot, _) = watch::channel(Snapshot::Connecting);
        let (_, actions) = mpsc::channel(1);
        let runner = Runner {
            endpoint: None,
            sink: rec.clone(),
            snapshot,
            actions,
            retry: Arc::new(Notify::new()),
        };
        runner.on_event(AgentToUi::Power(PowerEvent::PeerLost {
            machine: Machine::B,
            reason: "la PC dejó de responder (¿corte de corriente?)".into(),
        }));
        assert_eq!(
            *rec.notices.lock().unwrap(),
            vec![Notice::Power {
                machine: Machine::B,
                reason: "la PC dejó de responder (¿corte de corriente?)".into(),
            }]
        );
        // Un aviso de energía no toca el estado: ese llega empujado aparte.
        assert!(rec.snapshots.lock().unwrap().is_empty());
    }
}
