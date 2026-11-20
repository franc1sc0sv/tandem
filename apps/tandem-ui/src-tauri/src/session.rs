//! Live connection to `tandemd` for the main scene.
//!
//! The window does not ask: the session keeps an open connection, requests the desk and the
//! state on connecting and then forwards what the agent pushes (state, transaction
//! progress, pairing, keyboard and mouse). If the agent is not there or drops, the window
//! sees it as [`Snapshot::Unavailable`] and the session retries on its own.
//!
//! It is generic over the transport and over whom to notify ([`Sink`]): the tests
//! use the in-memory IPC with a real agent; the app, the socket or pipe and Tauri
//! events.

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

/// How often to retry if there is no agent. Short: the window has to
/// recover on its own as soon as `tandemd` starts.
pub const RETRY_EVERY: Duration = Duration::from_secs(2);

/// Everything the scene needs to draw itself.
// A single connected scene alive at a time: one more box is not worth it.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Snapshot {
    Connecting,
    Connected {
        /// Machine the agent (and this window) runs on.
        agent: Machine,
        /// The shared desk: the declared screen arrangement (DESIGN §7).
        desk: Config,
        state: State,
        /// The other agent: without a peer, the window warns that this machine works alone.
        peer: PeerView,
        /// Where the pairing stands.
        pairing: PairingView,
        /// The shared keyboard and mouse: each machine's chip.
        input: InputView,
        /// What each machine can do (T-084): the scene is drawn with this, not with the
        /// machine's name.
        caps: Box<CapsView>,
        /// The monitors of both machines with the last thing their owner read (T-110).
        monitors: Vec<MonitorView>,
    },
    Unavailable {
        endpoint: Option<String>,
        reason: String,
    },
}

/// [`PeerStatus`] as JSON for the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PeerView {
    Unpaired,
    Paired {
        name: String,
        /// `None` if no link sees the other agent.
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

/// The role of a path on the Network screen (T-118).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathStateView {
    Primary,
    Backup,
    Unplugged,
    Down,
}

/// One second of a path's history, in microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SampleView {
    pub rtt_us: Option<u32>,
    pub input_p99_us: Option<u32>,
}

/// A path of the Network screen: what `tandem_proto::PathMetrics` says, with the names
/// the window reads.
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

/// A path change ("switched to Wi-Fi in 0.6 s").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SwitchView {
    pub at_ms: u64,
    pub from: Option<LinkView>,
    pub to: Option<LinkView>,
    pub took_ms: Option<u32>,
}

/// [`NetworkStatus`] as JSON for the frontend (`agent-network` event, once per second).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NetworkView {
    pub paths: Vec<PathView>,
    pub switches: Vec<SwitchView>,
    pub measuring: Option<LinkView>,
    /// What the user has to do by hand (for example, assign a cable's IP).
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

/// [`PairingStatus`] as JSON for the frontend.
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
    /// Another machine offers to pair while the paired one is silent (T-345).
    Offered,
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
            PairingStatus::Offered => PairingView::Offered,
        }
    }
}

/// [`InputStatus`] as JSON for the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InputView {
    /// The machine that has the keyboard and mouse.
    pub server: Machine,
    /// Whether the hub is on this machine; `None` without detection.
    pub hub_here: Option<bool>,
    /// The session with the other agent is established.
    pub session: bool,
    /// The cursor is locked on its machine.
    pub locked: bool,
    /// Missing macOS permissions: the window warns about it.
    pub missing: Vec<PermissionView>,
    pub problem: Option<String>,
    /// Where the keyboard is is known (hub, configuration or setup). If not, the window
    /// says "undefined" and never assumes the Mac (T-087).
    pub defined: bool,
    /// The screens the engine reads here and those the other machine announced (T-150): the
    /// window draws them if the desk declares no `[[screens]]`.
    pub screens: Vec<ScreenRect>,
    pub peer_screens: Vec<ScreenRect>,
}

