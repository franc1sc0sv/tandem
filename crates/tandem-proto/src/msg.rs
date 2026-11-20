//! The protocol messages.
//!
//! There are three channels and all of them start with a [`Hello`]:
//! - agent <-> agent: [`ControlMsg`] in both directions;
//! - agent <-> agent before trusting each other: [`PairingMsg`], only over the
//!   pairing connection;
//! - UI -> agent: [`UiRequest`]; agent -> UI: [`AgentToUi`].

use serde::{Deserialize, Serialize};
use tandem_core::{
    Config, DeskPatch, DeskVersion, HistoryEntry, Machine, MachineInfo, MonitorInput, Side, State,
    Step,
};

use crate::cursor::CursorSample;
use crate::env::{Environment, Hardware, Resources};
use crate::files::{Direction, FilesReport, ForwardInfo, TransferProgress};
use crate::net::NetworkStatus;
use crate::tunnel::{
    ClientKind, ExecSpec, OutputCursor, ProcessId, ProcessOutput, Project, ProjectTunnels, Tunnel,
    TunnelId, TunnelProcess,
};
use crate::workspace::{BringBackReport, WorkspaceSpec};
use crate::{PROTOCOL_VERSION, ProtoError, wire};

/// Who opens the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Peer {
    Agent(Machine),
    Ui,
    /// A Claude or terminal client over the local IPC (`tandem mcp`, `tandem`): the
    /// only one that can open and close tunnels (DESIGN §11; the UI only watches them, D9).
    Client(ClientKind),
}

/// First message of every connection, in each direction.
///
/// `version` must remain the **first field** in every version:
/// [`crate::decode_hello`] reads it before the rest so it can reject with a
/// clear error even if the rest has changed shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub version: u16,
    pub peer: Peer,
}

impl Hello {
    pub fn new(peer: Peer) -> Self {
        Hello {
            version: PROTOCOL_VERSION,
            peer,
        }
    }

    pub fn check_version(&self) -> Result<(), ProtoError> {
        check_version(self.version)
    }
}

pub(crate) fn check_version(theirs: u16) -> Result<(), ProtoError> {
    if theirs == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(ProtoError::IncompatibleVersion {
            ours: PROTOCOL_VERSION,
            theirs,
        })
    }
}

/// Control plane between the two agents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlMsg {
    /// Heartbeat; the other side answers [`ControlMsg::Pong`] with the same `nonce`.
    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
    /// The orchestrator asks the other agent for a plan step that only it can do.
    ExecuteStep {
        id: u64,
        #[serde(with = "wire::step")]
        step: Step,
    },
    /// Reply to [`ControlMsg::ExecuteStep`] with the same `id`.
    StepResult {
        id: u64,
        outcome: StepOutcome,
    },
    GetState,
    State(State),
    /// First message of a stream that is not a request stream: what follows on it are the
    /// messages of that channel, not [`ControlMsg`].
    Open(Channel),
    /// Where the hub sees this agent and whom it has as keyboard and mouse server.
    Hub(HubReport),
    /// The screens read by the sender (the ones it uses if the desk does not declare
    /// its own), for the keyboard and mouse geometry.
    Screens(Vec<ScreenRect>),
    /// What the sender has and knows how to do (T-084). Sent on connecting, on each
    /// new link and every time it changes; decisions come from here, not from the machine
    /// name (DESIGN §3.7).
    Capabilities(Box<Capabilities>),
    /// The sender's shared desk, with its stamp. Sent on connecting and on every
    /// change; the last one wins (T-099, [`DeskVersion::receive`]).
    Desk(Box<DeskVersion>),
    /// Setup's "Press a key" (T-087): arm or disarm listening, or report
    /// that the key was heard here.
    KeyProbe(KeyProbeMsg),
    /// The user said in setup where the keyboard and mouse are.
    Keyboard(Machine),
    /// What the owner of each monitor it has read (DECISIONS D13): its real input, whether
    /// it is on and the latest events. Only the owner reads events; the other machine
    /// knows them from here. Sent every time something changes and when a link appears.
    Monitors(Vec<MonitorReport>),
    /// The sender wants the receiver's live resources (T-105). With `on` it is
    /// renewed every few seconds while someone is watching: if it stops arriving (the link
    /// dropped), the other side stops sending them on its own.
    WatchResources {
        on: bool,
    },
    /// A sample of the sender's resources.
    Resources(Box<Resources>),
    /// Rescan the receiver's environments; what is found goes in its next
    /// [`ControlMsg::Capabilities`].
    Rescan,
    /// Update both machines, one at a time (T-122).
    Update(UpdateMsg),
    /// Who sends the keyboard and mouse: both capture and the machine that
    /// receives physical input sends (DECISIONS D7, DESIGN §7).
    Focus(FocusMsg),
    /// The sender wants the receiver's live cursor (T-128): samples and
    /// crossings. With `on` it is renewed every few seconds while someone is watching, like
    /// [`ControlMsg::WatchResources`].
    WatchCursor {
        on: bool,
    },
    /// A sample of the sender's cursor, which has focus. It is dropped if the
    /// link lags: the next one replaces it.
    Cursor(CursorSample),
    /// The cursor crossed the edge between the two machines.
    Crossed {
        from: Machine,
        to: Machine,
        /// The edge of `from` it left through.
        edge: Side,
    },
    /// How the sender classifies the connection it travels on, according to its output
    /// interface and not according to the configuration (T-094). Sent when opening each connection; the link
    /// label is the worse of the two classifications.
    LinkClass {
        kind: Link,
    },
    /// File drag between the two machines (T-121).
    Drag(DragMsg),
    /// The sender left the desk ("Leave desk"): it forgot the receiver's key, which
    /// also forgets it and is left without a peer (T-152). Without a link it does not arrive: the other
    /// keeps trusting until it leaves too.
    Unpaired,
}

/// The file drag messages (D14), one per `DragPeer` step. `id` is set by
/// the source: a reply from an old drag is not confused with the current one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DragMsg {
    /// Source -> destination: prepares a temporary folder for `names`.
    Begin { id: u64, names: Vec<String> },
    /// Destination -> source: the folder is ready (its path) or why it could not be.
    Staged {
        id: u64,
        dir: Result<String, String>,
    },
    /// Source -> destination: everything is copied, start the synthetic drag.
    Ready { id: u64 },
    /// Destination -> source: how the drag ended there.
    Finished {
        id: u64,
        end: Result<DragOutcome, String>,
    },
    /// Source -> destination: abort and delete what was copied.
    Cancel { id: u64 },
}

/// How the drag ended at the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DragOutcome {
    /// Dropped on an app or folder: the system copied the files.
    Dropped,
    /// Esc, or dropped outside a valid target.
    Cancelled,
}

/// File drag progress for the UI notice (T-121).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DragStatus {
    /// Copying `name`, file `index` (from 0) of `total`.
    Copying {
        name: String,
        index: u32,
        total: u32,
    },
    /// All copied: all that remains is dropping on the other machine.
    Copied {
        total: u32,
    },
    Dropped,
    Cancelled,
    Failed(String),
}

/// What the two agents tell each other to update (T-122). The versions are the app's
/// (`x.y.z` or `x.y.z-rc.n`); the UI is the one that compares them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateMsg {
    /// The version the sender is running. Sent on connecting: this way the other knows when
    /// it came back with the new version.
    Version(String),
    /// "Install `version`": the agent that receives it hands it to its UI, which is the one
    /// downloads and restarts.
    Install { version: String },
}

/// The versions of the two machines, for the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateStatus {
    /// This agent's.
    pub here: String,
    /// The last one the other announced; `None` if it was never heard.
    pub peer: Option<String>,
    /// The other asked this machine to install this version.
    pub install: Option<String>,
    /// The other connected speaking another protocol version (its own): it runs another
    /// Tandem version, although which one cannot be known.
    pub peer_protocol: Option<u16>,
}

/// Keyboard and mouse arbitration with no owner (D7). Each change bumps `epoch`; with two
/// differing views the one with the higher `epoch` wins and, if tied, the one from side `A`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FocusMsg {
    /// The sender receives physical input and is not the one sending: it asks for it.
    Claim { epoch: u64 },
    /// The sender stops sending: from `epoch` the receiver sends.
    Grant { epoch: u64 },
    /// Who sends according to the sender (on connect, every so often and on an old request).
    State { epoch: u64, server: Machine },
}

/// The messages of the "press a key" test between the two agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyProbeMsg {
    Arm,
    Disarm,
    /// The key reached the machine that sent it.
    Heard,
    /// The sender is already listening: sent again when a link appears, because the `Arm` may have
    /// gone out before there was one. Unlike `Arm` it does not clear what the receiver heard.
    /// Last, so the discriminants of the other messages stay where they were.
    Armed,
}

/// What a machine has and knows how to do, as its agent announces it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub machine: Machine,
    /// Its system, name and color (DECISIONS A1): the UI and the decisions that depend
    /// on the system come from here, not from the side.
    pub info: MachineInfo,
    /// The keyboard and mouse hub is connected here; `None` without detection.
    pub hub: Option<bool>,
    pub input: InputCaps,
    pub power: PowerCaps,
    /// Data plane services it offers (SSH, folders...); empty until v2.
    pub services: Vec<String>,
    /// The modules that exist on its system, with their state (DECISIONS D3, T-098). The
    /// ones that do not exist there do not appear.
    pub modules: Vec<ModuleInfo>,
    /// Detected execution environments (T-105); empty until the first scan finishes.
    pub environments: Vec<Environment>,
    /// The hardware according to the scan (T-105).
    pub hardware: Hardware,
    /// Monitors with a live DDC channel from this machine. Empty until T-110.
    pub monitors: Vec<MonitorChannel>,
}

/// Tandem's modules. Each machine announces only the ones that exist on its system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ModuleId {
    /// Shared keyboard and mouse.
    Input,
    Clipboard,
    /// Drag files between machines (D14).
    Files,
    /// Claude tunnels (D8).
    Tunnels,
    /// Monitor control over DDC (D11).
    Monitors,
    /// Wake-on-LAN and clean shutdown.
    Power,
    /// Input on the Windows secure desktop (UAC, sign-in; T-027).
    SecureDesktop,
}

