//! Conexión viva con `tandemd` para la escena principal.
//!
//! La ventana no pregunta: la sesión mantiene una conexión abierta, pide el puesto y el
//! estado al conectar y después reenvía lo que el agente empuja (estado, avance de las
//! transacciones, pedidos de cambio manual). Si el agente no está o se cae, la ventana
//! lo ve como [`Snapshot::Unavailable`] y la sesión reintenta sola.
//!
//! Es genérica sobre el transporte y sobre a quién se le avisa ([`Sink`]): las pruebas
//! usan la IPC en memoria con un agente real; la app, el socket o pipe y los eventos de
//! Tauri.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tandem_core::{BuiltinDisplay, Config, Input, Machine, Mode, MonitorId, State, Step};
use tandem_proto::{
    AgentToUi, CalibrationOutcome, CalibrationStep, CalibrationUpdate, Capabilities,
    DeskCapabilities, DisplayInfo, DisplayProfile, InputPermission, InputStatus, Link,
    ManualSwitchEvent, PairingStatus, PeerStatus, PowerEvent, ProbeAnswer, ProbeAsk, ProbeEvent,
    ProbePlace, Reply, SavedProfile, SetupInfo, TxnEvent, UiCommand,
};
use tandemd::ipc::{IpcClient, IpcError};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Notify, mpsc, oneshot, watch};

use crate::status::{CLOSED, QUERY_TIMEOUT, explain};

/// Cada cuánto se vuelve a intentar si no hay agente. Corto: la ventana tiene que
/// recuperarse sola en cuanto `tandemd` arranca.
pub const RETRY_EVERY: Duration = Duration::from_secs(2);