/// [`DeskCapabilities`] as JSON for the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapsView {
    /// This machine (always on for itself).
    pub here: MachineView,
    /// The other one, according to its last announcement; `None` if it was never seen.
    pub peer: Option<MachineView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MachineView {
    pub machine: Machine,
    /// Name, system (`macos`/`windows`) and accent color: the UI has no fixed
    /// "Mac"/"PC" texts (T-097).
    pub name: String,
    pub os: tandem_core::Os,
    pub color: String,
    pub hub: Option<bool>,
    pub input: InputCapsView,
    pub power: PowerCapsView,
    pub services: Vec<String>,
    /// The modules of its system with their state (T-098): the Machine page (T-115).
    pub modules: Vec<ModuleView>,
    /// Detected execution environments (T-105).
    pub environments: Vec<EnvView>,
    /// The machine per the scan (T-105).
    pub hardware: HardwareView,
}

/// [`ModuleInfo`] as JSON for the frontend: the id is the desk's (`clipboard`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleView {
    pub id: &'static str,
    pub name: String,
    pub detail: String,
    pub state: ModuleStateView,
    /// Why it is not available, if it is not.
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

/// [`Environment`] as JSON for the frontend.
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

/// [`Hardware`] as JSON for the frontend (bytes as a number: they fit in an `f64` without loss).
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
    /// `None` if it shares RAM.
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
    /// The binary macOS grants the permissions to (T-090).
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

/// What an agent of `machine` would announce with the test desk, with no peer seen.
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

/// The same names as the permissions guide ([`crate::setup`]).
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

/// Notices that are not state: shown once and gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Notice {
    Txn {
        event: TxnView,
    },
    /// The setup's "press a key" test (T-087).
    KeyProbe {
        event: KeyProbeView,
    },
    /// The other machine was considered off without anyone turning it off (power cut or
    /// suspended): `reason` is shown as is.
    Power {
        machine: Machine,
        reason: String,
    },
    /// Where the cursor is, at 30 Hz at most (T-128); only arrives with [`Action::WatchCursor`].
    Cursor {
        sample: CursorView,
    },
    /// The cursor crossed the edge between the two machines: `edge` is the edge of `from` it
    /// left through.
    Crossed {
        from: Machine,
        to: Machine,
        edge: Side,
    },
    /// The other machine changed the desk and won (D5): "Changed from X · Undo". Undo
    /// is [`Action::UndoDesk`] with `undo_to`.
    DeskChanged {
        by: Machine,
        rev: u64,
        summary: String,
        undo_to: u64,
    },
    /// Progress of a file copy between the machines (`AgentToUi::Transfer`): a notice with a
    /// bar. `finished` closes the copy.
    Transfer {
        upload: bool,
        path: String,
        done: u64,
        total: u64,
        finished: bool,
    },
    /// Progress of a file drag between the machines (T-121).
    Drag {
        status: DragView,
    },
}

/// [`DragStatus`] as JSON for the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DragView {
    /// Copying `name`, file `index` (from 0) of `total`.
    Copying {
        name: String,
        index: u32,
        total: u32,
    },
    /// Everything copied: only the drop on the other machine remains.
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

/// [`CursorSample`] as JSON for the frontend.
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

/// [`KeyProbeStatus`] as JSON for the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct KeyProbeView {
    pub armed: bool,
    pub heard: Option<Machine>,
    pub second: Option<Machine>,
}

impl From<PowerEvent> for Notice {
    fn from(e: PowerEvent) -> Self {
        match e {
            PowerEvent::PeerLost { machine, reason } => Notice::Power { machine, reason },
        }
    }
}

/// [`TxnEvent`] as readable JSON for the frontend (the steps with their `step` tag).
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
    /// Rollback: step `index`, already done, was undone.
    StepUndone {
        txn: u64,
        index: u32,
    },
    Committed {
        txn: u64,
        state: State,
    },
    /// A step failed: `state` is what was left done.
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