impl ModuleId {
    /// As stored in the desk (`[modules.a] clipboard = false`): stable, does not change
    /// even if the name the UI shows changes.
    pub fn as_str(self) -> &'static str {
        match self {
            ModuleId::Input => "input",
            ModuleId::Clipboard => "clipboard",
            ModuleId::Files => "files",
            ModuleId::Tunnels => "tunnels",
            ModuleId::Monitors => "monitors",
            ModuleId::Power => "power",
            ModuleId::SecureDesktop => "secure_desktop",
        }
    }

    /// The inverse of [`ModuleId::as_str`].
    pub fn parse(s: &str) -> Option<ModuleId> {
        ModuleId::ALL.into_iter().find(|m| m.as_str() == s)
    }

    pub const ALL: [ModuleId; 7] = [
        ModuleId::Input,
        ModuleId::Clipboard,
        ModuleId::Files,
        ModuleId::Tunnels,
        ModuleId::Monitors,
        ModuleId::Power,
        ModuleId::SecureDesktop,
    ];
}

/// What state a module is in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleState {
    Active,
    /// The user turned it off.
    Off,
    /// It cannot work; `reason` says why and, if possible, what to do.
    Unavailable {
        reason: String,
    },
}

/// A module as its machine announces it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleInfo {
    pub id: ModuleId,
    /// For the UI ("Keyboard and mouse").
    pub name: String,
    /// What it does on this machine (the line under the name).
    pub detail: String,
    pub state: ModuleState,
    /// The user can turn it on and off ([`UiCommand::SetModule`]).
    pub switchable: bool,
}

/// A monitor the machine that announces it sees (T-110): with a live DDC channel (it answered the
/// read-only probe, whether compatible or not) or, [`MonitorCompat::NoChannel`], only by its EDID
/// (T-340).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorChannel {
    /// `<model>-<short hash of the EDID serial>`: the same over DP and over HDMI.
    pub id: String,
    /// The model, for the UI.
    pub name: String,
    pub compat: MonitorCompat,
    /// How long the probe took to answer, measured when opening the channel (it does not change while
    /// the channel stays alive, so the owner choice does not oscillate).
    pub latency_ms: u32,
    /// The monitor input this machine's cable goes to, according to the interface type
    /// of its EDID (E-EDID 1.4); `None` if the EDID does not say (T-111).
    pub via: Option<MonitorInput>,
}

/// Whether a monitor speaks the own-firmware standard (DECISIONS D11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MonitorCompat {
    Compatible {
        protocol: u8,
    },
    /// It is shown, but nothing is ever written to it.
    NotCompatible {
        reason: String,
    },
    /// The system has its EDID, but it does not answer over DDC (the 34" only answers on the
    /// input it shows, T-340). It says which input and screen this machine's cable is; it never
    /// owns the monitor and is never written to.
    NoChannel,
}

impl MonitorCompat {
    pub fn is_compatible(&self) -> bool {
        matches!(self, MonitorCompat::Compatible { .. })
    }
}

/// What the owner of a monitor read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorReport {
    pub id: String,
    /// The input on screen (the real one, from `0xEE`); `None` if it could not be read.
    pub input: Option<MonitorInput>,
    pub power_on: Option<bool>,
    pub signal: Option<bool>,
    /// The latest events, oldest to newest.
    pub events: Vec<MonitorEvent>,
}

/// A firmware event, with the time already converted to the owner's clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorEvent {
    /// Milliseconds since the Unix epoch, according to the owner machine's clock.
    pub at_ms: u64,
    pub kind: MonitorEventKind,
    /// The input it talks about, if it talks about one.
    pub input: Option<MonitorInput>,
}

/// What happened, for the icon and the UI text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MonitorEventKind {
    /// The monitor started (or restarted).
    Boot,
    /// The input on screen changed (over DDC or from the monitor menu).
    Input,
    /// Someone requested an input change over DDC.
    SwitchRequested,
    /// The firmware confirmed the change: stable image on the new input.
    SwitchDone,
    /// The change gave no image; the monitor stays on the previous one.
    SwitchFailed,
    /// It was turned on or off.
    Power,
    /// The signal was gained or lost.
    Signal,
    /// A menu setting changed.
    Setting,
    /// Events were lost (the ring wrapped between two reads).
    Lost,
    Other,
}

/// A monitor as the UI sees it (Monitors screen, mockup `V3-Monitors`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorView {
    pub id: String,
    pub name: String,
    pub compat: MonitorCompat,
    /// The machines that see it: with a live DDC channel or only by its EDID (T-340).
    pub seen_by: Vec<Machine>,
    /// The input each machine that sees it has its cable on, according to its EDID (T-111).
    pub via: Vec<(Machine, MonitorInput)>,
    /// Who reads and controls it; `None` if it is not compatible or nobody has a channel.
    pub owner: Option<Machine>,
    /// The latest the owner read; `None` while there is no reading.
    pub report: Option<MonitorReport>,
}

/// A machine's keyboard and mouse engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputCaps {
    /// It can capture the keyboard and mouse (have them and send them).
    pub capture: bool,
    /// It can inject them (receive them from the other).
    pub inject: bool,
    /// macOS permissions it is missing.
    pub missing: Vec<InputPermission>,
    /// The process the agent runs as: macOS grants permissions to that exact binary, and
    /// the UI tells the user which one without going through a terminal (T-090).
    pub process: ProcessInfo,
}

/// Which process the agent is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// Path of the running executable (`/Applications/Tandem.app/Contents/MacOS/tandemd`);
    /// empty if the system did not provide it.
    pub exe: String,
    pub pid: u32,
    /// When it started, in seconds since the Unix epoch: after restarting the agent it changes, and
    /// so the UI knows that the permissions just granted are now evaluated in a new process.
    pub started: u64,
}

/// A machine's power.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerCaps {
    /// It can wake the other (it has its `[power.wol]`).
    pub wake_peer: bool,
    /// It can be woken by Wake-on-LAN.
    pub wakeable: bool,
    /// It knows how to shut down cleanly on the other's request.
    pub power_off: bool,
    /// The MAC of this machine's Ethernet card (`aa:bb:cc:dd:ee:ff`), so the other
    /// stores it when pairing and can wake it (T-159). `None` if it could not be determined.
    pub mac: Option<String>,
    /// The MAC this machine wakes the other with (its `[power.wol]`): the one learned when
    /// pairing or the one typed by hand. `None`: it cannot power it on.
    pub wol_mac: Option<String>,
}

/// What each machine of the desk knows, for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeskCapabilities {
    /// This machine.
    pub here: Capabilities,
    /// The other, according to its latest announcement; `None` if it was never seen.
    pub peer: Option<Capabilities>,
}

/// The input's and the clipboard's own streams, apart from the request one: a
/// large image does not hold up the keys, and each one has its own messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channel {
    /// The keyboard and mouse session: opened by the server.
    Input,
    /// The clipboard: opened by side A.
    Clipboard,
    /// A tunnel request ([`crate::TunnelUp`]): opened by the source. If it is
    /// [`crate::TunnelUp::Attach`], the stream becomes the tunnel's data channel.
    Tunnel,
    /// A path's bandwidth test (T-102): the requester sends unframed data
    /// for [`crate::net::MEASURE_SECS`] and closes; the other answers
    /// [`crate::net::MeasureReport`].
    Measure,
}

/// What one agent reports to the other to decide the roles (DESIGN §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubReport {
    /// The hub (already debounced) is on the sender's machine.
    pub hub: bool,
    /// Who the server is according to the sender.
    pub server: Machine,
}

/// A screen, in the native coordinates of its machine's cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenRect {
    /// Its id in the arrangement (`mac-1`, or the one from a `[[screens]]`).
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Native units per logical point, in thousandths (1500 = 150%): an integer so
    /// the message can be compared.
    pub scale_milli: u32,
}

/// Pairing by numeric comparison (DESIGN §9), in the order they are sent:
/// the initiator sends `Offer`, the responder `Answer`, the initiator `Reveal`; then
/// each side sends `Confirm` when its user confirms the code, or `Reject`.
///
/// Keys travel as raw bytes: this crate does not know the identity types. Whoever
/// receives them checks they are the same ones the connection authenticated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairingMsg {
    Offer {
        public_key: [u8; 32],
        name: String,
        commitment: [u8; 32],
    },
    Answer {
        public_key: [u8; 32],
        name: String,
        nonce: [u8; 32],
    },
    Reveal {
        nonce: [u8; 32],
    },
    /// The user on this side confirmed seeing `code`.
    Confirm {
        code: u32,
    },
    /// The user on this side rejected the code or canceled.
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepOutcome {
    Done,
    Failed(String),
}

impl From<Result<(), String>> for StepOutcome {
    fn from(r: Result<(), String>) -> Self {
        match r {
            Ok(()) => StepOutcome::Done,
            Err(e) => StepOutcome::Failed(e),
        }
    }
}

impl From<StepOutcome> for Result<(), String> {
    fn from(o: StepOutcome) -> Self {
        match o {
            StepOutcome::Done => Ok(()),
            StepOutcome::Failed(e) => Err(e),
        }
    }
}