/// Todo lo que la escena necesita para dibujarse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Snapshot {
    Connecting,
    Connected {
        /// Máquina en la que corre el agente (y esta ventana).
        agent: Machine,
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
    pub name: String,
    pub os: String,
    pub monitors: Vec<MonitorCapsView>,
    pub builtin: bool,
    pub hub: Option<bool>,
    pub input: InputCapsView,
    pub power: PowerCapsView,
    pub services: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MonitorCapsView {
    pub monitor: Option<MonitorId>,
    pub pnp_id: Option<String>,
    pub name: String,
    pub switch: bool,
    pub ddc: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InputCapsView {
    pub capture: bool,
    pub inject: bool,
    pub missing: Vec<PermissionView>,
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
            name: c.name,
            os: c.os,
            monitors: c
                .monitors
                .into_iter()
                .map(|m| MonitorCapsView {
                    monitor: m.monitor,
                    pnp_id: m.pnp_id,
                    name: m.name,
                    switch: m.switch,
                    ddc: m.ddc,
                })
                .collect(),
            builtin: c.builtin,
            hub: c.hub,
            input: InputCapsView {
                capture: c.input.capture,
                inject: c.input.inject,
                missing: c.input.missing.into_iter().map(permission_view).collect(),
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
    let mut cfg = tandemd::AgentConfig::from_toml(
        "machine = \"mac\"\n[[monitors]]\nid = \"m34\"\nlabel = '34\"'\nwiring = { pc = \"displayport\", mac = \"hdmi\" }",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    cfg.machine = machine;
    let here = tandemd::capabilities::local(&cfg, &cfg.desk, None, &InputStatus::new(machine));
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
    ManualSwitch {
        event: ManualView,
    },
    /// Una sonda de calibración que mostrar o quitar ([`crate::probe`]).
    Probe {
        event: ProbeView,
    },
    /// El avance de «Agregar monitor».
    Calibration {
        event: CalibrationView,
    },
    /// La otra máquina se dio por apagada sin que nadie la apagara (corte de corriente o
    /// suspendida): `reason` se muestra tal cual y `stranded` son los monitores que
    /// quedan en negro.
    Power {
        machine: Machine,
        reason: String,
        stranded: Vec<MonitorId>,
    },
}

impl From<PowerEvent> for Notice {
    fn from(e: PowerEvent) -> Self {
        match e {
            PowerEvent::PeerLost {
                machine,
                reason,
                stranded,
            } => Notice::Power {
                machine,
                reason,
                stranded,
            },
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
    RollingBack {
        txn: u64,
    },
    Committed {
        txn: u64,
        state: State,
    },
    RolledBack {
        txn: u64,
        state: State,
    },
    Inconsistent {
        txn: u64,
        undo_failures: Vec<UndoView>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UndoView {
    pub step: Step,
    pub error: String,
}

impl From<TxnEvent> for TxnView {
    fn from(e: TxnEvent) -> Self {
        match e {
            TxnEvent::Started { txn, steps } => TxnView::Started { txn, steps },
            TxnEvent::StepStarted { txn, index } => TxnView::StepStarted { txn, index },
            TxnEvent::StepDone { txn, index } => TxnView::StepDone { txn, index },
            TxnEvent::StepFailed { txn, index, error } => TxnView::StepFailed { txn, index, error },
            TxnEvent::RollingBack { txn } => TxnView::RollingBack { txn },
            TxnEvent::Committed { txn, state } => TxnView::Committed { txn, state },
            TxnEvent::RolledBack { txn, state } => TxnView::RolledBack { txn, state },
            TxnEvent::Inconsistent { txn, undo_failures } => TxnView::Inconsistent {
                txn,
                undo_failures: undo_failures
                    .into_iter()
                    .map(|u| UndoView {
                        step: u.step,
                        error: u.error,
                    })
                    .collect(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ManualView {
    Requested {
        id: u64,
        monitor: MonitorId,
        label: String,
        to: Machine,
        input: Input,
    },
    Closed {
        id: u64,
    },
}

impl From<ManualSwitchEvent> for ManualView {
    fn from(e: ManualSwitchEvent) -> Self {
        match e {
            ManualSwitchEvent::Requested {
                id,
                monitor,
                label,
                to,
                input,
            } => ManualView::Requested {
                id,
                monitor,
                label,
                to,
                input,
            },
            ManualSwitchEvent::Closed { id } => ManualView::Closed { id },
        }
    }
}

/// [`ProbeEvent`] en JSON para el frontend y para la ventana de la sonda.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProbeView {
    Show {
        id: u64,
        /// El nombre del monitor (EDID o descripción del sistema).
        monitor: String,
        /// La entrada que debería verse.
        input: Input,
        /// La máquina cableada a esa entrada.
        machine: Machine,
        ask: AskView,
        timeout_ms: u64,
        place: PlaceView,
    },
    Hide {
        id: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AskView {
    Candidate { code: u8 },
    UserSwitch,
    Back { code: u8 },
    Verify { code: u8 },
    Stalled { code: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlaceView {
    /// A pantalla completa en el monitor; `screen` es dónde lo ve el agente, si lo sabe.
    Monitor { screen: Option<ScreenView> },
    /// El agente buscó el monitor y no está en el escritorio: la sonda no se muestra.
    Offscreen,
    /// La pregunta «¿qué ves?» de opción múltiple en la ventana principal.
    Question,
}

/// La respuesta a una sonda, como la manda el frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnswerView {
    /// Clic en la sonda a pantalla completa.
    Seen,
    /// «Veo la Mac» / «Veo la PC».
    Machine {
        machine: Machine,
    },
    Black,
    Stop,
}

impl From<AnswerView> for ProbeAnswer {
    fn from(a: AnswerView) -> Self {
        match a {
            AnswerView::Seen => ProbeAnswer::Seen,
            AnswerView::Machine { machine } => ProbeAnswer::Machine(machine),
            AnswerView::Black => ProbeAnswer::Black,
            AnswerView::Stop => ProbeAnswer::Stop,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ScreenView {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl From<ProbeEvent> for ProbeView {
    fn from(e: ProbeEvent) -> Self {
        match e {
            ProbeEvent::Show { id, probe, place } => ProbeView::Show {
                id,
                monitor: probe.monitor.name,
                input: probe.input,
                machine: probe.machine,
                ask: match probe.ask {
                    ProbeAsk::Candidate { code } => AskView::Candidate { code },
                    ProbeAsk::UserSwitch => AskView::UserSwitch,
                    ProbeAsk::Back { code } => AskView::Back { code },
                    ProbeAsk::Verify { code } => AskView::Verify { code },
                    ProbeAsk::Stalled { code } => AskView::Stalled { code },
                },
                timeout_ms: probe.timeout_ms,
                place: match place {
                    ProbePlace::Monitor { screen } => PlaceView::Monitor {
                        screen: screen.map(|s| ScreenView {
                            x: s.x,
                            y: s.y,
                            width: s.width,
                            height: s.height,
                        }),
                    },
                    ProbePlace::Offscreen => PlaceView::Offscreen,
                    ProbePlace::Question => PlaceView::Question,
                },
            },
            ProbeEvent::Hide { id } => ProbeView::Hide { id },
        }
    }
}

/// Un monitor del bus DDC ([`DisplayInfo`]) en JSON para «Agregar monitor».
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DisplayView {
    pub index: u32,
    pub name: String,
    pub pnp_id: Option<String>,
    pub monitor: Option<MonitorId>,
    pub profile: DisplayProfileView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DisplayProfileView {
    None,
    Configured {
        hdmi: u8,
        display_port: u8,
    },
    Known {
        name: String,
        user: bool,
        hdmi: u8,
        display_port: u8,
        toml: String,
    },
}

impl From<DisplayInfo> for DisplayView {
    fn from(d: DisplayInfo) -> Self {
        DisplayView {
            index: d.index,
            name: d.name,
            pnp_id: d.pnp_id,
            monitor: d.monitor,
            profile: match d.profile {
                DisplayProfile::None => DisplayProfileView::None,
                DisplayProfile::Configured { hdmi, display_port } => {
                    DisplayProfileView::Configured { hdmi, display_port }
                }
                DisplayProfile::Known {
                    name,
                    user,
                    hdmi,
                    display_port,
                    toml,
                } => DisplayProfileView::Known {
                    name,
                    user,
                    hdmi,
                    display_port,
                    toml,
                },
            },
        }
    }
}

/// [`CalibrationUpdate`] en JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CalibrationView {
    Started {
        display: u32,
        name: String,
        monitor: Option<MonitorId>,
        start: Input,
    },
    Progress {
        step: StepView,
    },
    Finished {
        outcome: OutcomeView,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepView {
    Reading,
    UserSwitch {
        input: Input,
    },
    Testing {
        input: Input,
    },
    Trying {
        input: Input,
        code: u8,
        attempt: u32,
        of: u32,
    },
    Seen {
        input: Input,
        code: u8,
        seen: bool,
    },
    Returned {
        code: u8,
    },
    Found {
        input: Input,
        code: u8,
    },
    Verifying,
    Verified {
        input: Input,
        code: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutcomeView {
    Saved {
        name: String,
        hdmi: u8,
        display_port: u8,
        attempts: u32,
        monitor: Option<MonitorId>,
        file: Option<String>,
        toml: Option<String>,
        notes: Vec<String>,
    },
    Failed {
        reason: String,
    },
    Cancelled,
}

impl From<CalibrationStep> for StepView {
    fn from(s: CalibrationStep) -> Self {
        match s {
            CalibrationStep::Reading => StepView::Reading,
            CalibrationStep::UserSwitch { input } => StepView::UserSwitch { input },
            CalibrationStep::Testing { input } => StepView::Testing { input },
            CalibrationStep::Trying {
                input,
                code,
                attempt,
                of,
            } => StepView::Trying {
                input,
                code,
                attempt,
                of,
            },
            CalibrationStep::Seen { input, code, seen } => StepView::Seen { input, code, seen },
            CalibrationStep::Returned { code } => StepView::Returned { code },
            CalibrationStep::Found { input, code } => StepView::Found { input, code },
            CalibrationStep::Verifying => StepView::Verifying,
            CalibrationStep::Verified { input, code } => StepView::Verified { input, code },
        }
    }
}

impl From<CalibrationUpdate> for CalibrationView {
    fn from(u: CalibrationUpdate) -> Self {
        match u {
            CalibrationUpdate::Started {
                display,
                name,
                monitor,
                start,
            } => CalibrationView::Started {
                display,
                name,
                monitor,
                start,
            },
            CalibrationUpdate::Progress(step) => CalibrationView::Progress { step: step.into() },
            CalibrationUpdate::Finished(outcome) => CalibrationView::Finished {
                outcome: match outcome {
                    CalibrationOutcome::Saved(SavedProfile {
                        name,
                        hdmi,
                        display_port,
                        attempts,
                        monitor,
                        file,
                        toml,
                        notes,
                    }) => OutcomeView::Saved {
                        name,
                        hdmi,
                        display_port,
                        attempts,
                        monitor,
                        file,
                        toml,
                        notes,
                    },
                    CalibrationOutcome::Failed { reason } => OutcomeView::Failed { reason },
                    CalibrationOutcome::Cancelled => OutcomeView::Cancelled,
                },
            },
        }
    }
}

/// Lo que la ventana puede pedir.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    SetMode {
        mode: Mode,
    },
    MoveMonitor {
        monitor: MonitorId,
        to: Machine,
    },
    SetBuiltin {
        display: BuiltinDisplay,
    },
    /// Encender o apagar `machine` (la otra, en la escena).
    SetPower {
        machine: Machine,
        on: bool,
    },
    AnswerManualSwitch {
        id: u64,
        done: bool,
    },
    /// Clic en la sonda, o lo que se eligió en la pregunta «¿qué ves?».
    AnswerProbe {
        id: u64,
        answer: AnswerView,
    },
    StartPairing,
    ConfirmPairing,
    CancelPairing,
    Unpair,
    /// «Agregar monitor»: los monitores del bus DDC; contesta [`ActionResult::Displays`].
    ListDisplays,
    StartCalibration {
        display: u32,
        #[serde(default)]
        monitor: Option<MonitorId>,
    },
    CancelCalibration,
    UseProfile {
        display: u32,
        #[serde(default)]
        monitor: Option<MonitorId>,
    },
    /// Cambiar el cableado de un monitor (T-082); el puesto nuevo llega en la escena.
    SetWiring {
        monitor: MonitorId,
        wiring: BTreeMap<Machine, Input>,
    },
    /// De dónde salió la configuración; contesta [`ActionResult::Setup`].
    GetSetup,
}

impl From<Action> for UiCommand {
    fn from(a: Action) -> Self {
        match a {
            Action::SetMode { mode } => UiCommand::SetMode(mode),
            Action::MoveMonitor { monitor, to } => UiCommand::MoveMonitor { monitor, to },
            Action::SetBuiltin { display } => UiCommand::SetBuiltin(display),
            Action::SetPower { machine, on } => UiCommand::SetPower { machine, on },
            Action::AnswerManualSwitch { id, done } => UiCommand::AnswerManualSwitch { id, done },
            Action::AnswerProbe { id, answer } => UiCommand::AnswerProbe {
                id,
                answer: answer.into(),
            },
            Action::StartPairing => UiCommand::StartPairing,
            Action::ConfirmPairing => UiCommand::ConfirmPairing,
            Action::CancelPairing => UiCommand::CancelPairing,
            Action::Unpair => UiCommand::Unpair,
            Action::ListDisplays => UiCommand::ListDisplays,
            Action::StartCalibration { display, monitor } => {
                UiCommand::StartCalibration { display, monitor }
            }
            Action::CancelCalibration => UiCommand::CancelCalibration,
            Action::UseProfile { display, monitor } => UiCommand::UseProfile { display, monitor },
            Action::SetWiring { monitor, wiring } => UiCommand::SetWiring { monitor, wiring },
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
    /// Los monitores del bus DDC, para «Agregar monitor».
    Displays {
        displays: Vec<DisplayView>,
    },
    /// La configuración del agente: archivo, si la generó él y su puerto.
    Setup {
        setup: SetupInfo,
    },
}

impl From<Reply> for ActionResult {
    fn from(r: Reply) -> Self {
        match r {
            Reply::Accepted { txn } => ActionResult::Accepted { txn },
            Reply::Rejected(reason) => ActionResult::Rejected { reason },
            Reply::Displays(list) => ActionResult::Displays {
                displays: list.into_iter().map(Into::into).collect(),
            },
            Reply::Setup(setup) => ActionResult::Setup { setup },
            Reply::Done
            | Reply::State(_)
            | Reply::Desk(_)
            | Reply::Peer(_)
            | Reply::Pairing(_)
            | Reply::Input(_)
            | Reply::Capabilities(_) => ActionResult::Done,
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
                other => return Err(IpcError::UnexpectedReply(other)),
            };
            let state = client.get_state().await?;
            let peer = match client.request(UiCommand::GetPeer).await? {
                Reply::Peer(p) => p.into(),
                other => return Err(IpcError::UnexpectedReply(other)),
            };
            let pairing = match client.request(UiCommand::GetPairing).await? {
                Reply::Pairing(p) => p.into(),
                other => return Err(IpcError::UnexpectedReply(other)),
            };
            let input = match client.request(UiCommand::GetInput).await? {
                Reply::Input(i) => i.into(),
                other => return Err(IpcError::UnexpectedReply(other)),
            };
            let caps = match client.request(UiCommand::GetCapabilities).await? {
                Reply::Capabilities(c) => Box::new((*c).into()),
                other => return Err(IpcError::UnexpectedReply(other)),
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
                if let TxnEvent::Committed { state, .. } | TxnEvent::RolledBack { state, .. } =
                    &event
                {
                    self.update_state(state.clone());
                }
                self.sink.notice(&Notice::Txn {
                    event: event.into(),
                });
            }
            AgentToUi::ManualSwitch(event) => self.sink.notice(&Notice::ManualSwitch {
                event: event.into(),
            }),
            AgentToUi::Probe(event) => self.sink.notice(&Notice::Probe {
                event: event.into(),
            }),
            AgentToUi::Calibration(event) => self.sink.notice(&Notice::Calibration {
                event: event.into(),
            }),
            AgentToUi::Power(event) => self.sink.notice(&event.into()),
            AgentToUi::Peer(status) => self.update_peer(status.into()),
            AgentToUi::Pairing(status) => self.update_pairing(status.into()),
            AgentToUi::Input(status) => self.update_input(status.into()),
            AgentToUi::Desk(new) => self.update_desk(new),
            AgentToUi::Capabilities(new) => self.update_caps((*new).into()),
            // Solo llegan respuestas dentro de `request`; una suelta no le sirve a nadie.
            AgentToUi::Reply { .. } => {}
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

    fn update_caps(&self, new: CapsView) {
        self.update(|c| match c {
            Snapshot::Connected { caps, .. } if **caps != new => {
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
machine = "mac"

[[monitors]]
id = "m34"
label = '34"'
wiring = { pc = "displayport", mac = "hdmi" }

[[monitors]]
id = "m32"
label = '32"'
wiring = { pc = "displayport", mac = "hdmi" }
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
                agent: Machine::Mac,
                desk: a.agent.config().desk.clone(),
                state: a.agent.state(),
                peer: PeerView::Unpaired,
                pairing: PairingView::Idle,
                input: InputStatus::new(Machine::Mac).into(),
                caps: Box::new(a.agent.capabilities().into()),
            }
        );
    }

    /// T-082: un clic en la UI cambia el cableado; la escena recibe el puesto nuevo y
    /// `GetSetup` dice de dónde salió la configuración.
    #[tokio::test]
    async fn rewiring_a_monitor_updates_the_scene() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let file = dir.path().join("tandem.toml");
        std::fs::write(&file, CONFIG).unwrap();
        a.agent.set_config_file(file.clone());
        a.agent.set_generated(true);
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;

        let swapped: BTreeMap<Machine, Input> = [
            (Machine::Pc, Input::Hdmi),
            (Machine::Mac, Input::DisplayPort),
        ]
        .into();
        let action: Action = serde_json::from_value(serde_json::json!({
            "kind": "set_wiring",
            "monitor": "m32",
            "wiring": { "pc": "hdmi", "mac": "displayport" },
        }))
        .unwrap();
        assert_eq!(handle.act(action).await, ActionResult::Done);
        let rewired = |s: &Snapshot| {
            matches!(s, Snapshot::Connected { desk, .. }
                if desk.monitor(&"m32".into()).is_some_and(|m| m.wiring == swapped))
        };
        rec.until(|snaps, _| snaps.last().is_some_and(rewired))
            .await;
        assert!(rewired(&handle.snapshot()));

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
            ..InputStatus::new(Machine::Pc)
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
        s.on.pc = true;
        s.builtin_pref = BuiltinDisplay::Mirror;
        a.agent.set_state(s.clone()).unwrap();
        let want = s.clone();
        rec.until(|snaps, _| {
            matches!(snaps.last(), Some(Snapshot::Connected { state, .. }) if *state == want)
        })
        .await;
        assert!(matches!(handle.snapshot(), Snapshot::Connected { state, .. } if state == s));
    }

    #[tokio::test]
    async fn mode_change_reports_each_step() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;

        let r = handle.act(Action::SetMode { mode: Mode::SoloPc }).await;
        let ActionResult::Accepted { txn } = r else {
            panic!("se esperaba Accepted: {r:?}");
        };
        // Sin backends, el agente intenta y deshace: la ventana ve cada paso.
        rec.until(|_, n| {
            n.iter().any(|n| {
                matches!(n, Notice::Txn { event: TxnView::RolledBack { txn: t, .. } } if *t == txn)
            })
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
                machine: Machine::Pc
            })
        );
        assert!(notices.iter().any(|n| matches!(
            n,
            Notice::Txn {
                event: TxnView::StepFailed { index: 0, .. }
            }
        )));
    }

    /// T-078: la sonda llega como aviso, el clic de la ventana vuelve al agente y la
    /// sonda se cierra.
    #[tokio::test]
    async fn probes_reach_the_window_and_the_click_goes_back() {
        let dir = tempfile::tempdir().unwrap();
        let running = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(running.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;
        let probe = tandem_proto::CalibrationProbe {
            monitor: tandem_proto::ProbeMonitor {
                edid: None,
                name: "LC34G55T".into(),
            },
            input: Input::Hdmi,
            machine: Machine::Mac,
            ask: tandem_proto::ProbeAsk::Candidate { code: 6 },
            timeout_ms: 60_000,
        };
        let agent = running.agent.clone();
        let shown = tokio::spawn(async move { agent.probes().show(&probe, false).await });
        rec.until(|_, n| {
            n.iter().any(|n| {
                matches!(
                    n,
                    Notice::Probe {
                        event: ProbeView::Show { .. }
                    }
                )
            })
        })
        .await;
        let id = rec
            .notices
            .lock()
            .unwrap()
            .iter()
            .find_map(|n| match n {
                Notice::Probe {
                    event: ProbeView::Show { id, .. },
                } => Some(*id),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            handle
                .act(Action::AnswerProbe {
                    id,
                    answer: AnswerView::Seen
                })
                .await,
            ActionResult::Done
        );
        assert_eq!(
            shown.await.unwrap(),
            tandem_proto::ProbeOutcome::Answered(tandem_proto::ProbeAnswer::Seen)
        );
        rec.until(|_, n| {
            n.iter().any(|n| {
                *n == Notice::Probe {
                    event: ProbeView::Hide { id },
                }
            })
        })
        .await;
        drop(running.stop);
    }

    #[tokio::test]
    async fn rejections_are_passed_through() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;
        let r = handle
            .act(Action::MoveMonitor {
                monitor: "no-existe".into(),
                to: Machine::Pc,
            })
            .await;
        assert!(
            matches!(&r, ActionResult::Rejected { reason } if reason.contains("no-existe")),
            "{r:?}"
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
                machine: Machine::Pc,
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
                    machine: Machine::Pc,
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
        let cfg = AgentConfig::from_toml(CONFIG).unwrap();
        let state = State::initial(&cfg.desk, Machine::Mac);
        let snap = Snapshot::Connected {
            agent: Machine::Mac,
            desk: cfg.desk,
            state,
            peer: PeerView::Unpaired,
            pairing: PairingView::Idle,
            input: InputStatus {
                server: Machine::Pc,
                hub_here: Some(false),
                session: true,
                locked: false,
                missing: vec![InputPermission::InputMonitoring],
                problem: None,
            }
            .into(),
            caps: test_caps(Machine::Mac),
        };
        let v = serde_json::to_value(&snap).unwrap();
        assert_eq!(v["caps"]["here"]["machine"], "mac");
        assert_eq!(v["caps"]["here"]["builtin"], true);
        assert_eq!(v["caps"]["here"]["monitors"][0]["switch"], false);
        assert_eq!(
            v["caps"]["here"]["power"],
            serde_json::json!({ "wake_peer": false, "wakeable": true, "power_off": false })
        );
        assert_eq!(v["caps"]["peer"], serde_json::Value::Null);
        assert_eq!(
            v["state"]["on"],
            serde_json::json!({ "mac": true, "pc": false })
        );
        assert_eq!(
            v["input"],
            serde_json::json!({
                "server": "pc",
                "hub_here": false,
                "session": true,
                "locked": false,
                "missing": ["input_monitoring"],
                "problem": null,
            })
        );
        assert_eq!(v["kind"], "connected");
        assert_eq!(v["desk"]["monitors"][0]["label"], "34\"");
        assert_eq!(v["desk"]["monitors"][0]["wiring"]["pc"], "displayport");
        assert_eq!(v["state"]["owner"]["m32"], "mac");
        assert_eq!(v["peer"], serde_json::json!({ "kind": "unpaired" }));
        assert_eq!(v["pairing"], serde_json::json!({ "kind": "idle" }));
        let pairing = |p: PairingStatus| serde_json::to_value(PairingView::from(p)).unwrap();
        assert_eq!(
            pairing(PairingStatus::Searching),
            serde_json::json!({ "kind": "searching" })
        );
        assert_eq!(
            pairing(PairingStatus::Comparing {
                code: "012345".into(),
                peer: "PC".into(),
                confirmed: true,
            }),
            serde_json::json!({ "kind": "comparing", "code": "012345", "peer": "PC", "confirmed": true })
        );
        assert_eq!(
            pairing(PairingStatus::Done { peer: "PC".into() }),
            serde_json::json!({ "kind": "done", "peer": "PC" })
        );
        assert_eq!(
            pairing(PairingStatus::Failed { reason: "x".into() }),
            serde_json::json!({ "kind": "failed", "reason": "x" })
        );
        assert_eq!(
            serde_json::to_value(PeerView::from(PeerStatus::Paired {
                name: "PC".into(),
                link: Some(Link::Usb4),
            }))
            .unwrap(),
            serde_json::json!({ "kind": "paired", "name": "PC", "link": "usb4" })
        );
        assert_eq!(
            serde_json::to_value(PeerView::from(PeerStatus::Paired {
                name: "PC".into(),
                link: None,
            }))
            .unwrap(),
            serde_json::json!({ "kind": "paired", "name": "PC", "link": null })
        );

        let n = Notice::Txn {
            event: TxnView::Started {
                txn: 3,
                steps: vec![
                    Step::PowerOn {
                        machine: Machine::Pc,
                    },
                    Step::SwitchInput {
                        monitor: "m34".into(),
                        from: Machine::Mac,
                        to: Machine::Pc,
                        input: Input::DisplayPort,
                        by: Machine::Pc,
                    },
                ],
            },
        };
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            serde_json::json!({
                "kind": "txn",
                "event": { "type": "started", "txn": 3, "steps": [
                    { "step": "power_on", "machine": "pc" },
                    { "step": "switch_input", "monitor": "m34", "from": "mac", "to": "pc", "input": "displayport", "by": "pc" }
                ]}
            })
        );
        let m = Notice::ManualSwitch {
            event: ManualView::Closed { id: 2 },
        };
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            serde_json::json!({ "kind": "manual_switch", "event": { "type": "closed", "id": 2 } })
        );
        let probe = |place| tandem_proto::ProbeEvent::Show {
            id: 7,
            probe: tandem_proto::CalibrationProbe {
                monitor: tandem_proto::ProbeMonitor {
                    edid: None,
                    name: "Odyssey G5".into(),
                },
                input: Input::Hdmi,
                machine: Machine::Mac,
                ask: tandem_proto::ProbeAsk::Candidate { code: 5 },
                timeout_ms: 8000,
            },
            place,
        };
        let shown = Notice::Probe {
            event: probe(ProbePlace::Monitor {
                screen: Some(tandem_proto::ProbeScreen {
                    x: -2560,
                    y: 0,
                    width: 2560,
                    height: 1440,
                }),
            })
            .into(),
        };
        assert_eq!(
            serde_json::to_value(&shown).unwrap(),
            serde_json::json!({
                "kind": "probe",
                "event": {
                    "type": "show", "id": 7, "monitor": "Odyssey G5", "input": "hdmi",
                    "machine": "mac",
                    "ask": { "kind": "candidate", "code": 5 }, "timeout_ms": 8000,
                    "place": { "kind": "monitor", "screen": { "x": -2560, "y": 0, "width": 2560, "height": 1440 } }
                }
            })
        );
        let asked = serde_json::to_value(ProbeView::from(probe(ProbePlace::Question))).unwrap();
        assert_eq!(asked["place"], serde_json::json!({ "kind": "question" }));
        let unplaced = ProbeView::from(probe(ProbePlace::Monitor { screen: None }));
        assert_eq!(
            serde_json::to_value(unplaced).unwrap()["place"],
            serde_json::json!({ "kind": "monitor", "screen": null })
        );
        assert_eq!(
            serde_json::to_value(ProbeView::from(probe(ProbePlace::Offscreen))).unwrap()["place"],
            serde_json::json!({ "kind": "offscreen" })
        );
        let mut switch = probe(ProbePlace::Question);
        if let tandem_proto::ProbeEvent::Show { probe, .. } = &mut switch {
            probe.ask = tandem_proto::ProbeAsk::UserSwitch;
        }
        assert_eq!(
            serde_json::to_value(ProbeView::from(switch)).unwrap()["ask"],
            serde_json::json!({ "kind": "user_switch" })
        );
        for (ask, json) in [
            (
                tandem_proto::ProbeAsk::Back { code: 0x0F },
                serde_json::json!({ "kind": "back", "code": 15 }),
            ),
            (
                tandem_proto::ProbeAsk::Verify { code: 5 },
                serde_json::json!({ "kind": "verify", "code": 5 }),
            ),
            (
                tandem_proto::ProbeAsk::Stalled { code: 5 },
                serde_json::json!({ "kind": "stalled", "code": 5 }),
            ),
        ] {
            let mut e = probe(ProbePlace::Question);
            if let tandem_proto::ProbeEvent::Show { probe, .. } = &mut e {
                probe.ask = ask;
            }
            assert_eq!(
                serde_json::to_value(ProbeView::from(e)).unwrap()["ask"],
                json
            );
        }
        assert_eq!(
            serde_json::to_value(ProbeView::from(tandem_proto::ProbeEvent::Hide { id: 7 }))
                .unwrap(),
            serde_json::json!({ "type": "hide", "id": 7 })
        );
        let p = Notice::from(PowerEvent::PeerLost {
            machine: Machine::Pc,
            reason: "la PC dejó de responder".into(),
            stranded: vec!["m34".into()],
        });
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            serde_json::json!({
                "kind": "power",
                "machine": "pc",
                "reason": "la PC dejó de responder",
                "stranded": ["m34"]
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
            read(serde_json::json!({ "kind": "set_mode", "mode": { "mode": "solo_mac" } })),
            UiCommand::SetMode(Mode::SoloMac)
        );
        assert_eq!(
            read(serde_json::json!({
                "kind": "set_mode",
                "mode": { "mode": "shared", "assign": { "m34": "pc", "m32": "mac" } }
            })),
            UiCommand::SetMode(Mode::Shared(
                [("m34".into(), Machine::Pc), ("m32".into(), Machine::Mac)].into()
            ))
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "move_monitor", "monitor": "m32", "to": "pc" })),
            UiCommand::MoveMonitor {
                monitor: "m32".into(),
                to: Machine::Pc
            }
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "set_builtin", "display": "mirror" })),
            UiCommand::SetBuiltin(BuiltinDisplay::Mirror)
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "set_power", "machine": "pc", "on": false })),
            UiCommand::SetPower {
                machine: Machine::Pc,
                on: false
            }
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "set_power", "machine": "mac", "on": true })),
            UiCommand::SetPower {
                machine: Machine::Mac,
                on: true
            }
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "answer_manual_switch", "id": 4, "done": true })),
            UiCommand::AnswerManualSwitch { id: 4, done: true }
        );
        for (json, answer) in [
            (serde_json::json!({ "kind": "seen" }), ProbeAnswer::Seen),
            (
                serde_json::json!({ "kind": "machine", "machine": "mac" }),
                ProbeAnswer::Machine(Machine::Mac),
            ),
            (
                serde_json::json!({ "kind": "machine", "machine": "pc" }),
                ProbeAnswer::Machine(Machine::Pc),
            ),
            (serde_json::json!({ "kind": "black" }), ProbeAnswer::Black),
            (serde_json::json!({ "kind": "stop" }), ProbeAnswer::Stop),
        ] {
            assert_eq!(
                read(serde_json::json!({ "kind": "answer_probe", "id": 5, "answer": json })),
                UiCommand::AnswerProbe { id: 5, answer }
            );
        }
        assert_eq!(
            read(serde_json::json!({ "kind": "start_calibration", "display": 1 })),
            UiCommand::StartCalibration {
                display: 1,
                monitor: None
            }
        );
        assert_eq!(
            read(serde_json::json!({ "kind": "use_profile", "display": 0, "monitor": "m32" })),
            UiCommand::UseProfile {
                display: 0,
                monitor: Some("m32".into())
            }
        );
        for (kind, cmd) in [
            ("start_pairing", UiCommand::StartPairing),
            ("confirm_pairing", UiCommand::ConfirmPairing),
            ("cancel_pairing", UiCommand::CancelPairing),
            ("unpair", UiCommand::Unpair),
            ("list_displays", UiCommand::ListDisplays),
            ("cancel_calibration", UiCommand::CancelCalibration),
        ] {
            assert_eq!(read(serde_json::json!({ "kind": kind })), cmd);
        }
    }

    #[test]
    fn displays_and_calibration_are_what_the_frontend_reads() {
        let r = ActionResult::from(Reply::Displays(vec![
            DisplayInfo {
                index: 1,
                name: "Odyssey G5".into(),
                pnp_id: Some("SAM7489".into()),
                monitor: Some("m32".into()),
                profile: DisplayProfile::Known {
                    name: "Odyssey G5 32\"".into(),
                    user: true,
                    hdmi: 5,
                    display_port: 15,
                    toml: "x".into(),
                },
            },
            DisplayInfo {
                index: 2,
                name: "U2720Q".into(),
                pnp_id: None,
                monitor: None,
                profile: DisplayProfile::None,
            },
        ]));
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            serde_json::json!({
                "kind": "displays",
                "displays": [
                    {
                        "index": 1, "name": "Odyssey G5", "pnp_id": "SAM7489", "monitor": "m32",
                        "profile": {
                            "kind": "known", "name": "Odyssey G5 32\"", "user": true,
                            "hdmi": 5, "display_port": 15, "toml": "x"
                        }
                    },
                    {
                        "index": 2, "name": "U2720Q", "pnp_id": null, "monitor": null,
                        "profile": { "kind": "none" }
                    }
                ]
            })
        );
        let configured = DisplayView::from(DisplayInfo {
            index: 0,
            name: "a".into(),
            pnp_id: None,
            monitor: None,
            profile: DisplayProfile::Configured {
                hdmi: 6,
                display_port: 9,
            },
        });
        assert_eq!(
            serde_json::to_value(&configured).unwrap()["profile"],
            serde_json::json!({ "kind": "configured", "hdmi": 6, "display_port": 9 })
        );

        let json = |u: CalibrationUpdate| {
            serde_json::to_value(Notice::Calibration { event: u.into() }).unwrap()
        };
        assert_eq!(
            json(CalibrationUpdate::Started {
                display: 1,
                name: "Odyssey G5".into(),
                monitor: None,
                start: Input::DisplayPort,
            }),
            serde_json::json!({
                "kind": "calibration",
                "event": { "type": "started", "display": 1, "name": "Odyssey G5", "monitor": null, "start": "displayport" }
            })
        );
        assert_eq!(
            json(CalibrationUpdate::Progress(CalibrationStep::Trying {
                input: Input::Hdmi,
                code: 5,
                attempt: 1,
                of: 31,
            }))["event"],
            serde_json::json!({
                "type": "progress",
                "step": { "kind": "trying", "input": "hdmi", "code": 5, "attempt": 1, "of": 31 }
            })
        );
        for (step, kind) in [
            (CalibrationStep::Reading, "reading"),
            (
                CalibrationStep::UserSwitch { input: Input::Hdmi },
                "user_switch",
            ),
            (CalibrationStep::Testing { input: Input::Hdmi }, "testing"),
            (
                CalibrationStep::Seen {
                    input: Input::Hdmi,
                    code: 5,
                    seen: false,
                },
                "seen",
            ),
            (CalibrationStep::Returned { code: 15 }, "returned"),
            (
                CalibrationStep::Found {
                    input: Input::Hdmi,
                    code: 5,
                },
                "found",
            ),
            (CalibrationStep::Verifying, "verifying"),
            (
                CalibrationStep::Verified {
                    input: Input::Hdmi,
                    code: 5,
                },
                "verified",
            ),
        ] {
            assert_eq!(
                json(CalibrationUpdate::Progress(step))["event"]["step"]["kind"],
                kind
            );
        }
        assert_eq!(
            json(CalibrationUpdate::Finished(CalibrationOutcome::Saved(
                SavedProfile {
                    name: "Odyssey G5 (SAM7489)".into(),
                    hdmi: 5,
                    display_port: 15,
                    attempts: 2,
                    monitor: Some("m32".into()),
                    file: Some("/d/SAM7489.toml".into()),
                    toml: Some("t".into()),
                    notes: vec!["n".into()],
                }
            )))["event"],
            serde_json::json!({
                "type": "finished",
                "outcome": {
                    "kind": "saved", "name": "Odyssey G5 (SAM7489)", "hdmi": 5, "display_port": 15,
                    "attempts": 2, "monitor": "m32", "file": "/d/SAM7489.toml", "toml": "t",
                    "notes": ["n"]
                }
            })
        );
        assert_eq!(
            json(CalibrationUpdate::Finished(CalibrationOutcome::Failed {
                reason: "r".into()
            }))["event"]["outcome"],
            serde_json::json!({ "kind": "failed", "reason": "r" })
        );
        assert_eq!(
            json(CalibrationUpdate::Finished(CalibrationOutcome::Cancelled))["event"]["outcome"],
            serde_json::json!({ "kind": "cancelled" })
        );
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
    }

    #[test]
    fn txn_and_manual_events_keep_their_data() {
        let s = State {
            owner: Default::default(),
            on: tandem_core::Powered::both(),
            builtin_pref: BuiltinDisplay::Off,
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
                TxnEvent::RollingBack { txn: 1 },
                TxnView::RollingBack { txn: 1 },
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
                TxnEvent::Inconsistent {
                    txn: 1,
                    undo_failures: vec![tandem_proto::UndoFailure {
                        step: Step::UpdateInputGeometry,
                        error: "x".into(),
                    }],
                },
                TxnView::Inconsistent {
                    txn: 1,
                    undo_failures: vec![UndoView {
                        step: Step::UpdateInputGeometry,
                        error: "x".into(),
                    }],
                },
            ),
        ];
        for (event, view) in cases {
            assert_eq!(TxnView::from(event), view);
        }
        assert_eq!(
            ManualView::from(ManualSwitchEvent::Requested {
                id: 1,
                monitor: "m32".into(),
                label: "32\"".into(),
                to: Machine::Pc,
                input: Input::DisplayPort,
            }),
            ManualView::Requested {
                id: 1,
                monitor: "m32".into(),
                label: "32\"".into(),
                to: Machine::Pc,
                input: Input::DisplayPort,
            }
        );
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
            machine: Machine::Pc,
            reason: "la PC dejó de responder (¿corte de corriente?)".into(),
            stranded: vec!["m34".into(), "m32".into()],
        }));
        assert_eq!(
            *rec.notices.lock().unwrap(),
            vec![Notice::Power {
                machine: Machine::Pc,
                reason: "la PC dejó de responder (¿corte de corriente?)".into(),
                stranded: vec!["m34".into(), "m32".into()],
            }]
        );
        // Un aviso de energía no toca el estado: ese llega empujado aparte.
        assert!(rec.snapshots.lock().unwrap().is_empty());
    }
}