/// What the window can ask for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Power on or off `machine` (the other one, in the scene).
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
    /// The window heard a key during the test.
    HeardKey,
    SetKeyboard {
        machine: Machine,
    },
    PingPeer,
    /// "Ask for permissions" (T-090): macOS shows its dialog on behalf of `tandemd`.
    RequestPermissions,
    /// "Lock here" (T-129): locks or releases the cursor on this machine.
    LockHere {
        on: bool,
    },
    /// The desk name (`EditDesk { Name }`, T-159); empty goes back to the default.
    SetDeskName {
        name: String,
    },
    /// The MAC this machine uses to wake the other one (T-159); `null` clears it.
    SetWol {
        mac: Option<String>,
    },
    /// Where the configuration came from; answers [`ActionResult::Setup`].
    GetSetup,
    /// Ask the other machine to install `version` (T-122). Used by "Update all"
    /// from the app core, not the window.
    UpdatePeer {
        version: String,
    },
    /// "Save" of the arrangement (T-117, D18): which side of B A is on and with what alignment. It is
    /// a change to the shared desk, so it reaches the other machine (T-099).
    SetArrangement {
        a_side: Side,
        align: Align,
    },
    /// Turn a module of `machine` on or off (T-098); `module` is the desk's id.
    SetModule {
        machine: Machine,
        #[serde(deserialize_with = "module_id")]
        module: ModuleId,
        on: bool,
    },
    /// Rescan the environments of `machine` (T-105).
    Rescan {
        machine: Machine,
    },
    /// Receive the live resources of both machines ([`Sink::resources`]) while the
    /// visible screen shows them. The session remembers it and requests it again on reconnecting.
    WatchResources {
        on: bool,
    },
    /// Name or color of a machine (the shared desk, D4/D17); `None` does not change.
    SetMachine {
        machine: Machine,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        color: Option<String>,
    },
    /// Receive (or stop receiving) the live cursor as [`Notice::Cursor`] and
    /// [`Notice::Crossed`] (T-128). The session requests it again if it reconnects.
    WatchCursor {
        on: bool,
    },
    /// Receive the network state ([`Sink::network`]) once per second while the
    /// Network screen is visible. The session remembers it and requests it again on reconnecting.
    WatchNetwork {
        on: bool,
    },
    /// Measure a path's bandwidth; answers [`ActionResult::Bandwidth`].
    MeasureLink {
        path: LinkView,
    },
    /// Apply scene `id` (D6): a transaction with progress (T-116).
    ApplyScene {
        id: String,
    },
    /// Switch a compatible monitor's input, with the firmware's verdict (T-110).
    SwitchMonitor {
        monitor: String,
        input: MonitorInput,
    },
    /// Create a scene (T-116); without `id` one is derived from the name.
    AddScene {
        scene: Scene,
    },
    /// Save the changes of an existing scene.
    EditScene {
        scene: Scene,
    },
    DeleteScene {
        id: String,
    },
    /// The processes of a tunnel, also those that ended recently (T-114); answers
    /// [`ActionResult::Processes`]. Tunnels arrive on their own through [`Sink::tunnels`].
    Processes {
        #[serde(deserialize_with = "tunnels::tunnel_id")]
        tunnel: TunnelId,
    },
    /// The tunnels open now (T-154): a window that opens or reloads asks for them because
    /// [`Sink::tunnels`] only notifies on connecting and on each change; answers
    /// [`ActionResult::Tunnels`].
    Tunnels,
    /// The output of a process from the `stdout` and `stderr` cursors (T-114); answers
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
    /// Shared keyboard and mouse (`EditDesk { Prefs }`): the whole pair is sent.
    SetPrefs {
        cmd_ctrl: bool,
        drag_files: bool,
    },
    /// The desk's lock shortcut (`EditDesk { Shortcuts }`); empty = no shortcut.
    SetLockShortcut {
        lock: Vec<String>,
    },
    /// "Undo" of the change notice (D5): returns the desk to version `to_rev`.
    UndoDesk {
        to_rev: u64,
    },
}

/// Adds or subtracts a window to `n`; returns whether there was any and whether there is any.
fn count(n: &mut u32, on: bool) -> (bool, bool) {
    let before = *n > 0;
    *n = if on {
        n.saturating_add(1)
    } else {
        n.saturating_sub(1)
    };
    (before, *n > 0)
}