/// UI request; the reply arrives as [`AgentToUi::Reply`] with the same `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiRequest {
    pub id: u64,
    pub command: UiCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UiCommand {
    GetState,
    /// The shared desk (the declared screen arrangement).
    GetDesk,
    /// Power `machine` on or off (the other, normally); powering off is the clean
    /// shutdown transaction. Only if some machine can (see [`Capabilities::power`]).
    SetPower {
        machine: Machine,
        on: bool,
    },
    /// Who the agent is paired with and over which link it sees them.
    GetPeer,
    /// How pairing is going.
    GetPairing,
    /// Look for the other machine to pair; its progress arrives in [`AgentToUi::Pairing`].
    StartPairing,
    /// The user sees the same code on both screens.
    ConfirmPairing,
    /// The user rejects the code, or stops looking.
    CancelPairing,
    /// Forget the peer: it is removed from trust and no longer searched for.
    Unpair,
    /// Who the keyboard and mouse server is and whether this machine is missing permissions.
    GetInput,
    /// Ask macOS for the missing permissions (system dialog), on behalf of this
    /// agent. Only on the user's explicit order; the new state arrives in
    /// capabilities (T-090).
    RequestPermissions,
    /// Where this agent's configuration came from and which port it listens on.
    GetSetup,
    /// What each machine can do ([`DeskCapabilities`]).
    GetCapabilities,
    /// "Press a key": both machines listen (or stop listening).
    KeyProbe {
        on: bool,
    },
    /// This machine's window heard a key.
    HeardKey,
    /// The user confirms where the keyboard and mouse are. It is saved and the
    /// other machine is told.
    SetKeyboard(Machine),
    /// Visible latency of the link with the other machine.
    PingPeer,
    /// Turn a module of `machine` (this one or the other) on or off. It is a change to the
    /// shared desk (`Desk.modules`, D4), so it reaches the other with the desk; the
    /// new state arrives in [`AgentToUi::Machines`].
    SetModule {
        machine: Machine,
        module: ModuleId,
        on: bool,
    },
    /// Change the shared desk (DESIGN §4.4): it is applied here, saved and sent to the other
    /// machine. Answers [`Reply::Desk`] with the resulting desk, or the reason for rejection.
    EditDesk {
        patch: DeskPatch,
    },
    /// Go back to version `to_rev` of the history (D5, "Undo"): it is a new change with
    /// exactly that version's desk.
    UndoDesk {
        to_rev: u64,
    },
    /// The latest desk versions this machine had.
    GetDeskHistory,
    /// The monitors of both machines ([`MonitorView`]).
    GetMonitors,
    /// Change the input of a compatible monitor: a transaction with the firmware's
    /// verdict, done by its owner (answered with [`Reply::Accepted`]).
    SwitchMonitor {
        monitor: String,
        input: MonitorInput,
    },
    /// Rescan the environments of `machine`; what is found arrives in
    /// [`AgentToUi::Machines`] (T-105).
    Rescan {
        machine: Machine,
    },
    /// Receive (or stop receiving) [`AgentToUi::Resources`] from both machines over this
    /// connection. It is sampled only while someone is watching; when the connection closes it stops.
    WatchResources {
        on: bool,
    },
    /// The versions of both machines (T-122).
    GetUpdate,
    /// Ask the other machine to install `version` (T-122); its progress shows in
    /// [`AgentToUi::Update`] when it comes back with the new version.
    UpdatePeer {
        version: String,
    },
    /// Apply scene `id` of the desk (D6): a transaction (answered with
    /// [`Reply::Accepted`]). Creating, editing and deleting scenes are desk patches
    /// ([`UiCommand::EditDesk`]).
    ApplyScene {
        id: String,
    },
    /// "Lock here" (T-129): `on` locks the cursor on this machine (it does not cross to the other)
    /// and `!on` releases it; same effect as the input engine's lock shortcut.
    LockHere {
        on: bool,
    },
    /// The MAC this machine wakes the other with (T-159): saved in `[power.wol]`.
    /// `None` deletes it. Answers [`Reply::Done`] or the reason for rejection (invalid MAC).
    SetWol {
        mac: Option<String>,
    },
    /// Test agents only (T-239): relative motion fed to this machine's input engine as if it
    /// had been captured, in pixels. The Windows capture drops injected input on purpose (its
    /// loop guard), so the hardware suite drives the edge this way. Rejected unless the agent's
    /// `[input] synthetic_motion` is on.
    SyntheticMotion {
        dx: i32,
        dy: i32,
    },
    /// Open a tunnel to `target` (the other machine). Only from a
    /// [`Peer::Client`]; answers [`Reply::Tunnel`] (T-103).
    OpenTunnel {
        title: String,
        project: Option<Project>,
        target: Machine,
        /// The client's session (Claude's).
        session: String,
        /// A git repo that goes along with the tunnel (T-107): it is copied to the destination and the
        /// processes start in its folder.
        workspace: Option<WorkspaceSpec>,
    },
    /// Close a tunnel opened from this machine: its resources on the destination are closed.
    /// Only from a [`Peer::Client`].
    CloseTunnel {
        id: TunnelId,
    },
    /// The tunnels of both machines, grouped by project ([`Reply::Tunnels`]).
    GetTunnels,
    /// Receive (or stop receiving) [`AgentToUi::Network`] once per second over this
    /// connection, while the Network screen is visible (T-102).
    WatchNetwork {
        on: bool,
    },
    /// Measure a path's bandwidth (a [`crate::net::MEASURE_SECS`]
    /// test over a data stream). Answered when it finishes with
    /// [`Reply::Bandwidth`] or the reason for rejection.
    MeasureLink {
        path: Link,
    },
    /// Receive (or stop receiving) [`AgentToUi::Cursor`] and [`AgentToUi::Crossed`] from both
    /// machines over this connection (T-128). It is sampled only while some connection,
    /// from here or from the other machine, asks for it; when the connection closes it stops.
    WatchCursor {
        on: bool,
    },
    /// Copy `local` (file or folder on this machine) to `remote` on the tunnel's
    /// destination (T-106). Only from a [`Peer::Client`]; it takes a while, so it answers when it finishes
    /// with [`Reply::Files`] and meanwhile sends [`AgentToUi::Transfer`].
    FilesPut {
        tunnel: TunnelId,
        local: String,
        remote: String,
        /// Credentials that are sent, one by one (paths relative to `local`). By
        /// default nothing that looks like one is copied (T-130).
        include: Vec<String>,
    },
    /// Bring `remote` from the tunnel's destination to `local` (T-106). Answers like `FilesPut`.
    FilesGet {
        tunnel: TunnelId,
        remote: String,
        local: String,
    },
    /// Forward a TCP port through the tunnel (T-106); answers [`Reply::Forward`]. It closes
    /// with the tunnel or with [`UiCommand::StopForward`].
    PortForward {
        tunnel: TunnelId,
        local_port: u16,
        remote_host: String,
        remote_port: u16,
        direction: Direction,
    },
    /// Stop forwarding port `id` (T-106).
    StopForward {
        id: u64,
    },
    /// Launch a process in a tunnel opened from this machine (T-104). Only from a
    /// [`Peer::Client`]; answers [`Reply::Process`].
    Exec {
        tunnel: TunnelId,
        spec: ExecSpec,
    },
    /// Kill a process with its whole tree. Only from a [`Peer::Client`].
    Kill {
        tunnel: TunnelId,
        process: ProcessId,
    },
    /// A process's output from `from` (up to `max` bytes of each stream); answers
    /// [`Reply::Output`].
    Output {
        tunnel: TunnelId,
        process: ProcessId,
        from: OutputCursor,
        max: u32,
    },
    /// The processes of a tunnel, including the ones that finished recently
    /// ([`Reply::Processes`]).
    GetProcesses {
        tunnel: TunnelId,
    },
    /// Bring the tunnel workspace's work as a branch of the source repo (T-107);
    /// `commit` is the message for committing what is pending on the destination. Only from
    /// a [`Peer::Client`]; answers [`Reply::BroughtBack`].
    BringBack {
        tunnel: TunnelId,
        commit: Option<String>,
        /// Branch name (`tandem/<name>`); by default, the tunnel's title.
        name: Option<String>,
    },
}

/// The agent's configuration, for the UI to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupInfo {
    /// `tandem.toml`; `None` if the agent has no file (tests).
    pub config_file: Option<String>,
    /// The agent generated it on first start (T-082).
    pub generated: bool,
    /// The agent's real UDP port; `None` without network.
    pub port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentToUi {
    Reply {
        id: u64,
        reply: Reply,
    },
    /// Spontaneous notice when the state changes for any reason.
    State(State),
    Txn(TxnEvent),
    Power(PowerEvent),
    /// Spontaneous notice when the peer or the link to it changes.
    Peer(PeerStatus),
    /// Spontaneous notice with each pairing step.
    Pairing(PairingStatus),
    /// Spontaneous notice when the keyboard and mouse change (roles, session, permissions).
    Input(InputStatus),
    /// Spontaneous notice when the shared desk changes.
    Desk(Config),
    /// Spontaneous notice when what either machine announces changes (system,
    /// name, color, modules, environments, monitors). `remote` is `None` if the other was never
    /// seen.
    Machines {
        local: Box<Capabilities>,
        remote: Option<Box<Capabilities>>,
    },
    /// Progress of the setup's "press a key" test (T-087).
    KeyProbe(KeyProbeStatus),
    /// The other machine changed the desk and won (D5): the "Changed from X ·
    /// Undo" notice. Undo is [`UiCommand::UndoDesk`] with `undo_to`.
    DeskChanged {
        by: Machine,
        rev: u64,
        /// Which sections changed ("Arrangement and shortcuts").
        summary: String,
        /// The version that was here before the change.
        undo_to: u64,
    },
    /// Spontaneous notice when any monitor changes (channel, owner, input, events).
    Monitors(Vec<MonitorView>),
    /// Live resources of `machine`, to the connections that asked for
    /// [`UiCommand::WatchResources`] (T-105).
    Resources {
        machine: Machine,
        resources: Box<Resources>,
    },
    /// The versions of both machines and whether the other asked to update this one (T-122), with
    /// each change.
    Update(UpdateStatus),
    /// Spontaneous notice when any open tunnel changes (T-103): grouped by project,
    /// view only (D9).
    Tunnels(Vec<ProjectTunnels>),
    /// The metrics of each path, to the connections that asked for
    /// [`UiCommand::WatchNetwork`] (T-102).
    Network(Box<NetworkStatus>),
    /// The cursor, at 30 Hz at most, to the connections that asked for
    /// [`UiCommand::WatchCursor`] (T-128).
    Cursor(CursorSample),
    /// The cursor crossed the edge between the two machines: `edge` is the edge of `from` it
    /// left through. To the same connections.
    Crossed {
        from: Machine,
        to: Machine,
        edge: Side,
    },
    /// Progress of a file transfer from this machine (T-106).
    Transfer(TransferProgress),
    /// Progress of a file drag between the machines (T-121).
    Drag(DragStatus),
}

