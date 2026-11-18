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
use tandem_core::{
    Align, Arrangement, Config, DeskPatch, InputPrefs, Machine, MonitorInput, Scene, Shortcuts,
    Side, State, Step,
};
use tandem_proto::{
    AgentToUi, Capabilities, CursorSample, DeskCapabilities, DragStatus, EnvKind, Environment,
    Hardware, InputPermission, InputStatus, KeyProbeStatus, Link, ModuleId, ModuleInfo,
    ModuleState, MonitorView, NetworkStatus, PairingStatus, PathState, PeerStatus, PowerEvent,
    Reply, Resources, ScreenRect, SetupInfo, TunnelId, TxnEvent, UiCommand, UpdateStatus,
};
use tandemd::ipc::{IpcClient, IpcError};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Notify, mpsc, oneshot, watch};

use crate::status::{CLOSED, QUERY_TIMEOUT, explain};
use crate::tunnels::{self, OUTPUT_CHUNK, OutputItem, ProcessItem, ProjectItem, TransferItem};

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
        /// Los monitores de las dos máquinas con lo último que leyó su dueño (T-110).
        monitors: Vec<MonitorView>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkView {
    Usb4,
    Ethernet,
    Wifi,
}

impl From<Link> for LinkView {
    fn from(l: Link) -> Self {
        match l {
            Link::Usb4 => LinkView::Usb4,
            Link::Ethernet => LinkView::Ethernet,
            Link::Wifi => LinkView::Wifi,
        }
    }
}

impl From<LinkView> for Link {
    fn from(l: LinkView) -> Self {
        match l {
            LinkView::Usb4 => Link::Usb4,
            LinkView::Ethernet => Link::Ethernet,
            LinkView::Wifi => Link::Wifi,
        }
    }
}

/// El papel de un camino en la pantalla Red (T-118).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathStateView {
    Primary,
    Backup,
    Unplugged,
    Down,
}

/// Un segundo del historial de un camino, en microsegundos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SampleView {
    pub rtt_us: Option<u32>,
    pub input_p99_us: Option<u32>,
}

/// Un camino de la pantalla Red: lo que `tandem_proto::PathMetrics` dice, con los nombres
/// que lee la ventana.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PathView {
    pub link: LinkView,
    pub state: PathStateView,
    pub rtt_us: Option<u32>,
    pub input_p50_us: Option<u32>,
    pub input_p99_us: Option<u32>,
    pub input_target_us: u32,
    pub bytes_per_sec: Option<u64>,
    pub history: Vec<SampleView>,
}

/// Un cambio de camino («pasó a Wi-Fi en 0.6 s»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SwitchView {
    pub at_ms: u64,
    pub from: Option<LinkView>,
    pub to: Option<LinkView>,
    pub took_ms: Option<u32>,
}

/// [`NetworkStatus`] en JSON para el frontend (evento `agent-network`, una vez por segundo).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NetworkView {
    pub paths: Vec<PathView>,
    pub switches: Vec<SwitchView>,
    pub measuring: Option<LinkView>,
    /// Lo que el usuario tiene que hacer a mano (por ejemplo, asignar la IP de un cable).
    pub notices: Vec<NoticeView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NoticeView {
    pub interface: String,
    pub message: String,
    pub command: String,
}

impl From<NetworkStatus> for NetworkView {
    fn from(n: NetworkStatus) -> Self {
        NetworkView {
            paths: n
                .paths
                .into_iter()
                .map(|p| PathView {
                    link: p.link.into(),
                    state: match p.state {
                        PathState::Primary => PathStateView::Primary,
                        PathState::Backup => PathStateView::Backup,
                        PathState::Unplugged => PathStateView::Unplugged,
                        PathState::Down => PathStateView::Down,
                    },
                    rtt_us: p.rtt_us,
                    input_p50_us: p.input_p50_us,
                    input_p99_us: p.input_p99_us,
                    input_target_us: p.input_target_us,
                    bytes_per_sec: p.bandwidth.map(|b| b.bytes_per_sec),
                    history: p
                        .history
                        .into_iter()
                        .map(|s| SampleView {
                            rtt_us: s.rtt_us,
                            input_p99_us: s.input_p99_us,
                        })
                        .collect(),
                })
                .collect(),
            switches: n
                .switches
                .into_iter()
                .map(|s| SwitchView {
                    at_ms: s.at_ms,
                    from: s.from.map(Into::into),
                    to: s.to.map(Into::into),
                    took_ms: s.took_ms,
                })
                .collect(),
            measuring: n.measuring.map(Into::into),
            notices: n
                .notices
                .into_iter()
                .map(|x| NoticeView {
                    interface: x.interface,
                    message: x.message,
                    command: x.command,
                })
                .collect(),
        }
    }
}

