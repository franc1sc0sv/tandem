//! Versioned messages between agents and the UI.
//!
//! - [`msg`]: the messages: opening [`Hello`], control between agents ([`ControlMsg`]),
//!   pairing ([`PairingMsg`]) and the UI's local API ([`UiRequest`], [`AgentToUi`]).
//! - [`frame`]: `postcard` encoding and length framing (u32 LE).
//!
//! Every connection starts with a [`Hello`] in each direction, read with
//! [`decode_hello`]: if the version does not match it is cut off there with
//! [`ProtoError::IncompatibleVersion`], before interpreting anything else.
//!
//! The input messages (keyboard and mouse) are defined by the input session, in
//! `tandem_input::session::msg`, with this crate's encoding.

pub mod cursor;
pub mod env;
pub mod files;
pub mod frame;
pub mod msg;
pub mod net;
pub mod tunnel;
mod wire;
pub mod workspace;

pub use cursor::CursorSample;
pub use env::{
    CpuLoad, DiskSpace, EnvKind, Environment, Gpu, GpuLoad, Hardware, MemoryUse, Resources, Tool,
};
pub use files::{
    CHUNK_SIZE, Direction, EntryKind, FileEntry, FilesDown, FilesReport, FilesUp, ForwardInfo,
    Hash, MAX_ENTRIES, Manifest, Need, PAGE_BUDGET, PIECE, PortDown, PortUp, TransferProgress,
    entry_cost, entry_fits, page_entries, page_needs,
};
pub use frame::{FrameDecoder, MAX_FRAME_LEN, decode, decode_hello, encode, encode_frame};
pub use msg::{
    AgentToUi, Capabilities, Channel, ControlMsg, DeskCapabilities, DragMsg, DragOutcome,
    DragStatus, FocusMsg, Hello, HubReport, InputCaps, InputLatency, InputPermission, InputStatus,
    KeyProbeMsg, KeyProbeStatus, Link, ModuleId, ModuleInfo, ModuleState, MonitorChannel,
    MonitorCompat, MonitorEvent, MonitorEventKind, MonitorReport, MonitorView, PairingMsg,
    PairingStatus, Peer, PeerStatus, PowerCaps, PowerEvent, ProcessInfo, Reply, ScreenRect,
    SetupInfo, StepOutcome, TxnEvent, UiCommand, UiRequest, UpdateMsg, UpdateStatus,
};
pub use net::{
    Bandwidth, LatencySample, MeasureReport, NetworkNotice, NetworkStatus, PathMetrics, PathState,
    PathSwitch,
};
pub use tunnel::{
    Client, ClientKind, ExecSpec, FlowId, FlowInfo, OutputCursor, ProcessId, ProcessOutput,
    ProcessState, Project, ProjectTunnels, Tunnel, TunnelDown, TunnelId, TunnelProcess,
    TunnelState, TunnelUp, TunnelView, group_by_project,
};
pub use workspace::{BringBackReport, CommitInfo, WorkspaceDown, WorkspaceSpec, WorkspaceUp};

/// Protocol version. Bumped on any incompatible change to the messages.
///
/// Only the same version talks: the two agents and the UI ship together.
pub const PROTOCOL_VERSION: u16 = 43;

/// Since this version each agent announces its capabilities on connecting (T-084): an
/// older one does not send them and cannot be spoken to as a peer.
pub const CAPABILITIES_SINCE: u16 = 10;

/// Since this version the two sides are `A`/`B`, ordered by public key (T-097): an
/// older one still introduces itself as "Mac" or "PC" and it cannot be known which side it is.
pub const SIDES_SINCE: u16 = 16;

fn version_hint(theirs: u16) -> &'static str {
    if theirs < CAPABILITIES_SINCE {
        " (an older version, without a capabilities announcement): install the same Tandem version on both machines"
    } else if theirs < SIDES_SINCE {
        " (a version older than sides A/B, with fixed \"Mac\" and \"PC\"): install the same Tandem version on both machines"
    } else {
        ": install the same Tandem version on both machines"
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtoError {
    #[error("could not encode the message: {0}")]
    Encode(postcard::Error),
    #[error("malformed message: {0}")]
    Decode(postcard::Error),
    #[error("{0} bytes left over after the message")]
    TrailingBytes(usize),
    /// The connection is unusable: the stream can no longer be resynchronized.
    #[error("frame of {len} bytes, the maximum is {max}")]
    FrameTooLarge { len: usize, max: usize },
    #[error(
        "incompatible protocol version: ours is {ours}, the other side's is {theirs}{}",
        version_hint(*theirs)
    )]
    IncompatibleVersion { ours: u16, theirs: u16 },
}