/// Pairing as seen from the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairingStatus {
    /// Nobody asked to pair (or it was canceled).
    Idle,
    /// Looking for the other machine, which must also be pairing.
    Searching,
    /// The two machines found each other: the user compares `code` on both screens.
    Comparing {
        /// The 6 digits, with leading zeros.
        code: String,
        /// The name the other agent introduced itself with.
        peer: String,
        /// This machine's user already confirmed; the other's is missing.
        confirmed: bool,
    },
    /// Paired and saved in trust.
    Done { peer: String },
    /// Not paired; nothing was saved.
    Failed { reason: String },
    /// Nobody asked to pair, but the paired peer has not answered for a while and another
    /// machine announces itself for pairing on the LAN (T-345): the user may forget the old
    /// pair and pair with it. It replaces `Idle` and goes back to it when the offer is gone.
    Offered,
}

/// The shared keyboard and mouse, seen from this agent (the UI chip).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputStatus {
    /// The machine that has the keyboard and mouse.
    pub server: Machine,
    /// Whether the hub is on this machine; `None` without detection (no `[input.hub]` or
    /// no scanner on this system).
    pub hub_here: Option<bool>,
    /// The input session with the other agent is established.
    pub session: bool,
    /// The cursor is locked on the current screen (server shortcut).
    pub locked: bool,
    /// System permissions missing to capture or inject (macOS). Empty if
    /// none is missing.
    pub missing: Vec<InputPermission>,
    /// Why this machine's input engine does not work, if it does not.
    pub problem: Option<String>,
    /// It is known where the keyboard is: the hub says so, or the configuration, or setup.
    /// If not, `server` is only the value the engine starts with and the UI says "not
    /// set": the Mac is never assumed (T-087).
    pub defined: bool,
    /// Input latency (capture -> injection) of the last 5 s window, measured
    /// by the one receiving the input (T-093). `None` until the first measurement.
    #[serde(default)]
    pub latency: Option<InputLatency>,
    /// The screens the input engine reads on this machine, in their native
    /// coordinates (T-150): the UI draws them when the desk does not declare `[[screens]]`.
    #[serde(default)]
    pub screens: Vec<ScreenRect>,
    /// The ones the other machine announced (`ControlMsg::Screens`); empty if it has said nothing.
    #[serde(default)]
    pub peer_screens: Vec<ScreenRect>,
}

/// Input latency over a window: from the server's timestamp (converted to the
/// client's clock with the offset estimated from the heartbeats) to injection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputLatency {
    /// The link it was measured over; `None` if the one measuring does not know.
    pub link: Option<Link>,
    /// Events measured in the window.
    pub count: u32,
    pub p50_us: u32,
    pub p99_us: u32,
    pub max_us: u32,
}

impl InputStatus {
    /// Before knowing anything: the server the agent starts with.
    pub fn new(server: Machine) -> Self {
        InputStatus {
            server,
            hub_here: None,
            session: false,
            locked: false,
            missing: Vec::new(),
            problem: None,
            defined: false,
            latency: None,
            screens: Vec::new(),
            peer_screens: Vec::new(),
        }
    }
}

/// The "press a key" test as seen from the UI (T-087).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyProbeStatus {
    /// Both machines are listening.
    pub armed: bool,
    /// The machine that received the key, if one has arrived.
    pub heard: Option<Machine>,
    /// The other machine, when it was heard after `heard` (T-220).
    pub second: Option<Machine>,
}

/// macOS permissions (TCC) the input engine needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputPermission {
    /// Accessibility: capture swallowing events and inject.
    Accessibility,
    /// Input monitoring: watching the keyboard.
    InputMonitoring,
}

/// The other agent as seen from this one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerStatus {
    /// No paired peer: the agent works alone, with what this machine can do.
    Unpaired,
    Paired {
        /// The name it was saved with in trust.
        name: String,
        /// The link requests go over; `None` if none sees the other agent.
        link: Option<Link>,
    },
}

/// The links between the two machines, from highest to lowest priority (DESIGN §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Link {
    Usb4,
    Ethernet,
    Wifi,
}

/// Power changes of the other machine that do not come from a transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PowerEvent {
    /// The other machine stopped answering on all links without anyone turning it off
    /// (a power cut, or it suspended): this agent considers it off.
    PeerLost {
        machine: Machine,
        /// Text to show in the UI.
        reason: String,
    },
}

// The desk (`Config`) travels by value in replies, as in the rest of the UI: the
// size difference does not matter in a message created on request.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    State(State),
    /// The change started as transaction `txn`; its progress arrives in [`AgentToUi::Txn`].
    Accepted {
        txn: u64,
    },
    Rejected(String),
    /// Request handled, nothing more to answer.
    Done,
    Desk(Config),
    Peer(PeerStatus),
    Pairing(PairingStatus),
    Input(InputStatus),
    Setup(SetupInfo),
    Capabilities(Box<DeskCapabilities>),
    /// How long a ping takes over the `link` link.
    Pong {
        link: Link,
        millis: u32,
    },
    /// The desk history, from the newest version to the oldest.
    DeskHistory(Vec<HistoryEntry>),
    Monitors(Vec<MonitorView>),
    /// The versions of both machines (T-122).
    Update(UpdateStatus),
    /// The tunnel that was opened.
    Tunnel {
        tunnel: Box<Tunnel>,
        /// What the workspace left out for looking like a credential (T-130).
        skipped: Vec<String>,
        /// Credentials that were already in the repo history and travel in the bundle (T-130).
        history_secrets: Vec<String>,
    },
    Tunnels(Vec<ProjectTunnels>),
    /// The result of [`UiCommand::MeasureLink`].
    Bandwidth {
        link: Link,
        bytes_per_sec: u64,
    },
    /// What [`UiCommand::FilesPut`] or [`UiCommand::FilesGet`] copied.
    Files(FilesReport),
    /// The port that is now being forwarded ([`UiCommand::PortForward`]).
    Forward(ForwardInfo),
    /// The process that was launched (T-104).
    Process(Box<TunnelProcess>),
    Processes(Vec<TunnelProcess>),
    Output(Box<ProcessOutput>),
    /// The result of [`UiCommand::BringBack`].
    BroughtBack(Box<BringBackReport>),
}

/// Progress of a transaction, for the UI to show each step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxnEvent {
    Started {
        txn: u64,
        #[serde(with = "wire::steps")]
        steps: Vec<Step>,
    },
    /// `index` is the position in `steps` of [`TxnEvent::Started`].
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
    /// Rollback: step `index`, already done, was undone because a later one failed.
    StepUndone {
        txn: u64,
        index: u32,
    },
    Committed {
        txn: u64,
        state: State,
    },
    /// A step failed: `state` has what did get done (power steps are not
    /// undone).
    Failed {
        txn: u64,
        state: State,
    },
}

#[cfg(test)]
pub(crate) mod samples {
    //! One example of each variant. The `arm` functions do an exhaustive `match`: a
    //! new variant does not compile until it has a number, and the coverage test fails
    //! until it has an example.

    use tandem_core::{Align, Arrangement, DeskStamp, Scale, ScreenSpec, Side};

    use super::*;

    pub fn state() -> State {
        State {
            on: tandem_core::Powered::both(),
            scene: Some("work".into()),
        }
    }

    /// A desk with declared screens of both machines.
    pub fn desk() -> Config {
        Config {
            machines: tandem_core::Machines {
                a: Some(MachineInfo::new(Machine::A, tandem_core::Os::MacOs, None)),
                b: Some(MachineInfo::new(
                    Machine::B,
                    tandem_core::Os::Windows,
                    Some("DESK".into()),
                )),
            },
            screens: vec![
                ScreenSpec {
                    machine: Machine::B,
                    id: "34".into(),
                    x: 0,
                    y: 0,
                    width: 3440,
                    height: 1440,
                    scale: Scale::ONE,
                },
                ScreenSpec {
                    machine: Machine::A,
                    id: "laptop".into(),
                    x: 0,
                    y: 0,
                    width: 1512,
                    height: 982,
                    scale: Scale::from_milli(2000),
                },
            ],
            arrangement: Some(Arrangement {
                a_side: Side::Up,
                align: Align::End,
            }),
            scenes: tandem_core::default_scenes(
                &tandem_core::Machines {
                    a: Some(MachineInfo::new(Machine::A, tandem_core::Os::MacOs, None)),
                    b: Some(MachineInfo::new(
                        Machine::B,
                        tandem_core::Os::Windows,
                        Some("DESK".into()),
                    )),
                },
                None,
            ),
            ..Config::default()
        }
    }

    pub fn steps() -> Vec<Step> {
        vec![
            Step::PowerOn {
                machine: Machine::B,
            },
            Step::PowerOff {
                machine: Machine::A,
            },
            Step::SwitchMonitor {
                monitor: "lc34g55t-0a1b2c3d".into(),
                input: MonitorInput::Hdmi,
            },
            Step::ShowMachine {
                monitor: "lc34g55t-0a1b2c3d".into(),
                machine: Machine::B,
            },
            Step::Arrange {
                layout: Some(Arrangement::default()),
            },
            Step::Geometry,
        ]
    }

    pub fn step_arm(s: &Step) -> usize {
        match s {
            Step::PowerOn { .. } => 0,
            Step::PowerOff { .. } => 1,
            Step::SwitchMonitor { .. } => 2,
            Step::ShowMachine { .. } => 3,
            Step::Arrange { .. } => 4,
            Step::Geometry => 5,
        }
    }

    pub fn monitor_report() -> MonitorReport {
        MonitorReport {
            id: "lc34g55t-0a1b2c3d".into(),
            input: Some(MonitorInput::DisplayPort),
            power_on: Some(true),
            signal: None,
            events: vec![
                MonitorEvent {
                    at_ms: 1_763_000_000_000,
                    kind: MonitorEventKind::SwitchDone,
                    input: Some(MonitorInput::Hdmi),
                },
                MonitorEvent {
                    at_ms: 0,
                    kind: MonitorEventKind::Lost,
                    input: None,
                },
            ],
        }
    }