/// The module id as the desk stores it (`clipboard`, `secure_desktop`).
fn module_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ModuleId, D::Error> {
    let s = String::deserialize(d)?;
    ModuleId::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("no module \"{s}\"")))
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

/// The agent's response to an [`Action`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionResult {
    /// The transaction `txn` started; its progress arrives as [`Notice::Txn`].
    Accepted {
        txn: u64,
    },
    Done,
    Rejected {
        reason: String,
    },
    /// There is no agent to talk to.
    Unavailable {
        reason: String,
    },
    /// The agent's configuration: file, whether it generated it and its port.
    Setup {
        setup: SetupInfo,
    },
    /// How long the ping over the link took.
    Pong {
        link: LinkView,
        millis: u32,
    },
    /// What a path's bandwidth test measured.
    Bandwidth {
        link: LinkView,
        bytes_per_sec: u64,
    },
    /// The processes of a tunnel.
    Processes {
        processes: Vec<ProcessItem>,
    },
    /// The output of a process from where it was requested.
    Output {
        output: Box<OutputItem>,
    },
    /// The open tunnels, grouped by project (T-154).
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

/// Whom to tell what happens (in the app, the window through Tauri events).
pub trait Sink: Send + Sync + 'static {
    fn snapshot(&self, snapshot: &Snapshot);
    fn notice(&self, notice: &Notice);
    /// The versions of both machines (T-122); used by the updater, not the scene.
    fn update(&self, status: &UpdateStatus) {
        let _ = status;
    }

    /// A sample of `machine`'s resources (only while `WatchResources` was requested).
    /// It is not in the [`Snapshot`]: it arrives every second and the window builds its own history.
    fn resources(&self, machine: Machine, resources: &Resources) {
        let _ = (machine, resources);
    }

    /// The network state (only while `WatchNetwork` was requested). It is not in the
    /// [`Snapshot`]: it arrives every second and only the Network screen uses it.
    fn network(&self, network: &NetworkView) {
        let _ = network;
    }

    /// The open tunnels of both machines (Tunnels screen, T-114): they arrive on connecting
    /// and on each change, without being asked for.
    fn tunnels(&self, tunnels: &[ProjectItem]) {
        let _ = tunnels;
    }

    /// The progress of a tunnel's file copy (T-106), for the tunnel's traffic.
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
        // If the window no longer waits for the response, there is no one to give it to.
        let _ = self.reply.send(r);
    }
}

/// What the app keeps to talk to the session.
#[derive(Debug, Clone)]
pub struct Handle {
    actions: mpsc::Sender<Pending>,
    snapshot: watch::Receiver<Snapshot>,
    retry: Arc<Notify>,
}

impl Handle {
    /// The last known state, for a window that just opened.
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
                _ => "the session with tandemd ended".into(),
            };
            return ActionResult::Unavailable { reason };
        }
        answer.await.unwrap_or_else(|_| ActionResult::Unavailable {
            reason: CLOSED.into(),
        })
    }

    /// Retries now, without waiting for [`RETRY_EVERY`].
    pub fn retry_now(&self) {
        self.retry.notify_one();
    }
}

/// A session that cannot start (for example, not knowing where the agent listens):
/// the window shows the reason and every request is answered with it.
pub fn unavailable(endpoint: Option<String>, reason: String) -> Handle {
    let (actions, _) = mpsc::channel(1);
    let (_, snapshot) = watch::channel(Snapshot::Unavailable { endpoint, reason });
    Handle {
        actions,
        snapshot,
        retry: Arc::new(Notify::new()),
    }
}