#[cfg(test)]
mod tests {
    use super::PROTOCOL_VERSION;

    /// T-081 published v8, T-082 v9 and T-084 (the capabilities announcement) v10. T-085
    /// adds to the state what is not confirmed and the "Apply mode" and "what
    /// it shows" requests: another message shape, another version, so two different binaries
    /// are never accepted as compatible. T-087 (setup: physical monitors, keyboard by
    /// keypress) bumps to v12. T-089 (`ProbeInput` and `MarkManual` with `discard_codes`, which change
    /// the shape of the UI requests) bumps to v13. T-090 (agent process in the
    /// capabilities and permission request) bumps to v14. T-092 (monitors, probes
    /// and modes removed; screens with their own id and declared arrangement) bumps to v15.
    /// T-097 (sides A/B by public key, `MachineInfo` in the capabilities and in the desk)
    /// bumps to v16. T-098 (modules with state, environments and monitors in the capabilities;
    /// `SetModule` in the UI and between agents) bumps to v17. T-099 (the desk with scenes,
    /// shortcuts, preferences and modules, its dated stamp, patches, history and the change
    /// notice; `SetModule` no longer travels between agents: it goes in the desk) bumps to v18.
    /// T-110 (monitors: channel with compatibility and latency, owner reports,
    /// `SwitchMonitor`) bumps to v19. T-105 (environments with tools, hardware,
    /// live resources and rescan) bumps to v20. T-101 (`[data]`, the cap on data
    /// over Wi-Fi, in the desk and its patches) bumps to v21. T-122 (app versions and request
    /// to update the other) bumps to v22. T-100 (full scenes in the desk, the active
    /// scene in the state, scene steps, `StepUndone` and `ApplyScene`) bumps to v23.
    /// T-103 (tunnels: the `Tunnel` channel, the clients' greeting, their requests and the notice to
    /// the UI) bumps to v25. T-093 (timestamps and heartbeats with echo in the input,
    /// keyboard arbitration with no owner and latency in the input state) bumps to v26:
    /// it was going to be v24, but it landed after T-103. T-102 (network metrics, the
    /// bandwidth test channel) bumps to v28.
    /// T-111 (`[[cables]]` in the desk and its patch, each machine's input according to the EDID
    /// in the channel and each monitor's view) bumps to v29. T-128 (the live cursor:
    /// `WatchCursor`, `Cursor` and `Crossed` in the UI and between agents) bumps to v30. T-106
    /// (files and ports per tunnel, their requests and the transfer progress) bumps to
    /// v31. T-104 (processes per tunnel: `Exec`, `Kill`, `Output`, `GetProcesses` and the
    /// processes in the tunnels view) bumps to v32: it was going to be v29, but T-111, T-128 and
    /// T-106 landed earlier. T-107 bumps to v33. T-094 (`ControlMsg::LinkClass`, the
    /// classification of each connection by its interface) bumps to v34: it was going to be v33, but
    /// T-107 landed earlier.
    /// T-121 (file drag: `DragMsg` between agents and `DragStatus` to the UI) bumps to
    /// v35: it was going to be v33, but T-107 and T-094 landed earlier.
    /// T-129 (`LockHere`, "Lock here" from the UI) bumps to v36: it was going to be v35, but
    /// T-121 landed earlier.
    /// T-130 (credentials: `include` and what is omitted in files and workspaces, `allow_secret_env`
    /// in `exec`) bumps to v37: it was going to be v35, but T-121 and T-129 landed earlier.
    /// T-150 (`InputStatus` carries the screens of both machines, for the UI arrangement)
    /// bumps to v38.
    /// T-152 (`ControlMsg::Unpaired`, leaving the desk notifies the other machine) and T-159 (the MAC
    /// announced for Wake-on-LAN and `UiCommand::SetWol`) bump to v39: they were going to be v38, but T-150 landed earlier.
    /// T-239 (`UiCommand::SyntheticMotion`, motion fed to a test agent's input engine) bumps to v40.
    /// T-240 (`KeyProbeMsg::Armed`, the key probe's state is repeated when a link appears) bumps to v41.
    /// T-340 (`MonitorCompat::NoChannel`, a monitor seen only by its EDID) bumps to v42.
    /// T-345 (`PairingStatus::Offered`, another machine offers to pair while the paired peer is
    /// silent) bumps to v43.
    #[test]
    fn version_stays_above_the_one_from_t084() {
        assert_eq!(PROTOCOL_VERSION, 43);
        assert_eq!(super::CAPABILITIES_SINCE, 10);
        assert_eq!(super::SIDES_SINCE, 16);
    }
}