    pub fn monitor_views() -> Vec<MonitorView> {
        vec![
            MonitorView {
                id: "lc34g55t-0a1b2c3d".into(),
                name: "LC34G55T".into(),
                compat: MonitorCompat::Compatible { protocol: 2 },
                seen_by: vec![Machine::A, Machine::B],
                via: vec![
                    (Machine::A, MonitorInput::Hdmi),
                    (Machine::B, MonitorInput::DisplayPort),
                ],
                owner: Some(Machine::B),
                report: Some(monitor_report()),
            },
            MonitorView {
                id: "odyssey-0f0f0f0f".into(),
                name: "Odyssey G5".into(),
                compat: MonitorCompat::NotCompatible {
                    reason: "factory firmware".into(),
                },
                seen_by: vec![Machine::B],
                via: vec![],
                owner: None,
                report: None,
            },
        ]
    }

    pub fn monitor_event_kind_arm(k: &MonitorEventKind) -> usize {
        match k {
            MonitorEventKind::Boot => 0,
            MonitorEventKind::Input => 1,
            MonitorEventKind::SwitchRequested => 2,
            MonitorEventKind::SwitchDone => 3,
            MonitorEventKind::SwitchFailed => 4,
            MonitorEventKind::Power => 5,
            MonitorEventKind::Signal => 6,
            MonitorEventKind::Setting => 7,
            MonitorEventKind::Lost => 8,
            MonitorEventKind::Other => 9,
        }
    }

    pub fn monitor_event_kinds() -> Vec<MonitorEventKind> {
        use MonitorEventKind::*;
        vec![
            Boot,
            Input,
            SwitchRequested,
            SwitchDone,
            SwitchFailed,
            Power,
            Signal,
            Setting,
            Lost,
            Other,
        ]
    }

    pub fn hellos() -> Vec<Hello> {
        vec![
            Hello::new(Peer::Agent(Machine::A)),
            Hello::new(Peer::Agent(Machine::B)),
            Hello::new(Peer::Ui),
            Hello::new(Peer::Client(crate::ClientKind::Mcp)),
            Hello::new(Peer::Client(crate::ClientKind::Cli)),
        ]
    }

    pub fn control() -> Vec<ControlMsg> {
        let mut v = vec![
            ControlMsg::Ping { nonce: 0 },
            ControlMsg::Pong { nonce: u64::MAX },
            ControlMsg::StepResult {
                id: 7,
                outcome: StepOutcome::Done,
            },
            ControlMsg::StepResult {
                id: 8,
                outcome: StepOutcome::Failed("no Wake-on-LAN".into()),
            },
            ControlMsg::GetState,
            ControlMsg::State(state()),
            ControlMsg::Open(Channel::Input),
            ControlMsg::Open(Channel::Clipboard),
            ControlMsg::Open(Channel::Tunnel),
            ControlMsg::Open(Channel::Measure),
            ControlMsg::Hub(HubReport {
                hub: true,
                server: Machine::B,
            }),
            ControlMsg::Screens(vec![]),
            ControlMsg::Screens(screens()),
            ControlMsg::KeyProbe(KeyProbeMsg::Arm),
            ControlMsg::KeyProbe(KeyProbeMsg::Disarm),
            ControlMsg::KeyProbe(KeyProbeMsg::Heard),
            ControlMsg::KeyProbe(KeyProbeMsg::Armed),
            ControlMsg::Keyboard(Machine::B),
            ControlMsg::WatchResources { on: true },
            ControlMsg::WatchResources { on: false },
            ControlMsg::LinkClass { kind: Link::Usb4 },
            ControlMsg::LinkClass { kind: Link::Wifi },
            ControlMsg::Rescan,
            ControlMsg::Unpaired,
            ControlMsg::Monitors(vec![]),
            ControlMsg::Monitors(vec![monitor_report()]),
            ControlMsg::Update(UpdateMsg::Version("0.3.0".into())),
            ControlMsg::Update(UpdateMsg::Install {
                version: "0.3.1-rc.2".into(),
            }),
            ControlMsg::Focus(FocusMsg::Claim { epoch: 0 }),
            ControlMsg::Focus(FocusMsg::Grant { epoch: u64::MAX }),
            ControlMsg::Focus(FocusMsg::State {
                epoch: 3,
                server: Machine::A,
            }),
            ControlMsg::WatchCursor { on: true },
            ControlMsg::WatchCursor { on: false },
            ControlMsg::Drag(DragMsg::Begin {
                id: 1,
                names: vec!["a.txt".into(), "folder".into()],
            }),
            ControlMsg::Drag(DragMsg::Staged {
                id: 1,
                dir: Ok("/tmp/drag-1".into()),
            }),
            ControlMsg::Drag(DragMsg::Staged {
                id: u64::MAX,
                dir: Err("no space".into()),
            }),
            ControlMsg::Drag(DragMsg::Ready { id: 1 }),
            ControlMsg::Drag(DragMsg::Finished {
                id: 1,
                end: Ok(DragOutcome::Dropped),
            }),
            ControlMsg::Drag(DragMsg::Finished {
                id: 1,
                end: Ok(DragOutcome::Cancelled),
            }),
            ControlMsg::Drag(DragMsg::Finished {
                id: 1,
                end: Err("failed".into()),
            }),
            ControlMsg::Drag(DragMsg::Cancel { id: 1 }),
            ControlMsg::Crossed {
                from: Machine::A,
                to: Machine::B,
                edge: Side::Right,
            },
            ControlMsg::Desk(Box::new(DeskVersion {
                stamp: DeskStamp {
                    rev: 4,
                    at: 1_763_400_000_000,
                    by: Machine::B,
                },
                desk: desk(),
            })),
            ControlMsg::Desk(Box::new(DeskVersion {
                stamp: DeskStamp::fresh(Machine::A),
                desk: Config::default(),
            })),
        ];
        v.extend(
            capabilities()
                .into_iter()
                .map(|c| ControlMsg::Capabilities(Box::new(c))),
        );
        v.extend(
            crate::env::samples::resources()
                .into_iter()
                .map(|r| ControlMsg::Resources(Box::new(r))),
        );
        v.extend(
            crate::cursor::samples::samples()
                .into_iter()
                .map(ControlMsg::Cursor),
        );
        v.extend(
            steps()
                .into_iter()
                .enumerate()
                .map(|(i, step)| ControlMsg::ExecuteStep { id: i as u64, step }),
        );
        v
    }

    pub fn control_arm(m: &ControlMsg) -> usize {
        match m {
            ControlMsg::Ping { .. } => 0,
            ControlMsg::Pong { .. } => 1,
            ControlMsg::ExecuteStep { .. } => 2,
            ControlMsg::StepResult { .. } => 3,
            ControlMsg::GetState => 4,
            ControlMsg::State(_) => 5,
            ControlMsg::Open(_) => 6,
            ControlMsg::Hub(_) => 7,
            ControlMsg::Screens(_) => 8,
            ControlMsg::Capabilities(_) => 9,
            ControlMsg::Desk(_) => 10,
            ControlMsg::KeyProbe(_) => 11,
            ControlMsg::Keyboard(_) => 12,
            ControlMsg::Monitors(_) => 13,
            ControlMsg::WatchResources { .. } => 14,
            ControlMsg::Resources(_) => 15,
            ControlMsg::Rescan => 16,
            ControlMsg::Update(_) => 17,
            ControlMsg::Focus(_) => 18,
            ControlMsg::WatchCursor { .. } => 19,
            ControlMsg::Cursor(_) => 20,
            ControlMsg::Crossed { .. } => 21,
            ControlMsg::LinkClass { .. } => 22,
            ControlMsg::Drag(_) => 23,
            ControlMsg::Unpaired => 24,
        }
    }

    /// The two machines of the real desk.
    pub fn capabilities() -> Vec<Capabilities> {
        vec![
            Capabilities {
                machine: Machine::B,
                info: MachineInfo {
                    os: tandem_core::Os::Windows,
                    name: "DESK".into(),
                    color: "#a99be8".into(),
                },
                hub: Some(true),
                input: InputCaps {
                    capture: true,
                    inject: true,
                    missing: vec![],
                    process: ProcessInfo::default(),
                },
                power: PowerCaps {
                    wake_peer: false,
                    wakeable: true,
                    power_off: true,
                    mac: Some("aa:bb:cc:dd:ee:01".into()),
                    wol_mac: None,
                },
                services: vec!["ssh".into()],
                modules: modules(),
                environments: crate::env::samples::environments(),
                hardware: crate::env::samples::hardware(),
                monitors: vec![
                    MonitorChannel {
                        id: "lc34g55t-0a1b2c3d".into(),
                        name: "LC34G55T".into(),
                        compat: MonitorCompat::Compatible { protocol: 2 },
                        latency_ms: 42,
                        via: Some(MonitorInput::DisplayPort),
                    },
                    MonitorChannel {
                        id: "odyssey-0f0f0f0f".into(),
                        name: "Odyssey G5".into(),
                        compat: MonitorCompat::NotCompatible {
                            reason: "factory firmware".into(),
                        },
                        latency_ms: 0,
                        via: None,
                    },
                ],
            },
            Capabilities {
                machine: Machine::A,
                info: MachineInfo {
                    os: tandem_core::Os::MacOs,
                    name: "MacBook Pro".into(),
                    color: "#7aa7e6".into(),
                },
                hub: None,
                input: InputCaps {
                    capture: false,
                    inject: false,
                    missing: vec![
                        InputPermission::Accessibility,
                        InputPermission::InputMonitoring,
                    ],
                    process: ProcessInfo {
                        exe: "/Applications/Tandem.app/Contents/MacOS/tandemd".into(),
                        pid: 4242,
                        started: 1_760_000_000,
                    },
                },
                power: PowerCaps {
                    wake_peer: true,
                    wakeable: false,
                    power_off: false,
                    mac: None,
                    wol_mac: Some("aa:bb:cc:dd:ee:02".into()),
                },
                services: vec![],
                modules: vec![],
                environments: vec![],
                hardware: Hardware::default(),
                monitors: vec![],
            },
        ]
    }