impl From<PeerStatus> for PeerView {
    fn from(p: PeerStatus) -> Self {
        match p {
            PeerStatus::Unpaired => PeerView::Unpaired,
            PeerStatus::Paired { name, link } => PeerView::Paired {
                name,
                link: link.map(Into::into),
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
    /// Las pantallas que lee el motor aquí y las que anunció la otra máquina (T-150): la
    /// ventana las dibuja si el puesto no declara `[[screens]]`.
    pub screens: Vec<ScreenRect>,
    pub peer_screens: Vec<ScreenRect>,
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
    /// Los módulos de su sistema con su estado (T-098): la página Máquina (T-115).
    pub modules: Vec<ModuleView>,
    /// Entornos de ejecución detectados (T-105).
    pub environments: Vec<EnvView>,
    /// El equipo según el escaneo (T-105).
    pub hardware: HardwareView,
}

/// [`ModuleInfo`] en JSON para el frontend: el id es el del puesto (`clipboard`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleView {
    pub id: &'static str,
    pub name: String,
    pub detail: String,
    pub state: ModuleStateView,
    /// Por qué no está disponible, si no lo está.
    pub reason: Option<String>,
    pub switchable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleStateView {
    Active,
    Off,
    Unavailable,
}

impl From<ModuleInfo> for ModuleView {
    fn from(m: ModuleInfo) -> Self {
        let (state, reason) = match m.state {
            ModuleState::Active => (ModuleStateView::Active, None),
            ModuleState::Off => (ModuleStateView::Off, None),
            ModuleState::Unavailable { reason } => (ModuleStateView::Unavailable, Some(reason)),
        };
        ModuleView {
            id: m.id.as_str(),
            name: m.name,
            detail: m.detail,
            state,
            reason,
            switchable: m.switchable,
        }
    }
}

/// [`Environment`] en JSON para el frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvView {
    pub id: String,
    pub kind: EnvKindView,
    pub name: String,
    pub default: bool,
    pub ready: bool,
    pub detail: String,
    pub gpu: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvKindView {
    Shell,
    Wsl,
    Docker,
}

impl From<Environment> for EnvView {
    fn from(e: Environment) -> Self {
        EnvView {
            id: e.id,
            kind: match e.kind {
                EnvKind::Shell => EnvKindView::Shell,
                EnvKind::Wsl => EnvKindView::Wsl,
                EnvKind::Docker => EnvKindView::Docker,
            },
            name: e.name,
            default: e.default,
            ready: e.ready,
            detail: e.detail,
            gpu: e.gpu,
        }
    }
}

/// [`Hardware`] en JSON para el frontend (bytes como número: caben en un `f64` sin perder).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct HardwareView {
    pub cpu: Option<String>,
    pub cores: u32,
    pub memory: u64,
    pub gpus: Vec<GpuView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GpuView {
    pub name: String,
    /// `None` si comparte la RAM.
    pub vram: Option<u64>,
}

impl From<Hardware> for HardwareView {
    fn from(h: Hardware) -> Self {
        HardwareView {
            cpu: h.cpu,
            cores: h.cores,
            memory: h.memory,
            gpus: h
                .gpus
                .into_iter()
                .map(|g| GpuView {
                    name: g.name,
                    vram: g.vram,
                })
                .collect(),
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PowerCapsView {
    pub wake_peer: bool,
    pub wakeable: bool,
    pub power_off: bool,
    pub mac: Option<String>,
    pub wol_mac: Option<String>,
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
                mac: c.power.mac.clone(),
                wol_mac: c.power.wol_mac.clone(),
            },
            services: c.services,
            modules: c.modules.into_iter().map(Into::into).collect(),
            environments: c.environments.into_iter().map(Into::into).collect(),
            hardware: c.hardware.into(),
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
            screens: s.screens,
            peer_screens: s.peer_screens,
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
    /// Dónde está el cursor, a 30 Hz como mucho (T-128); solo llega con [`Action::WatchCursor`].
    Cursor {
        sample: CursorView,
    },
    /// El cursor cruzó el borde entre las dos máquinas: `edge` es el borde de `from` por el
    /// que salió.
    Crossed {
        from: Machine,
        to: Machine,
        edge: Side,
    },
    /// La otra máquina cambió el puesto y ganó (D5): «Cambiado desde X · Deshacer». Deshacer
    /// es [`Action::UndoDesk`] con `undo_to`.
    DeskChanged {
        by: Machine,
        rev: u64,
        summary: String,
        undo_to: u64,
    },
    /// Avance de una copia de archivos entre las máquinas (`AgentToUi::Transfer`): un aviso con
    /// barra. `finished` cierra la copia.
    Transfer {
        upload: bool,
        path: String,
        done: u64,
        total: u64,
        finished: bool,
    },
    /// El avance de un arrastre de archivos entre las máquinas (T-121).
    Drag {
        status: DragView,
    },
}

/// [`DragStatus`] en JSON para el frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DragView {
    /// Copiando `name`, el archivo `index` (desde 0) de `total`.
    Copying {
        name: String,
        index: u32,
        total: u32,
    },
    /// Todo copiado: queda soltar en la otra máquina.
    Copied {
        total: u32,
    },
    Dropped,
    Cancelled,
    Failed {
        reason: String,
    },
}

impl From<DragStatus> for DragView {
    fn from(s: DragStatus) -> Self {
        match s {
            DragStatus::Copying { name, index, total } => DragView::Copying { name, index, total },
            DragStatus::Copied { total } => DragView::Copied { total },
            DragStatus::Dropped => DragView::Dropped,
            DragStatus::Cancelled => DragView::Cancelled,
            DragStatus::Failed(reason) => DragView::Failed { reason },
        }
    }
}

/// [`CursorSample`] en JSON para el frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CursorView {
    pub machine: Machine,
    pub screen: String,
    pub x: i32,
    pub y: i32,
}

impl From<CursorSample> for CursorView {
    fn from(s: CursorSample) -> Self {
        CursorView {
            machine: s.machine,
            screen: s.screen,
            x: s.x,
            y: s.y,
        }
    }
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
    /// Vuelta atrás: el paso `index`, ya hecho, se deshizo.
    StepUndone {
        txn: u64,
        index: u32,
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
            TxnEvent::StepUndone { txn, index } => TxnView::StepUndone { txn, index },
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
    /// «Bloquear aquí» (T-129): bloquea o suelta el cursor en esta máquina.
    LockHere {
        on: bool,
    },
    /// El nombre del puesto (`EditDesk { Name }`, T-159); vacío vuelve al de omisión.
    SetDeskName {
        name: String,
    },
    /// La MAC con que esta máquina despierta a la otra (T-159); `null` la borra.
    SetWol {
        mac: Option<String>,
    },
    /// De dónde salió la configuración; contesta [`ActionResult::Setup`].
    GetSetup,
    /// Pedirle a la otra máquina que instale `version` (T-122). Lo usa «Actualizar todas»
    /// desde el núcleo de la app, no la ventana.
    UpdatePeer {
        version: String,
    },
    /// «Guardar» del acomodo (T-117, D18): de qué lado de B está A y con qué alineación. Es
    /// un cambio del puesto compartido, así que llega a la otra máquina (T-099).
    SetArrangement {
        a_side: Side,
        align: Align,
    },
    /// Encender o apagar un módulo de `machine` (T-098); `module` es el id del puesto.
    SetModule {
        machine: Machine,
        #[serde(deserialize_with = "module_id")]
        module: ModuleId,
        on: bool,
    },
    /// Volver a escanear los entornos de `machine` (T-105).
    Rescan {
        machine: Machine,
    },
    /// Recibir los recursos en vivo de las dos máquinas ([`Sink::resources`]) mientras la
    /// pantalla visible los muestra. La sesión lo recuerda y lo vuelve a pedir al reconectar.
    WatchResources {
        on: bool,
    },
    /// Nombre o color de una máquina (el puesto compartido, D4/D17); `None` no cambia.
    SetMachine {
        machine: Machine,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        color: Option<String>,
    },
    /// Recibir (o dejar de recibir) el cursor en vivo como [`Notice::Cursor`] y
    /// [`Notice::Crossed`] (T-128). La sesión lo vuelve a pedir si se reconecta.
    WatchCursor {
        on: bool,
    },
    /// Recibir el estado de la red ([`Sink::network`]) una vez por segundo mientras la pantalla
    /// Red está visible. La sesión lo recuerda y lo vuelve a pedir al reconectar.
    WatchNetwork {
        on: bool,
    },
    /// Medir el ancho de banda de un camino; contesta [`ActionResult::Bandwidth`].
    MeasureLink {
        path: LinkView,
    },
    /// Aplicar la escena `id` (D6): una transacción con progreso (T-116).
    ApplyScene {
        id: String,
    },
    /// Cambiar la entrada de un monitor compatible, con el veredicto del firmware (T-110).
    SwitchMonitor {
        monitor: String,
        input: MonitorInput,
    },
    /// Crear una escena (T-116); sin `id` se le da uno a partir del nombre.
    AddScene {
        scene: Scene,
    },
    /// Guardar los cambios de una escena existente.
    EditScene {
        scene: Scene,
    },
    DeleteScene {
        id: String,
    },
    /// Los procesos de un túnel, también los que terminaron hace poco (T-114); contesta
    /// [`ActionResult::Processes`]. Los túneles llegan solos por [`Sink::tunnels`].
    Processes {
        #[serde(deserialize_with = "tunnels::tunnel_id")]
        tunnel: TunnelId,
    },
    /// Los túneles abiertos ahora (T-154): una ventana que se abre o recarga los pide porque
    /// [`Sink::tunnels`] solo avisa al conectar y con cada cambio; contesta
    /// [`ActionResult::Tunnels`].
    Tunnels,
    /// La salida de un proceso desde los cursores `stdout` y `stderr` (T-114); contesta
    /// [`ActionResult::Output`].
    Output {
        #[serde(deserialize_with = "tunnels::tunnel_id")]
        tunnel: TunnelId,
        process: u32,
        #[serde(default)]
        stdout: u64,
        #[serde(default)]
        stderr: u64,
    },
    /// Teclado y mouse compartidos (`EditDesk { Prefs }`): se manda el par entero.
    SetPrefs {
        cmd_ctrl: bool,
        drag_files: bool,
    },
    /// El atajo de bloqueo del puesto (`EditDesk { Shortcuts }`); vacío = sin atajo.
    SetLockShortcut {
        lock: Vec<String>,
    },
    /// «Deshacer» del aviso de cambio (D5): vuelve el puesto a la versión `to_rev`.
    UndoDesk {
        to_rev: u64,
    },
}

/// Suma o resta una ventana a `n`; devuelve si había alguna y si hay alguna.
fn count(n: &mut u32, on: bool) -> (bool, bool) {
    let before = *n > 0;
    *n = if on {
        n.saturating_add(1)
    } else {
        n.saturating_sub(1)
    };
    (before, *n > 0)
}

/// El id del módulo como lo guarda el puesto (`clipboard`, `secure_desktop`).
fn module_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ModuleId, D::Error> {
    let s = String::deserialize(d)?;
    ModuleId::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("no hay módulo «{s}»")))
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
            Action::LockHere { on } => UiCommand::LockHere { on },
            Action::SetDeskName { name } => UiCommand::EditDesk {
                patch: DeskPatch::Name(name),
            },
            Action::SetWol { mac } => UiCommand::SetWol { mac },
            Action::GetSetup => UiCommand::GetSetup,
            Action::UpdatePeer { version } => UiCommand::UpdatePeer { version },
            Action::SetArrangement { a_side, align } => UiCommand::EditDesk {
                patch: DeskPatch::Arrangement(Some(Arrangement { a_side, align })),
            },
            Action::SetModule {
                machine,
                module,
                on,
            } => UiCommand::SetModule {
                machine,
                module,
                on,
            },
            Action::Rescan { machine } => UiCommand::Rescan { machine },
            Action::WatchResources { on } => UiCommand::WatchResources { on },
            Action::SetMachine {
                machine,
                name,
                color,
            } => UiCommand::EditDesk {
                patch: DeskPatch::Machine {
                    side: machine,
                    name,
                    color,
                },
            },
            Action::WatchCursor { on } => UiCommand::WatchCursor { on },
            Action::WatchNetwork { on } => UiCommand::WatchNetwork { on },
            Action::MeasureLink { path } => UiCommand::MeasureLink { path: path.into() },
            Action::ApplyScene { id } => UiCommand::ApplyScene { id },
            Action::SwitchMonitor { monitor, input } => UiCommand::SwitchMonitor { monitor, input },
            Action::AddScene { scene } => UiCommand::EditDesk {
                patch: DeskPatch::AddScene(scene),
            },
            Action::EditScene { scene } => UiCommand::EditDesk {
                patch: DeskPatch::EditScene(scene),
            },
            Action::DeleteScene { id } => UiCommand::EditDesk {
                patch: DeskPatch::DeleteScene { id },
            },
            Action::Processes { tunnel } => UiCommand::GetProcesses { tunnel },
            Action::Tunnels => UiCommand::GetTunnels,
            Action::Output {
                tunnel,
                process,
                stdout,
                stderr,
            } => UiCommand::Output {
                tunnel,
                process: tunnels::process_id(process),
                from: tunnels::cursor(stdout, stderr),
                max: OUTPUT_CHUNK,
            },
            Action::SetPrefs {
                cmd_ctrl,
                drag_files,
            } => UiCommand::EditDesk {
                patch: DeskPatch::Prefs(InputPrefs {
                    cmd_ctrl,
                    drag_files,
                }),
            },
            Action::SetLockShortcut { lock } => UiCommand::EditDesk {
                patch: DeskPatch::Shortcuts(Shortcuts { lock }),
            },
            Action::UndoDesk { to_rev } => UiCommand::UndoDesk { to_rev },
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
    /// Lo que midió la prueba de ancho de banda de un camino.
    Bandwidth {
        link: LinkView,
        bytes_per_sec: u64,
    },
    /// Los procesos de un túnel.
    Processes {
        processes: Vec<ProcessItem>,
    },
    /// La salida de un proceso desde donde se pidió.
    Output {
        output: Box<OutputItem>,
    },
    /// Los túneles abiertos, agrupados por proyecto (T-154).
    Tunnels {
        tunnels: Vec<ProjectItem>,
    },
}

impl From<Reply> for ActionResult {
    fn from(r: Reply) -> Self {
        match r {
            Reply::Accepted { txn } => ActionResult::Accepted { txn },
            Reply::Rejected(reason) => ActionResult::Rejected { reason },
            Reply::Setup(setup) => ActionResult::Setup { setup },
            Reply::Pong { link, millis } => ActionResult::Pong {
                link: link.into(),
                millis,
            },
            Reply::Bandwidth {
                link,
                bytes_per_sec,
            } => ActionResult::Bandwidth {
                link: link.into(),
                bytes_per_sec,
            },
            Reply::Processes(p) => ActionResult::Processes {
                processes: p.into_iter().map(Into::into).collect(),
            },
            Reply::Output(o) => ActionResult::Output {
                output: Box::new((*o).into()),
            },
            Reply::Tunnels(groups) => ActionResult::Tunnels {
                tunnels: tunnels::projects_of(groups),
            },
            Reply::Done
            | Reply::State(_)
            | Reply::Desk(_)
            | Reply::Peer(_)
            | Reply::Pairing(_)
            | Reply::Input(_)
            | Reply::Capabilities(_)
            | Reply::DeskHistory(_)
            | Reply::Monitors(_)
            | Reply::Update(_)
            | Reply::Tunnel { .. }
            | Reply::Files(_)
            | Reply::Forward(_)
            | Reply::Process(_)
            | Reply::BroughtBack(_) => ActionResult::Done,
        }
    }
}

/// A quién se le cuenta lo que pasa (en la app, la ventana por eventos de Tauri).
pub trait Sink: Send + Sync + 'static {
    fn snapshot(&self, snapshot: &Snapshot);
    fn notice(&self, notice: &Notice);
    /// Las versiones de las dos máquinas (T-122); las usa el updater, no la escena.
    fn update(&self, status: &UpdateStatus) {
        let _ = status;
    }

    /// Una muestra de los recursos de `machine` (solo mientras se pidió `WatchResources`).
    /// No va en la [`Snapshot`]: llega cada segundo y la ventana arma su propia historia.
    fn resources(&self, machine: Machine, resources: &Resources) {
        let _ = (machine, resources);
    }

    /// El estado de la red (solo mientras se pidió `WatchNetwork`). No va en la
    /// [`Snapshot`]: llega cada segundo y solo la pantalla Red lo usa.
    fn network(&self, network: &NetworkView) {
        let _ = network;
    }

    /// Los túneles abiertos de las dos máquinas (pantalla Túneles, T-114): llegan al conectar
    /// y con cada cambio, sin pedirlos.
    fn tunnels(&self, tunnels: &[ProjectItem]) {
        let _ = tunnels;
    }

    /// El avance de una copia de archivos de un túnel (T-106), para el tráfico del túnel.
    fn transfer(&self, transfer: &TransferItem) {
        let _ = transfer;
    }
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
        watchers: 0,
        cursor_watchers: 0,
        watch_network: false,
    };
    (handle, runner.run(connect))
}