/// Creates the session. The returned future is the session itself: it runs until the
/// last [`Handle`] is dropped.
///
/// `connect` opens a new connection each time it is called; `endpoint` is only for
/// telling the user where the agent was looked for.
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
    /// How many windows ask for live resources (the main one and the tray share this
    /// session): while there is any, each new connection requests them again.
    watchers: u32,
    /// How many windows ask for the live cursor (T-128), counted the same: while there is any,
    /// each new connection requests it again.
    cursor_watchers: u32,
    /// The Network screen asked for the live network: each new connection requests it again. It is a
    /// boolean and not a count: only the main window shows Network, and reloading it must not
    /// leave the request on forever.
    watch_network: bool,
}

/// How long the response to `MeasureLink` is waited for: the test itself plus the usual.
const MEASURE_TIMEOUT: Duration =
    Duration::from_secs(tandem_proto::net::MEASURE_SECS).saturating_add(QUERY_TIMEOUT);

/// A just-opened connection, with what is needed to draw the scene.
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

/// Why a connection ended.
enum Ended {
    /// The last [`Handle`] was dropped: the session ends.
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

    /// Adds or subtracts a window watching the resources; returns whether there was and whether there is any.
    fn count_watcher(&mut self, on: bool) -> (bool, bool) {
        count(&mut self.watchers, on)
    }

    /// The same for the windows watching the live cursor.
    fn count_cursor_watcher(&mut self, on: bool) -> (bool, bool) {
        count(&mut self.cursor_watchers, on)
    }

    fn publish(&self, snapshot: Snapshot) {
        self.sink.snapshot(&snapshot);
        self.snapshot.send_replace(snapshot);
    }