    /// A module in each state.
    pub fn modules() -> Vec<ModuleInfo> {
        vec![
            ModuleInfo {
                id: ModuleId::Input,
                name: "Keyboard and mouse".into(),
                detail: "Capture and injection".into(),
                state: ModuleState::Active,
                switchable: true,
            },
            ModuleInfo {
                id: ModuleId::Clipboard,
                name: "Clipboard".into(),
                detail: "Text, HTML and image".into(),
                state: ModuleState::Off,
                switchable: true,
            },
            ModuleInfo {
                id: ModuleId::Monitors,
                name: "Monitors".into(),
                detail: String::new(),
                state: ModuleState::Unavailable {
                    reason: "No DDC channel on this machine".into(),
                },
                switchable: false,
            },
        ]
    }

    pub fn module_state_arm(s: &ModuleState) -> usize {
        match s {
            ModuleState::Active => 0,
            ModuleState::Off => 1,
            ModuleState::Unavailable { .. } => 2,
        }
    }

    pub fn pairing() -> Vec<PairingMsg> {
        vec![
            PairingMsg::Offer {
                public_key: [1; 32],
                name: "Mac".into(),
                commitment: [2; 32],
            },
            PairingMsg::Answer {
                public_key: [3; 32],
                name: "PC".into(),
                nonce: [4; 32],
            },
            PairingMsg::Reveal { nonce: [5; 32] },
            PairingMsg::Confirm { code: 42 },
            PairingMsg::Reject,
        ]
    }

    pub fn pairing_arm(m: &PairingMsg) -> usize {
        match m {
            PairingMsg::Offer { .. } => 0,
            PairingMsg::Answer { .. } => 1,
            PairingMsg::Reveal { .. } => 2,
            PairingMsg::Confirm { .. } => 3,
            PairingMsg::Reject => 4,
        }
    }

    pub fn pairing_statuses() -> Vec<PairingStatus> {
        vec![
            PairingStatus::Idle,
            PairingStatus::Searching,
            PairingStatus::Comparing {
                code: "004217".into(),
                peer: "PC".into(),
                confirmed: false,
            },
            PairingStatus::Done { peer: "PC".into() },
            PairingStatus::Failed {
                reason: "the other machine rejected the code".into(),
            },
            PairingStatus::Offered,
        ]
    }

    pub fn screens() -> Vec<ScreenRect> {
        vec![
            ScreenRect {
                id: "pc-1".into(),
                x: -3440,
                y: 0,
                width: 3440,
                height: 1440,
                scale_milli: 1000,
            },
            ScreenRect {
                id: "laptop".into(),
                x: 0,
                y: 0,
                width: 1512,
                height: 982,
                scale_milli: 2000,
            },
        ]
    }

    pub fn pairing_status_arm(s: &PairingStatus) -> usize {
        match s {
            PairingStatus::Idle => 0,
            PairingStatus::Searching => 1,
            PairingStatus::Comparing { .. } => 2,
            PairingStatus::Done { .. } => 3,
            PairingStatus::Failed { .. } => 4,
            PairingStatus::Offered => 5,
        }
    }

    pub fn input_status() -> Vec<InputStatus> {
        vec![
            InputStatus::new(Machine::A),
            InputStatus {
                server: Machine::B,
                hub_here: Some(true),
                session: true,
                locked: true,
                missing: vec![
                    InputPermission::Accessibility,
                    InputPermission::InputMonitoring,
                ],
                problem: Some("the Accessibility permission is missing".into()),
                defined: true,
                latency: Some(InputLatency {
                    link: Some(Link::Wifi),
                    count: 5000,
                    p50_us: 900,
                    p99_us: 3_200,
                    max_us: 12_000,
                }),
                screens: screens(),
                peer_screens: screens(),
            },
        ]
    }

    pub fn ui_requests() -> Vec<UiRequest> {
        let cmds = vec![
            UiCommand::GetState,
            UiCommand::GetDesk,
            UiCommand::SetPower {
                machine: Machine::B,
                on: true,
            },
            UiCommand::SetPower {
                machine: Machine::A,
                on: false,
            },
            UiCommand::GetPeer,
            UiCommand::GetPairing,
            UiCommand::StartPairing,
            UiCommand::ConfirmPairing,
            UiCommand::CancelPairing,
            UiCommand::Unpair,
            UiCommand::GetInput,
            UiCommand::RequestPermissions,
            UiCommand::GetSetup,
            UiCommand::GetCapabilities,
            UiCommand::KeyProbe { on: true },
            UiCommand::KeyProbe { on: false },
            UiCommand::HeardKey,
            UiCommand::SetKeyboard(Machine::B),
            UiCommand::PingPeer,
            UiCommand::SetModule {
                machine: Machine::B,
                module: ModuleId::SecureDesktop,
                on: true,
            },
            UiCommand::UndoDesk { to_rev: 3 },
            UiCommand::GetDeskHistory,
            UiCommand::GetMonitors,
            UiCommand::SwitchMonitor {
                monitor: "lc34g55t-0a1b2c3d".into(),
                input: MonitorInput::DisplayPort,
            },
            UiCommand::Rescan {
                machine: Machine::A,
            },
            UiCommand::WatchResources { on: true },
            UiCommand::WatchResources { on: false },
            UiCommand::GetUpdate,
            UiCommand::UpdatePeer {
                version: "0.3.1".into(),
            },
            UiCommand::ApplyScene { id: "work".into() },
            UiCommand::OpenTunnel {
                title: "Run the tests".into(),
                project: tunnel_sample().project,
                target: Machine::B,
                session: "claude-1234".into(),
                workspace: Some(WorkspaceSpec {
                    repo_path: "/src/tandem".into(),
                    keep: true,
                    include: vec![".env.deploy".into()],
                }),
            },
            UiCommand::OpenTunnel {
                title: String::new(),
                project: None,
                target: Machine::A,
                session: String::new(),
                workspace: None,
            },
            UiCommand::CloseTunnel {
                id: crate::TunnelId(u64::MAX),
            },
            UiCommand::GetTunnels,
            UiCommand::WatchNetwork { on: true },
            UiCommand::MeasureLink { path: Link::Wifi },
            UiCommand::WatchCursor { on: true },
            UiCommand::WatchCursor { on: false },
            UiCommand::LockHere { on: true },
            UiCommand::LockHere { on: false },
            UiCommand::SetWol {
                mac: Some("aa:bb:cc:dd:ee:ff".into()),
            },
            UiCommand::SetWol { mac: None },
            UiCommand::SyntheticMotion { dx: -6, dy: 0 },
            UiCommand::SyntheticMotion {
                dx: i32::MIN,
                dy: i32::MAX,
            },
            UiCommand::FilesPut {
                tunnel: crate::TunnelId(1),
                local: "/home/u/build".into(),
                remote: "C:\\work\\build".into(),
                include: vec!["config/.env.local".into()],
            },
            UiCommand::FilesGet {
                tunnel: crate::TunnelId(1),
                remote: "C:\\work\\out.zip".into(),
                local: "/tmp/out.zip".into(),
            },
            UiCommand::PortForward {
                tunnel: crate::TunnelId(1),
                local_port: 8080,
                remote_host: "127.0.0.1".into(),
                remote_port: 3000,
                direction: Direction::LocalToRemote,
            },
            UiCommand::PortForward {
                tunnel: crate::TunnelId(1),
                local_port: 5432,
                remote_host: "0.0.0.0".into(),
                remote_port: 5432,
                direction: Direction::RemoteToLocal,
            },
            UiCommand::StopForward { id: 7 },
            UiCommand::Exec {
                tunnel: crate::TunnelId(1),
                spec: ExecSpec {
                    cmd: "cargo".into(),
                    args: vec!["test".into()],
                    shell: Some("zsh".into()),
                    cwd: Some("/src/tandem".into()),
                    env: vec![("RUST_LOG".into(), "info".into())],
                    allow_secret_env: true,
                },
            },
            UiCommand::Kill {
                tunnel: crate::TunnelId(1),
                process: ProcessId(2),
            },
            UiCommand::Output {
                tunnel: crate::TunnelId(1),
                process: ProcessId(2),
                from: OutputCursor {
                    stdout: 10,
                    stderr: u64::MAX,
                },
                max: 65536,
            },
            UiCommand::GetProcesses {
                tunnel: crate::TunnelId(1),
            },
            UiCommand::BringBack {
                tunnel: crate::TunnelId(1),
                commit: Some("work on the destination".into()),
                name: None,
            },
        ];
        let mut cmds = cmds;
        cmds.extend(
            desk_patches()
                .into_iter()
                .map(|patch| UiCommand::EditDesk { patch }),
        );
        cmds.into_iter()
            .enumerate()
            .map(|(i, command)| UiRequest {
                id: i as u64,
                command,
            })
            .collect()
    }

    pub fn ui_command_arm(c: &UiCommand) -> usize {
        match c {
            UiCommand::GetState => 0,
            UiCommand::GetDesk => 1,
            UiCommand::SetPower { .. } => 2,
            UiCommand::GetPeer => 3,
            UiCommand::GetPairing => 4,
            UiCommand::StartPairing => 5,
            UiCommand::ConfirmPairing => 6,
            UiCommand::CancelPairing => 7,
            UiCommand::Unpair => 8,
            UiCommand::GetInput => 9,
            UiCommand::RequestPermissions => 10,
            UiCommand::GetSetup => 11,
            UiCommand::GetCapabilities => 12,
            UiCommand::KeyProbe { .. } => 13,
            UiCommand::HeardKey => 14,
            UiCommand::SetKeyboard(_) => 15,
            UiCommand::PingPeer => 16,
            UiCommand::SetModule { .. } => 17,
            UiCommand::EditDesk { .. } => 18,
            UiCommand::UndoDesk { .. } => 19,
            UiCommand::GetDeskHistory => 20,
            UiCommand::GetMonitors => 21,
            UiCommand::SwitchMonitor { .. } => 22,
            UiCommand::Rescan { .. } => 23,
            UiCommand::WatchResources { .. } => 24,
            UiCommand::UpdatePeer { .. } => 25,
            UiCommand::GetUpdate => 26,
            UiCommand::ApplyScene { .. } => 27,
            UiCommand::OpenTunnel { .. } => 28,
            UiCommand::CloseTunnel { .. } => 29,
            UiCommand::GetTunnels => 30,
            UiCommand::WatchNetwork { .. } => 31,
            UiCommand::MeasureLink { .. } => 32,
            UiCommand::WatchCursor { .. } => 33,
            UiCommand::FilesPut { .. } => 34,
            UiCommand::FilesGet { .. } => 35,
            UiCommand::PortForward { .. } => 36,
            UiCommand::StopForward { .. } => 37,
            UiCommand::Exec { .. } => 38,
            UiCommand::Kill { .. } => 39,
            UiCommand::Output { .. } => 40,
            UiCommand::BringBack { .. } => 42,
            UiCommand::GetProcesses { .. } => 41,
            UiCommand::LockHere { .. } => 43,
            UiCommand::SetWol { .. } => 44,
            UiCommand::SyntheticMotion { .. } => 45,
        }
    }