struct Runner {
    endpoint: Option<String>,
    sink: Arc<dyn Sink>,
    snapshot: watch::Sender<Snapshot>,
    actions: mpsc::Receiver<Pending>,
    retry: Arc<Notify>,
    /// Cuántas ventanas piden los recursos en vivo (la principal y la bandeja comparten esta
    /// sesión): mientras haya alguna, cada conexión nueva los vuelve a pedir.
    watchers: u32,
    /// Cuántas ventanas piden el cursor en vivo (T-128), contadas igual: mientras haya alguna,
    /// cada conexión nueva lo vuelve a pedir.
    cursor_watchers: u32,
    /// La pantalla Red pidió la red en vivo: cada conexión nueva la vuelve a pedir. Es un
    /// booleano y no una cuenta: solo la ventana principal muestra Red, y recargarla no debe
    /// dejar el pedido encendido para siempre.
    watch_network: bool,
}

/// Cuánto se espera la respuesta de `MeasureLink`: la prueba misma más lo normal.
const MEASURE_TIMEOUT: Duration =
    Duration::from_secs(tandem_proto::net::MEASURE_SECS).saturating_add(QUERY_TIMEOUT);

/// Una conexión recién abierta, con lo necesario para dibujar la escena.
struct Opened<S> {
    client: IpcClient<S>,
    desk: Config,
    state: State,
    peer: PeerView,
    pairing: PairingView,
    input: InputView,
    caps: Box<CapsView>,
    monitors: Vec<MonitorView>,
    update: UpdateStatus,
    tunnels: Vec<tandem_proto::ProjectTunnels>,
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
                    monitors,
                    update,
                    tunnels,
                }) => {
                    let agent = client.agent();
                    self.sink.update(&update);
                    self.sink.tunnels(&tunnels::projects_of(tunnels));
                    self.publish(Snapshot::Connected {
                        agent,
                        desk,
                        state,
                        peer,
                        pairing,
                        input,
                        caps,
                        monitors,
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

    /// Suma o resta una ventana que mira los recursos; devuelve si había y si hay alguna.
    fn count_watcher(&mut self, on: bool) -> (bool, bool) {
        count(&mut self.watchers, on)
    }

    /// Lo mismo para las ventanas que miran el cursor en vivo.
    fn count_cursor_watcher(&mut self, on: bool) -> (bool, bool) {
        count(&mut self.cursor_watchers, on)
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
        let watching = self.watchers > 0;
        let watching_cursor = self.cursor_watchers > 0;
        let watching_network = self.watch_network;
        let opening = async {
            let mut client = connect.await?;
            if watching {
                client
                    .request(UiCommand::WatchResources { on: true })
                    .await?;
            }
            if watching_cursor {
                client.request(UiCommand::WatchCursor { on: true }).await?;
            }
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
            let monitors = match client.request(UiCommand::GetMonitors).await? {
                Reply::Monitors(m) => m,
                other => return Err(IpcError::UnexpectedReply(Box::new(other))),
            };
            let update = match client.request(UiCommand::GetUpdate).await? {
                Reply::Update(u) => u,
                other => return Err(IpcError::UnexpectedReply(Box::new(other))),
            };
            let tunnels = match client.request(UiCommand::GetTunnels).await? {
                Reply::Tunnels(t) => t,
                other => return Err(IpcError::UnexpectedReply(Box::new(other))),
            };
            if watching_network {
                client.request(UiCommand::WatchNetwork { on: true }).await?;
            }
            Ok(Opened {
                client,
                desk,
                state,
                peer,
                input,
                pairing,
                caps,
                monitors,
                update,
                tunnels,
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
                    let Some(mut p) = pending else { return Ended::Released };
                    if let UiCommand::WatchResources { on } = p.command {
                        // Solo el primero que pide y el último que suelta llegan al agente.
                        let (before, now) = self.count_watcher(on);
                        if before == now {
                            p.answer(ActionResult::Done);
                            continue;
                        }
                        p.command = UiCommand::WatchResources { on: now };
                    }
                    if let UiCommand::WatchCursor { on } = p.command {
                        // Igual: la conexión con el agente cuenta una vez por todas las ventanas.
                        let (before, now) = self.count_cursor_watcher(on);
                        if before == now {
                            p.answer(ActionResult::Done);
                            continue;
                        }
                        p.command = UiCommand::WatchCursor { on: now };
                    }
                    if let UiCommand::WatchNetwork { on } = p.command {
                        self.watch_network = on;
                    }
                    // La medición dura `MEASURE_SECS`: con el límite de siempre se daría por
                    // perdida la conexión a medio medir.
                    let limit = if matches!(p.command, UiCommand::MeasureLink { .. }) { MEASURE_TIMEOUT } else { QUERY_TIMEOUT };
                    let reply = tokio::time::timeout(limit, client.request(p.command.clone())).await;
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
            AgentToUi::Monitors(new) => self.update_monitors(new),
            AgentToUi::Update(status) => self.sink.update(&status),
            // Solo llegan respuestas dentro de `request`; una suelta no le sirve a nadie.
            AgentToUi::Reply { .. } => {}
            // El puesto nuevo ya llega con `Desk`; esto es solo el aviso con «Deshacer».
            AgentToUi::DeskChanged {
                by,
                rev,
                summary,
                undo_to,
            } => self.sink.notice(&Notice::DeskChanged {
                by,
                rev,
                summary,
                undo_to,
            }),
            AgentToUi::Resources { machine, resources } => {
                self.sink.resources(machine, &resources);
            }
            // La pantalla Túneles (solo lectura, D9) y el tráfico de cada túnel (T-114).
            AgentToUi::Tunnels(groups) => self.sink.tunnels(&tunnels::projects_of(groups)),
            // Las copias las pide un cliente (T-106): el avance va a Túneles y, como aviso, a la ventana.
            AgentToUi::Transfer(t) => {
                self.sink.notice(&Notice::Transfer {
                    upload: t.upload,
                    path: t.path.clone(),
                    done: t.done,
                    total: t.total,
                    finished: t.finished,
                });
                self.sink.transfer(&t.into());
            }
            // Solo con `Action::WatchCursor`: las pantallas Puesto y Acomodo los pintan.
            AgentToUi::Cursor(sample) => self.sink.notice(&Notice::Cursor {
                sample: sample.into(),
            }),
            AgentToUi::Crossed { from, to, edge } => {
                self.sink.notice(&Notice::Crossed { from, to, edge });
            }
            AgentToUi::Drag(status) => self.sink.notice(&Notice::Drag {
                status: status.into(),
            }),
            // Llega solo si se pidió `WatchNetwork` (la pantalla Red, T-118).
            AgentToUi::Network(status) => self.sink.network(&(*status).into()),
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

    fn update_monitors(&self, new: Vec<MonitorView>) {
        self.update(|c| match c {
            Snapshot::Connected { monitors, .. } if *monitors != new => {
                *monitors = new;
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
                    Some(p) => {
                        // Sin agente no hay recursos ni red, pero el pedido vale para la próxima conexión.
                        if let UiCommand::WatchResources { on } = p.command {
                            self.count_watcher(on);
                        }
                        if let UiCommand::WatchNetwork { on } = p.command {
                            self.watch_network = on;
                        }
                        p.answer(ActionResult::Unavailable { reason: reason.into() });
                    }
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
mac = "aa:bb:cc:dd:ee:ff"
"#;

    #[derive(Default)]
    struct Recorder {
        snapshots: Mutex<Vec<Snapshot>>,
        notices: Mutex<Vec<Notice>>,
        resources: Mutex<Vec<(Machine, Resources)>>,
        networks: Mutex<Vec<NetworkView>>,
        tunnels: Mutex<Vec<Vec<ProjectItem>>>,
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
        fn resources(&self, machine: Machine, r: &Resources) {
            self.resources.lock().unwrap().push((machine, r.clone()));
            self.changed.notify_waiters();
        }
        fn network(&self, n: &NetworkView) {
            self.networks.lock().unwrap().push(n.clone());
            self.changed.notify_waiters();
        }
        fn tunnels(&self, t: &[ProjectItem]) {
            self.tunnels.lock().unwrap().push(t.to_vec());
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

        /// Espera una muestra de recursos igual a `want` de `machine` (falla a los 10 s).
        async fn until_resources(&self, machine: Machine, want: &Resources) {
            tokio::time::timeout(Duration::from_secs(10), self.wait_resources(machine, want))
                .await
                .expect("no llegaron los recursos");
        }

        async fn wait_resources(&self, machine: Machine, want: &Resources) {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if self
                    .resources
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(m, r)| *m == machine && r == want)
                {
                    return;
                }
                changed.await;
            }
        }

        fn last(&self) -> Snapshot {
            self.snapshots.lock().unwrap().last().cloned().unwrap()
        }

        /// Espera a que lleguen al menos `n` estados de la red (falla a los 10 s).
        async fn until_networks(&self, n: usize) {
            let wait = async {
                loop {
                    let changed = self.changed.notified();
                    tokio::pin!(changed);
                    changed.as_mut().enable();
                    if self.networks.lock().unwrap().len() >= n {
                        return;
                    }
                    changed.await;
                }
            };
            tokio::time::timeout(Duration::from_secs(10), wait)
                .await
                .expect("no llegó el estado de la red");
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
                monitors: vec![],
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

    /// El cursor en vivo llega como avisos solo si la ventana lo pidió, y se vuelve a pedir
    /// al reconectar.
    #[tokio::test]
    async fn the_live_cursor_is_forwarded_only_when_asked_and_asked_again_on_reconnect() {
        let sample = |x| CursorSample {
            machine: Machine::B,
            screen: "b-1".into(),
            x,
            y: 7,
        };
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;

        // Sin pedirlo, la conexión no recibe nada y el motor ni se entera.
        assert!(!a.agent.cursor_gate().is_on());
        a.agent
            .publish_cursor(tandemd::CursorEvent::Sample(sample(1)));
        assert_eq!(
            handle.act(Action::WatchCursor { on: true }).await,
            ActionResult::Done
        );
        assert!(a.agent.cursor_gate().is_on());
        a.agent
            .publish_cursor(tandemd::CursorEvent::Sample(sample(2)));
        a.agent.publish_cursor(tandemd::CursorEvent::Crossed {
            from: Machine::A,
            to: Machine::B,
            edge: Side::Right,
        });
        rec.until(|_, n| n.iter().any(|n| matches!(n, Notice::Crossed { .. })))
            .await;
        let notices = rec.notices.lock().unwrap().clone();
        assert_eq!(
            notices,
            vec![
                Notice::Cursor {
                    sample: sample(2).into()
                },
                Notice::Crossed {
                    from: Machine::A,
                    to: Machine::B,
                    edge: Side::Right
                },
            ]
        );
        assert_eq!(
            serde_json::to_value(&notices[1]).unwrap(),
            serde_json::json!({ "kind": "crossed", "from": "a", "to": "b", "edge": "right" })
        );

        // Otro agente en el mismo lugar: la sesión se reconecta y lo vuelve a pedir.
        *slot.lock().unwrap() = None;
        a.stop.send(()).unwrap();
        a.task.await.unwrap().unwrap();
        rec.until(|s, _| is_unavailable(s)).await;
        let b = agent_in(dir.path());
        *slot.lock().unwrap() = Some(b.connector.clone());
        rec.until(|s, _| connected(s)).await;
        within_secs(async {
            while !b.agent.cursor_gate().is_on() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
    }

    /// El avance de un arrastre de archivos del agente llega a la ventana como aviso (T-121).
    #[tokio::test]
    async fn the_file_drag_progress_is_forwarded_as_notices() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (_handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;

        a.agent.drag().publish(DragStatus::Copying {
            name: "a.txt".into(),
            index: 1,
            total: 3,
        });
        a.agent
            .drag()
            .publish(DragStatus::Failed("sin espacio".into()));
        rec.until(|_, n| n.iter().any(|n| matches!(n, Notice::Drag { .. })))
            .await;
        rec.until(|_, n| n.len() >= 2).await;
        let notices = rec.notices.lock().unwrap().clone();
        assert_eq!(
            serde_json::to_value(&notices[0]).unwrap(),
            serde_json::json!({
                "kind": "drag",
                "status": { "kind": "copying", "name": "a.txt", "index": 1, "total": 3 }
            })
        );
        assert_eq!(
            serde_json::to_value(&notices[1]).unwrap(),
            serde_json::json!({
                "kind": "drag",
                "status": { "kind": "failed", "reason": "sin espacio" }
            })
        );
    }

    async fn within_secs<F: Future>(f: F) -> F::Output {
        tokio::time::timeout(Duration::from_secs(10), f)
            .await
            .expect("la prueba superó el tiempo máximo")
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
                latency: None,
                screens: vec![],
                peer_screens: vec![],
            }
            .into(),
            caps: test_caps(Machine::A),
            monitors: vec![],
        };
        let v = serde_json::to_value(&snap).unwrap();
        assert_eq!(v["monitors"], serde_json::json!([]));
        assert_eq!(v["kind"], "connected");
        assert_eq!(v["caps"]["here"]["machine"], "a");
        assert_eq!(
            v["caps"]["here"]["power"],
            serde_json::json!({ "wake_peer": false, "wakeable": true, "power_off": false, "mac": null, "wol_mac": null })
        );
        assert_eq!(v["caps"]["peer"], serde_json::Value::Null);
        assert_eq!(
            v["state"],
            serde_json::json!({ "on": { "a": true, "b": false }, "scene": null })
        );
        assert_eq!(
            v["desk"],
            serde_json::json!({
                "desk_name": null,
                "machines": { "a": null, "b": null },
                "screens": [{
                    "machine": "b", "id": "34", "x": 0, "y": 0,
                    "width": 3440, "height": 1440, "scale": 1.5
                }],
                "arrangement": null,
                "scenes": [],
                "shortcuts": { "lock": ["ScrollLock"] },
                "prefs": { "cmd_ctrl": true, "drag_files": true },
                "modules": { "a": {}, "b": {} },
                "data": { "wifi_cap_percent": 50, "wifi_cap_mbps": null },
                "cables": []
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
                "screens": [],
                "peer_screens": [],
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

    /// T-115: la página Máquina pide los recursos; llegan a la ventana y, si el agente se
    /// reinicia, la sesión los vuelve a pedir sola.
    #[tokio::test]
    async fn watched_resources_reach_the_window_and_survive_a_reconnection() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;
        assert_eq!(
            handle.act(Action::WatchResources { on: true }).await,
            ActionResult::Done
        );
        let mut theirs = Resources::default();
        theirs.cpu.load = 420;
        a.agent.receive_peer_resources(theirs.clone());
        rec.until_resources(Machine::B, &theirs).await;

        *slot.lock().unwrap() = None;
        a.stop.send(()).unwrap();
        a.task.await.unwrap().unwrap();
        rec.until(|s, _| is_unavailable(s)).await;
        let b = agent_in(dir.path());
        *slot.lock().unwrap() = Some(b.connector.clone());
        handle.retry_now();
        rec.until(|s, _| connected(s)).await;
        theirs.cpu.load = 610;
        b.agent.receive_peer_resources(theirs.clone());
        rec.until_resources(Machine::B, &theirs).await;
    }

    /// La ventana principal y la bandeja comparten la sesión: si una deja de mirar los
    /// recursos, la otra los sigue recibiendo.
    #[tokio::test]
    async fn resources_keep_flowing_while_any_window_watches() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;
        for on in [true, true, false] {
            assert_eq!(
                handle.act(Action::WatchResources { on }).await,
                ActionResult::Done
            );
        }
        let mut theirs = Resources::default();
        theirs.cpu.load = 333;
        a.agent.receive_peer_resources(theirs.clone());
        rec.until_resources(Machine::B, &theirs).await;
    }

    /// T-115: apagar un módulo desde la página Máquina llega al agente y vuelve en las
    /// capacidades con su estado nuevo.
    #[tokio::test]
    async fn a_module_switched_off_comes_back_in_the_capabilities() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;
        let clipboard = |s: &Snapshot| match s {
            Snapshot::Connected { caps, .. } => caps
                .here
                .modules
                .iter()
                .find(|m| m.id == "clipboard")
                .map(|m| m.state),
            _ => None,
        };
        assert_eq!(clipboard(&handle.snapshot()), Some(ModuleStateView::Active));
        assert_eq!(
            handle
                .act(Action::SetModule {
                    machine: Machine::A,
                    module: ModuleId::Clipboard,
                    on: false,
                })
                .await,
            ActionResult::Done
        );
        rec.until(|s, _| s.last().and_then(clipboard) == Some(ModuleStateView::Off))
            .await;
        // Rescan de esta máquina: se acepta y lo encontrado llega con las capacidades.
        assert_eq!(
            handle
                .act(Action::Rescan {
                    machine: Machine::A
                })
                .await,
            ActionResult::Done
        );
    }

    #[test]
    fn machine_page_views_keep_what_the_window_draws() {
        let m = ModuleView::from(ModuleInfo {
            id: ModuleId::SecureDesktop,
            name: "Pantallas protegidas".into(),
            detail: "UAC".into(),
            state: ModuleState::Unavailable {
                reason: "Falta el servicio".into(),
            },
            switchable: false,
        });
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            serde_json::json!({
                "id": "secure_desktop", "name": "Pantallas protegidas", "detail": "UAC",
                "state": "unavailable", "reason": "Falta el servicio", "switchable": false
            })
        );
        let e = EnvView::from(Environment {
            id: "wsl:Ubuntu-24.04".into(),
            kind: EnvKind::Wsl,
            name: "Ubuntu 24.04".into(),
            version: None,
            default: true,
            ready: true,
            detail: "uv · cargo".into(),
            tools: Vec::new(),
            gpu: true,
        });
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({
                "id": "wsl:Ubuntu-24.04", "kind": "wsl", "name": "Ubuntu 24.04",
                "default": true, "ready": true, "detail": "uv · cargo", "gpu": true
            })
        );
        let h = HardwareView::from(Hardware {
            cpu: Some("Apple M5".into()),
            cores: 12,
            memory: 24 << 30,
            gpus: vec![tandem_proto::Gpu {
                name: "Apple M5".into(),
                vram: None,
                driver: None,
            }],
        });
        assert_eq!(
            serde_json::to_value(&h).unwrap(),
            serde_json::json!({
                "cpu": "Apple M5", "cores": 12, "memory": 24u64 << 30,
                "gpus": [{ "name": "Apple M5", "vram": null }]
            })
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
        assert_eq!(
            read(serde_json::json!({ "kind": "lock_here", "on": true })),
            UiCommand::LockHere { on: true }
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "set_wol", "mac": "aa:bb:cc:dd:ee:ff" })),
            UiCommand::SetWol {
                mac: Some("aa:bb:cc:dd:ee:ff".into())
            }
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "set_desk_name", "name": "Oficina" })),
            UiCommand::EditDesk {
                patch: DeskPatch::Name("Oficina".into())
            }
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "watch_cursor", "on": true })),
            UiCommand::WatchCursor { on: true }
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
        assert_eq!(
            read(serde_json::json!({ "kind": "set_arrangement", "a_side": "up", "align": "end" })),
            UiCommand::EditDesk {
                patch: DeskPatch::Arrangement(Some(Arrangement {
                    a_side: Side::Up,
                    align: Align::End
                }))
            }
        );
        assert_eq!(
            read(
                serde_json::json!({ "kind": "set_module", "machine": "b", "module": "secure_desktop", "on": false })
            ),
            UiCommand::SetModule {
                machine: Machine::B,
                module: ModuleId::SecureDesktop,
                on: false
            }
        );
        assert!(
            serde_json::from_value::<Action>(serde_json::json!({ "kind": "set_module", "machine": "b", "module": "nada", "on": false }))
                .is_err()
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "rescan", "machine": "a" })),
            UiCommand::Rescan {
                machine: Machine::A
            }
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "watch_resources", "on": true })),
            UiCommand::WatchResources { on: true }
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "set_machine", "machine": "b", "color": "#5fbfb0" })),
            UiCommand::EditDesk {
                patch: DeskPatch::Machine {
                    side: Machine::B,
                    name: None,
                    color: Some("#5fbfb0".into())
                }
            }
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "apply_scene", "id": "juego" })),
            UiCommand::ApplyScene { id: "juego".into() }
        );
        assert_eq!(
            read(
                serde_json::json!({ "kind": "switch_monitor", "monitor": "m-1", "input": "hdmi" })
            ),
            UiCommand::SwitchMonitor {
                monitor: "m-1".into(),
                input: MonitorInput::Hdmi
            }
        );
        let scene =
            serde_json::json!({ "id": "juego", "name": "Juego", "monitors": [["m-1", "b"]] });
        assert!(matches!(
            read(serde_json::json!({ "kind": "add_scene", "scene": scene })),
            UiCommand::EditDesk { patch: DeskPatch::AddScene(s) } if s.name == "Juego"
        ));
        assert_eq!(
            read(serde_json::json!({ "kind": "delete_scene", "id": "juego" })),
            UiCommand::EditDesk {
                patch: DeskPatch::DeleteScene { id: "juego".into() }
            }
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "set_prefs", "cmd_ctrl": false, "drag_files": true })),
            UiCommand::EditDesk {
                patch: DeskPatch::Prefs(InputPrefs {
                    cmd_ctrl: false,
                    drag_files: true
                })
            }
        );
        assert_eq!(
            read(
                serde_json::json!({ "kind": "set_lock_shortcut", "lock": ["ControlLeft", "KeyL"] })
            ),
            UiCommand::EditDesk {
                patch: DeskPatch::Shortcuts(Shortcuts {
                    lock: vec!["ControlLeft".into(), "KeyL".into()]
                })
            }
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "undo_desk", "to_rev": 4 })),
            UiCommand::UndoDesk { to_rev: 4 }
        );
        assert_eq!(
            serde_json::to_value(Notice::DeskChanged {
                by: Machine::B,
                rev: 5,
                summary: "Acomodo".into(),
                undo_to: 4
            })
            .unwrap(),
            serde_json::json!({ "kind": "desk_changed", "by": "b", "rev": 5, "summary": "Acomodo", "undo_to": 4 })
        );
        assert_eq!(
            serde_json::to_value(Notice::Transfer {
                upload: true,
                path: "/a/b".into(),
                done: 5,
                total: 10,
                finished: false
            })
            .unwrap(),
            serde_json::json!({ "kind": "transfer", "upload": true, "path": "/a/b", "done": 5, "total": 10, "finished": false })
        );
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
            scene: None,
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
            watchers: 0,
            cursor_watchers: 0,
            watch_network: false,
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
    /// T-118: la pantalla Red pide la red en vivo y llega a la ventana con sus tres caminos;
    /// si el agente se reinicia, la sesión la vuelve a pedir sola.
    #[tokio::test]
    async fn the_network_reaches_the_window_and_survives_a_reconnection() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;
        assert!(
            rec.networks.lock().unwrap().is_empty(),
            "sin pedirla no llega"
        );
        assert_eq!(
            handle.act(Action::WatchNetwork { on: true }).await,
            ActionResult::Done
        );
        rec.until_networks(1).await;
        let first = rec.networks.lock().unwrap()[0].clone();
        let links: Vec<_> = first.paths.iter().map(|p| p.link).collect();
        assert_eq!(links, [LinkView::Usb4, LinkView::Ethernet, LinkView::Wifi]);
        assert!(
            first
                .paths
                .iter()
                .all(|p| p.state == PathStateView::Unplugged)
        );

        *slot.lock().unwrap() = None;
        a.stop.send(()).unwrap();
        a.task.await.unwrap().unwrap();
        rec.until(|s, _| is_unavailable(s)).await;
        let seen = rec.networks.lock().unwrap().len();
        let b = agent_in(dir.path());
        *slot.lock().unwrap() = Some(b.connector.clone());
        handle.retry_now();
        rec.until(|s, _| connected(s)).await;
        rec.until_networks(seen + 1).await;
    }

    /// T-114: al conectar la ventana recibe los túneles sin pedirlos, y los procesos de un
    /// túnel que no existe se rechazan sin tirar la sesión.
    #[tokio::test]
    async fn the_tunnels_reach_the_window_on_connect() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;
        assert_eq!(rec.tunnels.lock().unwrap().first(), Some(&vec![]));
        let got = handle
            .act(Action::Processes {
                tunnel: TunnelId(7),
            })
            .await;
        assert!(
            !matches!(got, ActionResult::Unavailable { .. }),
            "la sesión sigue viva: {got:?}"
        );
        rec.until(|s, _| connected(s)).await;
    }

    /// T-154: una ventana nueva pide los túneles existentes en vez de esperar un cambio.
    #[tokio::test]
    async fn a_new_window_can_ask_for_the_tunnels() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;
        match handle.act(Action::Tunnels).await {
            ActionResult::Tunnels { tunnels } => assert!(tunnels.is_empty()),
            other => panic!("se esperaban los túneles: {other:?}"),
        }
    }

    /// Dejar de mirar la red corta el flujo; y un pedido hecho sin agente vale para la
    /// próxima conexión.
    #[tokio::test]
    async fn the_network_stops_when_the_window_stops_watching() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(None));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| is_unavailable(s)).await;
        assert!(matches!(
            handle.act(Action::WatchNetwork { on: true }).await,
            ActionResult::Unavailable { .. }
        ));
        *slot.lock().unwrap() = Some(a.connector.clone());
        handle.retry_now();
        rec.until(|s, _| connected(s)).await;
        rec.until_networks(1).await;
        assert_eq!(
            handle.act(Action::WatchNetwork { on: false }).await,
            ActionResult::Done
        );
        let n = rec.networks.lock().unwrap().len();
        tokio::time::sleep(Duration::from_millis(2300)).await;
        assert_eq!(rec.networks.lock().unwrap().len(), n);
    }

    /// Medir un camino sin conexión por él se rechaza con la razón del agente, sin tirar la
    /// sesión.
    #[tokio::test]
    async fn measuring_a_path_without_connection_is_rejected_and_the_session_survives() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;
        let got = handle
            .act(Action::MeasureLink {
                path: LinkView::Wifi,
            })
            .await;
        assert!(matches!(got, ActionResult::Rejected { .. }), "{got:?}");
        assert!(connected(&rec.snapshots.lock().unwrap()));
    }

    #[test]
    fn network_views_use_the_names_the_window_reads() {
        let n = NetworkView::from(NetworkStatus {
            notices: vec![],
            paths: vec![tandem_proto::PathMetrics {
                link: Link::Usb4,
                state: PathState::Primary,
                rtt_us: Some(300),
                input_p50_us: Some(400),
                input_p99_us: Some(1900),
                input_target_us: 4000,
                bandwidth: Some(tandem_proto::Bandwidth {
                    bytes_per_sec: 1_175_000_000,
                    at_ms: 5,
                }),
                history: vec![tandem_proto::LatencySample {
                    rtt_us: Some(300),
                    input_p99_us: None,
                }],
            }],
            switches: vec![tandem_proto::PathSwitch {
                at_ms: 9,
                from: Some(Link::Usb4),
                to: Some(Link::Wifi),
                took_ms: Some(600),
            }],
            measuring: Some(Link::Wifi),
        });
        let v = serde_json::to_value(&n).unwrap();
        assert_eq!(v["paths"][0]["link"], "usb4");
        assert_eq!(v["paths"][0]["state"], "primary");
        assert_eq!(v["paths"][0]["bytes_per_sec"], 1_175_000_000u64);
        assert_eq!(v["paths"][0]["history"][0]["rtt_us"], 300);
        assert_eq!(v["switches"][0]["to"], "wifi");
        assert_eq!(v["switches"][0]["took_ms"], 600);
        assert_eq!(v["measuring"], "wifi");
        let action: Action =
            serde_json::from_str(r#"{"kind":"measure_link","path":"ethernet"}"#).unwrap();
        assert_eq!(
            UiCommand::from(action),
            UiCommand::MeasureLink {
                path: Link::Ethernet
            }
        );
        let got = ActionResult::from(Reply::Bandwidth {
            link: Link::Usb4,
            bytes_per_sec: 7,
        });
        let v = serde_json::to_value(&got).unwrap();
        assert_eq!(
            (v["kind"].as_str(), v["link"].as_str()),
            (Some("bandwidth"), Some("usb4"))
        );
        assert_eq!(v["bytes_per_sec"], 7);
    }
}