    /// Connects and requests what is needed to draw the scene, all with the same limit.
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
                        // Only the first to ask and the last to release reach the agent.
                        let (before, now) = self.count_watcher(on);
                        if before == now {
                            p.answer(ActionResult::Done);
                            continue;
                        }
                        p.command = UiCommand::WatchResources { on: now };
                    }
                    if let UiCommand::WatchCursor { on } = p.command {
                        // Same: the connection with the agent counts once for all the windows.
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
                    // The measurement lasts `MEASURE_SECS`: with the usual limit the connection would be considered
                    // lost midway through measuring.
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
                // The final state also arrives pushed; taking it here avoids the
                // scene staying for an instant with the previous one when the progress closes.
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
            AgentToUi::KeyProbe(KeyProbeStatus {
                armed,
                heard,
                second,
            }) => {
                self.sink.notice(&Notice::KeyProbe {
                    event: KeyProbeView {
                        armed,
                        heard,
                        second,
                    },
                });
            }
            AgentToUi::Monitors(new) => self.update_monitors(new),
            AgentToUi::Update(status) => self.sink.update(&status),
            // Only responses inside `request` arrive; a stray one is of no use to anyone.
            AgentToUi::Reply { .. } => {}
            // The new desk already arrives with `Desk`; this is only the notice with "Undo".
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
            // The Tunnels screen (read-only, D9) and each tunnel's traffic (T-114).
            AgentToUi::Tunnels(groups) => self.sink.tunnels(&tunnels::projects_of(groups)),
            // Copies are requested by a client (T-106): the progress goes to Tunnels and, as a notice, to the window.
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
            // Only with `Action::WatchCursor`: the Desk and Arrangement screens paint them.
            AgentToUi::Cursor(sample) => self.sink.notice(&Notice::Cursor {
                sample: sample.into(),
            }),
            AgentToUi::Crossed { from, to, edge } => {
                self.sink.notice(&Notice::Crossed { from, to, edge });
            }
            AgentToUi::Drag(status) => self.sink.notice(&Notice::Drag {
                status: status.into(),
            }),
            // Arrives only if `WatchNetwork` was requested (the Network screen, T-118).
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

    /// Changes the connected scene and publishes it if `change` says it changed.
    fn update(&self, change: impl FnOnce(&mut Snapshot) -> bool) {
        let mut next = self.snapshot.borrow().clone();
        if change(&mut next) {
            self.publish(next);
        }
    }

    /// What each machine can do. This one's side comes from here too: the agent changes
    /// side when pairing ([`tandemd::Agent::adopt_side`]) and announces it in its capabilities;
    /// the connection's `Hello` only says the one from when it connected.
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

    /// Waits until the next attempt; meanwhile, requests are answered without an agent.
    /// `false` if there is no one left to serve.
    async fn wait_retry(&mut self, reason: &str) -> bool {
        let wait = tokio::time::sleep(RETRY_EVERY);
        tokio::pin!(wait);
        loop {
            tokio::select! {
                () = &mut wait => return true,
                () = self.retry.notified() => return true,
                pending = self.actions.recv() => match pending {
                    Some(p) => {
                        // Without an agent there are no resources or network, but the request holds for the next connection.
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
    format!(
        "tandemd did not respond in {} s",
        QUERY_TIMEOUT.as_secs_f64()
    )
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

# What to wake the PC with: without this, the plan does not even try to power it on.
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
        /// Waits until what was received satisfies `done`.
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

        /// Waits for a resource sample equal to `want` from `machine` (fails after 10 s).
        async fn until_resources(&self, machine: Machine, want: &Resources) {
            tokio::time::timeout(Duration::from_secs(10), self.wait_resources(machine, want))
                .await
                .expect("the resources did not arrive");
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

        /// Waits for at least `n` network states to arrive (fails after 10 s).
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
                .expect("the network state did not arrive");
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

    /// Always connects to the agent that is in `slot` at that moment (or fails if there is none).
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

    /// The arrangement changed on the agent reaches the scene, and `GetSetup` says where
    /// the configuration came from.
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

    /// Review of PR #68: if the agent changes side (when pairing), the connected UI
    /// follows; if not, the tray would report this machine's own state as if it were the other's.
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
            panic!("connected was expected");
        };
        assert_eq!(agent, Machine::B);
        assert_eq!(caps.here.machine, Machine::B);
        assert!(
            state.is_on(Machine::B),
            "this machine, with its new side, on"
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
        // Without the network set up, the agent explains why it cannot pair.
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
            panic!("Accepted was expected: {r:?}");
        };
        // Without backends, the agent tries and fails: the window sees each step.
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
            panic!("Started was expected: {notices:?}");
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
        // Without a network pairing is not possible: the agent's reason arrives as is.
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
        assert!(reason.contains("is not running"), "{reason}");

        // Requests are answered without hanging.
        let r = handle
            .act(Action::SetPower {
                machine: Machine::B,
                on: true,
            })
            .await;
        assert!(matches!(r, ActionResult::Unavailable { .. }), "{r:?}");

        // The agent starts: the next attempt finds it.
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
        // Without paused time: if it waited RETRY_EVERY, the test would take 2 s.
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
        // Accepts the connection and never greets.
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
        assert_eq!(reason, "tandemd did not respond in 2 s");
        // And it retries.
        rec.until(|s, _| s.len() >= 2).await;
        assert!(tries.load(Ordering::SeqCst) >= 2);
    }

    /// The live cursor arrives as notices only if the window asked for it, and is requested again
    /// on reconnecting.
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

        // Without asking for it, the connection receives nothing and the engine does not even find out.
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

        // Another agent in the same place: the session reconnects and requests it again.
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

    /// The progress of an agent's file drag reaches the window as a notice (T-121).
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
            .publish(DragStatus::Failed("no space".into()));
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
                "status": { "kind": "failed", "reason": "no space" }
            })
        );
    }

    async fn within_secs<F: Future>(f: F) -> F::Output {
        tokio::time::timeout(Duration::from_secs(10), f)
            .await
            .expect("the test exceeded the maximum time")
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
        let handle = unavailable(None, "no HOME".into());
        assert_eq!(
            handle.snapshot(),
            Snapshot::Unavailable {
                endpoint: None,
                reason: "no HOME".into()
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
                reason: "no HOME".into()
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
            pairing(PairingStatus::Offered),
            serde_json::json!({ "kind": "offered" })
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
            reason: "the PC stopped responding".into(),
        });
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            serde_json::json!({
                "kind": "power",
                "machine": "b",
                "reason": "the PC stopped responding",
            })
        );
        assert_eq!(
            serde_json::to_value(ActionResult::Rejected { reason: "x".into() }).unwrap(),
            serde_json::json!({ "kind": "rejected", "reason": "x" })
        );
    }

    /// T-115: the Machine page requests the resources; they reach the window and, if the agent
    /// restarts, the session requests them again on its own.
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

    /// The main window and the tray share the session: if one stops watching the
    /// resources, the other keeps receiving them.
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

    /// T-115: turning off a module from the Machine page reaches the agent and comes back in the
    /// capabilities with its new state.
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
        // Rescan of this machine: accepted and what was found arrives with the capabilities.
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
            name: "Protected screens".into(),
            detail: "UAC".into(),
            state: ModuleState::Unavailable {
                reason: "Service missing".into(),
            },
            switchable: false,
        });
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            serde_json::json!({
                "id": "secure_desktop", "name": "Protected screens", "detail": "UAC",
                "state": "unavailable", "reason": "Service missing", "switchable": false
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
            read(serde_json::json!({ "kind": "set_desk_name", "name": "Office" })),
            UiCommand::EditDesk {
                patch: DeskPatch::Name("Office".into())
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
            serde_json::from_value::<Action>(serde_json::json!({ "kind": "set_module", "machine": "b", "module": "nothing", "on": false }))
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
            read(serde_json::json!({ "kind": "apply_scene", "id": "game" })),
            UiCommand::ApplyScene { id: "game".into() }
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
        let scene = serde_json::json!({ "id": "game", "name": "Game", "monitors": [["m-1", "b"]] });
        assert!(matches!(
            read(serde_json::json!({ "kind": "add_scene", "scene": scene })),
            UiCommand::EditDesk { patch: DeskPatch::AddScene(s) } if s.name == "Game"
        ));
        assert_eq!(
            read(serde_json::json!({ "kind": "delete_scene", "id": "game" })),
            UiCommand::EditDesk {
                patch: DeskPatch::DeleteScene { id: "game".into() }
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
                summary: "Arrangement".into(),
                undo_to: 4
            })
            .unwrap(),
            serde_json::json!({ "kind": "desk_changed", "by": "b", "rev": 5, "summary": "Arrangement", "undo_to": 4 })
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
        // What concerns monitors no longer exists: the window cannot ask for it.
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
            reason: "the PC stopped responding (power cut?)".into(),
        }));
        assert_eq!(
            *rec.notices.lock().unwrap(),
            vec![Notice::Power {
                machine: Machine::B,
                reason: "the PC stopped responding (power cut?)".into(),
            }]
        );
        // A power notice does not touch the state: that one arrives pushed separately.
        assert!(rec.snapshots.lock().unwrap().is_empty());
    }
    /// T-118: the Network screen asks for the live network and it reaches the window with its three paths;
    /// if the agent restarts, the session requests it again on its own.
    #[tokio::test]
    async fn the_network_reaches_the_window_and_survives_a_reconnection() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;
        assert!(
            rec.networks.lock().unwrap().is_empty(),
            "it does not arrive without asking"
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

    /// T-114: on connecting the window receives the tunnels without asking for them, and the processes of a
    /// tunnel that does not exist are rejected without bringing the session down.
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
            "the session is still alive: {got:?}"
        );
        rec.until(|s, _| connected(s)).await;
    }

    /// T-154: a new window asks for the existing tunnels instead of waiting for a change.
    #[tokio::test]
    async fn a_new_window_can_ask_for_the_tunnels() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_in(dir.path());
        let slot = Arc::new(Mutex::new(Some(a.connector.clone())));
        let (handle, rec) = session(&slot);
        rec.until(|s, _| connected(s)).await;
        match handle.act(Action::Tunnels).await {
            ActionResult::Tunnels { tunnels } => assert!(tunnels.is_empty()),
            other => panic!("the tunnels were expected: {other:?}"),
        }
    }

    /// Stopping watching the network cuts the flow; and a request made without an agent holds for the
    /// next connection.
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

    /// Measuring a path with no connection over it is rejected with the agent's reason, without bringing the
    /// session down.
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