    /// A patch for each section of the desk.
    pub fn desk_patches() -> Vec<DeskPatch> {
        vec![
            DeskPatch::Machine {
                side: Machine::B,
                name: Some("Tower".into()),
                color: Some("#5fbfb0".into()),
            },
            DeskPatch::Machine {
                side: Machine::A,
                name: None,
                color: None,
            },
            DeskPatch::Name("Office".into()),
            DeskPatch::Arrangement(None),
            DeskPatch::Arrangement(Some(Arrangement {
                a_side: Side::Down,
                align: Align::Start,
            })),
            DeskPatch::Screens {
                machine: Machine::A,
                screens: desk().screens,
            },
            DeskPatch::Scenes(desk().scenes),
            DeskPatch::AddScene(tandem_core::default_scenes(&desk().machines, None).remove(2)),
            DeskPatch::EditScene(tandem_core::Scene {
                monitors: vec![("lc34g55t-0a1b2c3d".into(), Machine::A)],
                require_on: Some(Machine::A),
                ..tandem_core::Scene::new("work", "Work")
            }),
            DeskPatch::DeleteScene { id: "x".into() },
            DeskPatch::Shortcuts(tandem_core::Shortcuts { lock: vec![] }),
            DeskPatch::Prefs(tandem_core::InputPrefs {
                cmd_ctrl: false,
                drag_files: true,
            }),
            DeskPatch::Module {
                machine: Machine::B,
                module: "clipboard".into(),
                on: false,
            },
        ]
    }

    pub fn txn_events() -> Vec<TxnEvent> {
        vec![
            TxnEvent::Started {
                txn: 1,
                steps: steps(),
            },
            TxnEvent::Started {
                txn: 2,
                steps: vec![],
            },
            TxnEvent::StepStarted { txn: 1, index: 0 },
            TxnEvent::StepDone { txn: 1, index: 0 },
            TxnEvent::StepFailed {
                txn: 1,
                index: 1,
                error: "timeout".into(),
            },
            TxnEvent::StepUndone { txn: 1, index: 0 },
            TxnEvent::Committed {
                txn: 1,
                state: state(),
            },
            TxnEvent::Failed {
                txn: 1,
                state: state(),
            },
        ]
    }

    pub fn txn_arm(e: &TxnEvent) -> usize {
        match e {
            TxnEvent::Started { .. } => 0,
            TxnEvent::StepStarted { .. } => 1,
            TxnEvent::StepDone { .. } => 2,
            TxnEvent::StepFailed { .. } => 3,
            TxnEvent::Committed { .. } => 4,
            TxnEvent::Failed { .. } => 5,
            TxnEvent::StepUndone { .. } => 6,
        }
    }

    pub fn to_ui() -> Vec<AgentToUi> {
        let mut v = vec![
            AgentToUi::Reply {
                id: 1,
                reply: Reply::State(state()),
            },
            AgentToUi::Reply {
                id: 2,
                reply: Reply::Accepted { txn: 9 },
            },
            AgentToUi::Reply {
                id: 3,
                reply: Reply::Rejected("the PC is not responding".into()),
            },
            AgentToUi::Reply {
                id: 4,
                reply: Reply::Done,
            },
            AgentToUi::Reply {
                id: 5,
                reply: Reply::Desk(desk()),
            },
            AgentToUi::Reply {
                id: 6,
                reply: Reply::Peer(PeerStatus::Unpaired),
            },
            AgentToUi::Peer(PeerStatus::Paired {
                name: "PC".into(),
                link: None,
            }),
            AgentToUi::State(state()),
            AgentToUi::Power(PowerEvent::PeerLost {
                machine: Machine::B,
                reason: "the PC is not responding".into(),
            }),
            AgentToUi::Power(PowerEvent::PeerLost {
                machine: Machine::A,
                reason: String::new(),
            }),
            AgentToUi::Reply {
                id: 7,
                reply: Reply::Input(InputStatus::new(Machine::B)),
            },
        ];
        v.extend([
            AgentToUi::Drag(DragStatus::Copying {
                name: "a.txt".into(),
                index: 0,
                total: 2,
            }),
            AgentToUi::Drag(DragStatus::Copied { total: 2 }),
            AgentToUi::Drag(DragStatus::Dropped),
            AgentToUi::Drag(DragStatus::Cancelled),
            AgentToUi::Drag(DragStatus::Failed("no space".into())),
        ]);
        v.push(AgentToUi::Reply {
            id: 7,
            reply: Reply::Pairing(PairingStatus::Searching),
        });
        v.extend(pairing_statuses().into_iter().map(AgentToUi::Pairing));
        v.extend(input_status().into_iter().map(AgentToUi::Input));
        v.extend(txn_events().into_iter().map(AgentToUi::Txn));
        v.push(AgentToUi::Reply {
            id: 9,
            reply: Reply::Setup(SetupInfo {
                config_file: Some(r"C:\Users\Ana\AppData\Roaming\Tandem\tandem.toml".into()),
                generated: true,
                port: Some(52100),
            }),
        });
        v.push(AgentToUi::Reply {
            id: 10,
            reply: Reply::Setup(SetupInfo {
                config_file: None,
                generated: false,
                port: None,
            }),
        });
        v.push(AgentToUi::Desk(desk()));
        v.push(AgentToUi::DeskChanged {
            by: Machine::B,
            rev: 7,
            summary: "Arrangement and shortcuts".into(),
            undo_to: 6,
        });
        v.push(AgentToUi::Reply {
            id: 14,
            reply: Reply::DeskHistory(vec![
                HistoryEntry {
                    stamp: DeskStamp {
                        rev: 7,
                        at: 9,
                        by: Machine::B,
                    },
                    summary: "Arrangement".into(),
                },
                HistoryEntry {
                    stamp: DeskStamp::fresh(Machine::A),
                    summary: String::new(),
                },
            ]),
        });
        v.push(AgentToUi::KeyProbe(KeyProbeStatus::default()));
        v.push(AgentToUi::KeyProbe(KeyProbeStatus {
            armed: true,
            heard: Some(Machine::B),
            second: Some(Machine::A),
        }));
        v.push(AgentToUi::Reply {
            id: 13,
            reply: Reply::Pong {
                link: Link::Wifi,
                millis: 7,
            },
        });
        let [pc, mac] = <[Capabilities; 2]>::try_from(capabilities()).unwrap();
        v.push(AgentToUi::Machines {
            local: Box::new(mac.clone()),
            remote: Some(Box::new(pc)),
        });
        v.push(AgentToUi::Machines {
            local: Box::new(mac.clone()),
            remote: None,
        });
        v.push(AgentToUi::Reply {
            id: 11,
            reply: Reply::Capabilities(Box::new(DeskCapabilities {
                here: mac,
                peer: None,
            })),
        });
        v.push(AgentToUi::Monitors(vec![]));
        v.push(AgentToUi::Tunnels(vec![]));
        v.push(AgentToUi::Tunnels(crate::group_by_project([
            tunnel_sample(),
        ])));
        v.push(AgentToUi::Reply {
            id: 13,
            reply: Reply::Tunnel {
                tunnel: Box::new(tunnel_sample()),
                skipped: vec!["deploy/id_rsa".into()],
                history_secrets: vec![".env".into()],
            },
        });
        v.push(AgentToUi::Reply {
            id: 14,
            reply: Reply::Tunnels(crate::group_by_project([tunnel_sample()])),
        });
        v.push(AgentToUi::Tunnels(crate::group_by_project([
            crate::TunnelView {
                tunnel: tunnel_sample(),
                running: vec![process_sample()],
            },
        ])));
        v.push(AgentToUi::Reply {
            id: 15,
            reply: Reply::Process(Box::new(process_sample())),
        });
        v.push(AgentToUi::Reply {
            id: 16,
            reply: Reply::Processes(vec![process_sample()]),
        });
        v.push(AgentToUi::Reply {
            id: 17,
            reply: Reply::Output(Box::new(ProcessOutput {
                process: process_sample(),
                stdout: b"ok\n".to_vec(),
                stderr: vec![0xff, b'\n'],
                from: OutputCursor::default(),
                next: OutputCursor {
                    stdout: 3,
                    stderr: 2,
                },
                done: true,
            })),
        });
        v.push(AgentToUi::Reply {
            id: 18,
            reply: Reply::BroughtBack(Box::new(BringBackReport {
                branch: Some("tandem/fix".into()),
                commits: vec![crate::CommitInfo {
                    id: "abc123".into(),
                    summary: "Fix the bug".into(),
                    author: "Claude".into(),
                }],
                committed_pending: true,
            })),
        });
        v.push(AgentToUi::Monitors(monitor_views()));
        v.push(AgentToUi::Reply {
            id: 12,
            reply: Reply::Monitors(monitor_views()),
        });
        v.extend(
            crate::env::samples::resources()
                .into_iter()
                .map(|r| AgentToUi::Resources {
                    machine: Machine::B,
                    resources: Box::new(r),
                }),
        );
        v.push(AgentToUi::Network(Box::new(crate::net::samples::status())));
        v.push(AgentToUi::Network(Box::default()));
        v.extend(
            crate::cursor::samples::samples()
                .into_iter()
                .map(AgentToUi::Cursor),
        );
        v.push(AgentToUi::Crossed {
            from: Machine::B,
            to: Machine::A,
            edge: Side::Left,
        });
        v.push(AgentToUi::Transfer(TransferProgress {
            tunnel: crate::TunnelId(1),
            upload: true,
            path: "/home/u/build".into(),
            done: 1 << 20,
            total: 5 << 20,
            finished: false,
        }));
        v.push(AgentToUi::Reply {
            id: 15,
            reply: Reply::Files(FilesReport {
                files: 3,
                dirs: 1,
                bytes: 10,
                sent: 4,
                skipped: vec![".env".into(), ".ssh/".into()],
            }),
        });
        v.push(AgentToUi::Reply {
            id: 16,
            reply: Reply::Forward(ForwardInfo {
                id: 7,
                tunnel: crate::TunnelId(1),
                direction: Direction::LocalToRemote,
                local_port: 8080,
                remote_host: "127.0.0.1".into(),
                remote_port: 3000,
            }),
        });
        v.push(AgentToUi::Reply {
            id: 14,
            reply: Reply::Bandwidth {
                link: Link::Wifi,
                bytes_per_sec: 51_250_000,
            },
        });
        v.extend(links().into_iter().map(|link| {
            AgentToUi::Peer(PeerStatus::Paired {
                name: "Mac".into(),
                link: Some(link),
            })
        }));
        v.push(AgentToUi::Update(UpdateStatus::default()));
        v.push(AgentToUi::Reply {
            id: 14,
            reply: Reply::Update(UpdateStatus {
                here: "0.3.0".into(),
                peer: None,
                install: None,
                peer_protocol: None,
            }),
        });
        v.push(AgentToUi::Update(UpdateStatus {
            here: "0.3.0".into(),
            peer: Some("0.3.1".into()),
            install: Some("0.3.1".into()),
            peer_protocol: Some(PROTOCOL_VERSION + 1),
        }));
        v
    }

    pub fn links() -> Vec<Link> {
        vec![Link::Usb4, Link::Ethernet, Link::Wifi]
    }

    pub fn link_arm(l: &Link) -> usize {
        match l {
            Link::Usb4 => 0,
            Link::Ethernet => 1,
            Link::Wifi => 2,
        }
    }

    pub fn to_ui_arm(m: &AgentToUi) -> usize {
        match m {
            AgentToUi::Reply { .. } => 0,
            AgentToUi::State(_) => 1,
            AgentToUi::Txn(_) => 2,
            AgentToUi::Power(_) => 3,
            AgentToUi::Peer(_) => 4,
            AgentToUi::Pairing(_) => 5,
            AgentToUi::Input(_) => 6,
            AgentToUi::Desk(_) => 7,
            AgentToUi::Machines { .. } => 8,
            AgentToUi::KeyProbe(_) => 9,
            AgentToUi::DeskChanged { .. } => 10,
            AgentToUi::Monitors(_) => 11,
            AgentToUi::Resources { .. } => 12,
            AgentToUi::Update(_) => 13,
            AgentToUi::Tunnels(_) => 14,
            AgentToUi::Network(_) => 15,
            AgentToUi::Cursor(_) => 16,
            AgentToUi::Crossed { .. } => 17,
            AgentToUi::Transfer(_) => 18,
            AgentToUi::Drag(_) => 19,
        }
    }

    pub fn reply_arm(r: &Reply) -> usize {
        match r {
            Reply::State(_) => 0,
            Reply::Accepted { .. } => 1,
            Reply::Rejected(_) => 2,
            Reply::Done => 3,
            Reply::Desk(_) => 4,
            Reply::Peer(_) => 5,
            Reply::Pairing(_) => 6,
            Reply::Input(_) => 7,
            Reply::Setup(_) => 8,
            Reply::Capabilities(_) => 9,
            Reply::Pong { .. } => 10,
            Reply::DeskHistory(_) => 11,
            Reply::Monitors(_) => 12,
            Reply::Update(_) => 13,
            Reply::Tunnel { .. } => 14,
            Reply::Tunnels(_) => 15,
            Reply::Bandwidth { .. } => 16,
            Reply::Files(_) => 17,
            Reply::Forward(_) => 18,
            Reply::Process(_) => 19,
            Reply::Processes(_) => 20,
            Reply::Output(_) => 21,
            Reply::BroughtBack(_) => 22,
        }
    }

    pub fn peer_arm(p: &Peer) -> usize {
        match p {
            Peer::Agent(Machine::A) => 0,
            Peer::Agent(Machine::B) => 1,
            Peer::Ui => 2,
            Peer::Client(_) => 3,
        }
    }

    pub fn process_sample() -> TunnelProcess {
        TunnelProcess {
            id: ProcessId(1),
            tunnel: tunnel_sample().id,
            command: "cargo test".into(),
            shell: Some("wsl:Ubuntu-24.04".into()),
            cwd: Some("/src/tandem".into()),
            pid: Some(4242),
            stdout: crate::FlowId(1),
            stderr: crate::FlowId(2),
            started: 1_763_400_000_000,
            duration: None,
            state: crate::ProcessState::Running,
        }
    }

    pub fn tunnel_sample() -> crate::Tunnel {
        crate::Tunnel {
            id: crate::TunnelId(0x1234_5678_9abc_def0),
            title: "Run the tests on the Mac".into(),
            project: Some(crate::Project {
                name: "tandem".into(),
                root: "/home/u/projects/tandem".into(),
            }),
            origin: Machine::B,
            target: Machine::A,
            opened_by: crate::Client {
                kind: crate::ClientKind::Mcp,
                session: "claude-1234".into(),
            },
            state: crate::TunnelState::Open,
            created: 1_763_400_000_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fmt::Debug;

    use serde::Serialize;
    use serde::de::DeserializeOwned;

    use super::samples::*;
    use super::*;
    use crate::{decode, encode};

    fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(msgs: &[T]) {
        for m in msgs {
            let bytes = encode(m).unwrap();
            assert_eq!(&decode::<T>(&bytes).unwrap(), m);
        }
    }

    /// The samples cover the `n` variants of an enum.
    fn covers<T>(samples: &[T], arm: impl Fn(&T) -> usize, n: usize) {
        let seen: BTreeSet<usize> = samples.iter().map(arm).collect();
        assert_eq!(seen, (0..n).collect(), "samples missing for some variant");
    }

    #[test]
    fn samples_cover_every_variant() {
        covers(&steps(), step_arm, 6);
        covers(&hellos(), |h| peer_arm(&h.peer), 4);
        covers(&control(), control_arm, 25);
        let cmds: Vec<_> = ui_requests().into_iter().map(|r| r.command).collect();
        covers(&cmds, ui_command_arm, 46);
        covers(&modules(), |m| module_state_arm(&m.state), 3);
        covers(&to_ui(), to_ui_arm, 20);
        covers(&monitor_event_kinds(), monitor_event_kind_arm, 10);
        let kinds: Vec<_> = capabilities()
            .iter()
            .flat_map(|c| c.environments.iter().map(|e| e.kind))
            .collect();
        covers(&kinds, |k| crate::env::samples::env_kind_arm(*k), 3);
        covers(&pairing(), pairing_arm, 5);
        covers(&pairing_statuses(), pairing_status_arm, 6);
        covers(&links(), link_arm, 3);
        let replies: Vec<_> = to_ui()
            .into_iter()
            .filter_map(|m| match m {
                AgentToUi::Reply { reply, .. } => Some(reply),
                _ => None,
            })
            .collect();
        covers(&replies, reply_arm, 23);
        covers(&txn_events(), txn_arm, 7);
    }

    #[test]
    fn module_ids_have_a_stable_text() {
        for m in ModuleId::ALL {
            assert_eq!(ModuleId::parse(m.as_str()), Some(m));
        }
        assert_eq!(ModuleId::Clipboard.as_str(), "clipboard");
        assert_eq!(ModuleId::parse("Clipboard"), None);
    }

    #[test]
    fn hello_round_trip() {
        round_trip(&hellos());
    }

    #[test]
    fn control_round_trip() {
        round_trip(&control());
    }

    #[test]
    fn pairing_round_trip() {
        round_trip(&pairing());
    }

    #[test]
    fn ui_request_round_trip() {
        round_trip(&ui_requests());
    }

    #[test]
    fn agent_to_ui_round_trip() {
        round_trip(&to_ui());
    }

    #[test]
    fn every_monitor_event_kind_round_trips() {
        round_trip(&monitor_event_kinds());
    }

    #[test]
    fn step_outcome_converts_from_and_to_result() {
        assert_eq!(StepOutcome::from(Ok(())), StepOutcome::Done);
        let failed = StepOutcome::from(Err::<(), _>("x".to_owned()));
        assert_eq!(Result::<(), String>::from(failed), Err("x".to_owned()));
    }

    #[test]
    fn current_hello_passes_check() {
        assert_eq!(Hello::new(Peer::Ui).check_version(), Ok(()));
    }

    /// An agent from before sides A/B (v15, with `Peer::Agent(Mac)`) is cut off at the
    /// greeting, with a message saying what to do.
    #[test]
    fn an_agent_from_before_the_sides_is_rejected_in_the_hello() {
        let old = crate::encode(&(15u16, 0u8, 0u8)).unwrap();
        let e = crate::decode_hello(&old).unwrap_err();
        assert_eq!(
            e,
            ProtoError::IncompatibleVersion {
                ours: PROTOCOL_VERSION,
                theirs: 15,
            }
        );
        let text = e.to_string();
        assert!(text.contains("sides A/B"), "{text}");
        assert!(text.contains("the same Tandem version"), "{text}");
    }

    #[test]
    fn other_version_fails_check() {
        let h = Hello {
            version: PROTOCOL_VERSION + 1,
            peer: Peer::Ui,
        };
        assert_eq!(
            h.check_version(),
            Err(ProtoError::IncompatibleVersion {
                ours: PROTOCOL_VERSION,
                theirs: PROTOCOL_VERSION + 1,
            })
        );
    }
}
