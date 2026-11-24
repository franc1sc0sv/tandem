//! Claude's tunnels for the Tunnels screen (read-only, D9): what
//! `tandem_proto::ProjectTunnels`, `TunnelProcess` and `ProcessOutput` say, in the shape the
//! window reads.
//!
//! Tunnel ids are random 64-bit: JSON would pass them as a number and JavaScript loses
//! the upper bits, so they travel as text (the `Display` of `TunnelId`, 16 hex).

use serde::{Deserialize, Serialize};
use tandem_core::Machine;
use tandem_proto::{
    OutputCursor, ProcessId, ProcessOutput, ProcessState, ProjectTunnels, TransferProgress,
    TunnelId, TunnelProcess, TunnelState,
};

/// How many bytes of each flow the window asks for per query.
pub const OUTPUT_CHUNK: u32 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProcessStateView {
    Running,
    Exited { code: Option<i32> },
    Killed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcessItem {
    pub id: u32,
    pub command: String,
    pub pid: Option<u32>,
    /// Unix milliseconds, according to the destination machine's clock.
    pub started: u64,
    pub duration: Option<u64>,
    pub state: ProcessStateView,
}

impl From<TunnelProcess> for ProcessItem {
    fn from(p: TunnelProcess) -> Self {
        ProcessItem {
            id: p.id.0,
            command: p.command,
            pid: p.pid,
            started: p.started,
            duration: p.duration,
            state: match p.state {
                ProcessState::Running => ProcessStateView::Running,
                ProcessState::Exited { code } => ProcessStateView::Exited { code },
                ProcessState::Killed => ProcessStateView::Killed,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TunnelItem {
    pub id: String,
    pub title: String,
    pub origin: Machine,
    pub target: Machine,
    pub open: bool,
    /// Unix milliseconds, according to the origin's clock.
    pub created: u64,
    /// Only the processes that are running.
    pub running: Vec<ProcessItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectItem {
    /// `None`: tunnels open without a project.
    pub name: Option<String>,
    pub root: Option<String>,
    pub tunnels: Vec<TunnelItem>,
}

/// The tunnels of both machines, grouped by project (`agent-tunnels` event).
pub fn projects_of(groups: Vec<ProjectTunnels>) -> Vec<ProjectItem> {
    groups
        .into_iter()
        .map(|g| ProjectItem {
            name: g.project.as_ref().map(|p| p.name.clone()),
            root: g.project.map(|p| p.root),
            tunnels: g
                .tunnels
                .into_iter()
                .map(|t| TunnelItem {
                    id: t.tunnel.id.to_string(),
                    title: t.tunnel.title,
                    origin: t.tunnel.origin,
                    target: t.tunnel.target,
                    open: t.tunnel.state == TunnelState::Open,
                    created: t.tunnel.created,
                    running: t.running.into_iter().map(Into::into).collect(),
                })
                .collect(),
        })
        .collect()
}

/// The output of a process from a cursor, already as text (`Output` response).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputItem {
    pub process: ProcessItem,
    pub stdout: String,
    pub stderr: String,
    pub next_stdout: u64,
    pub next_stderr: u64,
    /// The process ended and everything was already delivered.
    pub done: bool,
}

impl From<ProcessOutput> for OutputItem {
    fn from(o: ProcessOutput) -> Self {
        OutputItem {
            process: o.process.into(),
            // A character split between two queries shows as "�": the output is for reading.
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            next_stdout: o.next.stdout,
            next_stderr: o.next.stderr,
            done: o.done,
        }
    }
}

/// New output of the watched process (`agent-output` event). `data` is the raw bytes in base64:
/// the terminal view decodes UTF-8 and escape sequences itself, also when a character or a
/// sequence is split between two pushes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiveOutputItem {
    pub tunnel: String,
    pub process: ProcessItem,
    /// Whether it runs in a terminal (then stdout carries everything).
    pub pty: bool,
    pub from_stdout: u64,
    pub from_stderr: u64,
    pub next_stdout: u64,
    pub next_stderr: u64,
    /// stdout followed by stderr.
    pub data: String,
    pub done: bool,
}

impl From<ProcessOutput> for LiveOutputItem {
    fn from(o: ProcessOutput) -> Self {
        let mut bytes = o.stdout;
        bytes.extend_from_slice(&o.stderr);
        LiveOutputItem {
            tunnel: o.process.tunnel.to_string(),
            pty: o.process.pty,
            process: o.process.into(),
            from_stdout: o.from.stdout,
            from_stderr: o.from.stderr,
            next_stdout: o.next.stdout,
            next_stderr: o.next.stderr,
            data: base64(&bytes),
            done: o.done,
        }
    }
}

/// Standard base64 with padding (what `atob` reads).
pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(c.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(c.get(2).copied().unwrap_or(0));
        let sym = |shift: u32| char::from(ABC[((n >> shift) & 63) as usize]);
        out.push(sym(18));
        out.push(sym(12));
        out.push(if c.len() > 1 { sym(6) } else { '=' });
        out.push(if c.len() > 2 { sym(0) } else { '=' });
    }
    out
}

/// The progress of a tunnel's file copy (`agent-transfer` event).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TransferItem {
    pub tunnel: String,
    pub upload: bool,
    pub path: String,
    pub done: u64,
    pub total: u64,
    pub finished: bool,
}

impl From<TransferProgress> for TransferItem {
    fn from(t: TransferProgress) -> Self {
        TransferItem {
            tunnel: t.tunnel.to_string(),
            upload: t.upload,
            path: t.path,
            done: t.done,
            total: t.total,
            finished: t.finished,
        }
    }
}

/// `TunnelId` from the text it travels to the window with.
pub fn parse_tunnel(s: &str) -> Result<TunnelId, String> {
    u64::from_str_radix(s, 16)
        .map(TunnelId)
        .map_err(|_| format!("\"{s}\" is not a tunnel id"))
}

/// Deserializes the tunnel id of an action.
pub fn tunnel_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<TunnelId, D::Error> {
    let s = String::deserialize(d)?;
    parse_tunnel(&s).map_err(serde::de::Error::custom)
}

/// From where to keep reading a process's output.
pub fn cursor(stdout: u64, stderr: u64) -> OutputCursor {
    OutputCursor { stdout, stderr }
}

pub fn process_id(id: u32) -> ProcessId {
    ProcessId(id)
}

#[cfg(test)]
mod tests {
    use tandem_proto::{Client, ClientKind, FlowId, Project, Tunnel, TunnelView, group_by_project};

    use super::*;

    fn tunnel(id: u64, project: Option<&str>, state: TunnelState) -> Tunnel {
        Tunnel {
            id: TunnelId(id),
            title: format!("session {id}"),
            project: project.map(|p| Project {
                name: p.into(),
                root: format!("/src/{p}"),
            }),
            origin: Machine::A,
            target: Machine::B,
            opened_by: Client {
                kind: ClientKind::Mcp,
                session: "s".into(),
            },
            state,
            created: 1_000 + id,
        }
    }

    fn process(id: u32, state: ProcessState) -> TunnelProcess {
        TunnelProcess {
            id: ProcessId(id),
            tunnel: TunnelId(1),
            command: "cargo test".into(),
            shell: None,
            cwd: None,
            pid: Some(77),
            stdout: FlowId(1),
            stderr: FlowId(2),
            started: 5,
            duration: None,
            state,
            pty: false,
        }
    }

    #[test]
    fn the_id_travels_as_text_and_comes_back_without_losing_bits() {
        let id = TunnelId(u64::MAX - 3);
        let text = id.to_string();
        assert_eq!(text.len(), 16);
        assert_eq!(parse_tunnel(&text), Ok(id));
        assert!(parse_tunnel("not hex").is_err());
    }

    #[test]
    fn projects_keep_order_processes_and_state() {
        let mut running = TunnelView::from(tunnel(1, Some("tandem"), TunnelState::Open));
        running.running = vec![process(3, ProcessState::Running)];
        let groups = group_by_project([
            running,
            TunnelView::from(tunnel(2, None, TunnelState::Closed)),
        ]);
        let items = projects_of(groups);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name.as_deref(), Some("tandem"));
        assert_eq!(items[0].root.as_deref(), Some("/src/tandem"));
        assert_eq!(
            items[0].tunnels[0].running[0].state,
            ProcessStateView::Running
        );
        assert_eq!(items[0].tunnels[0].running[0].id, 3);
        assert_eq!(items[1].name, None);
        assert!(!items[1].tunnels[0].open);
    }

    #[test]
    fn output_arrives_as_text_with_its_cursor() {
        let out = ProcessOutput {
            process: process(3, ProcessState::Exited { code: Some(0) }),
            stdout: b"ok\n".to_vec(),
            stderr: vec![b'e', 0xff],
            from: cursor(0, 0),
            next: cursor(3, 2),
            done: true,
        };
        let item = OutputItem::from(out);
        assert_eq!(item.stdout, "ok\n");
        assert_eq!(item.stderr, "e\u{fffd}");
        assert_eq!((item.next_stdout, item.next_stderr), (3, 2));
        assert!(item.done);
        assert_eq!(
            item.process.state,
            ProcessStateView::Exited { code: Some(0) }
        );
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(&[0xe2, 0x82]), "4oI=");
        assert_eq!(base64(&[0xff, 0xfe, 0xfd, 0x00]), "//79AA==");
    }

    /// Live output carries raw bytes: a UTF-8 character split between two pushes is not turned
    /// into a replacement character (the terminal view joins them), and the cursors say exactly
    /// where it goes.
    #[test]
    fn live_output_keeps_raw_bytes_and_its_cursors() {
        let mut p = process(3, ProcessState::Running);
        p.pty = true;
        let item = LiveOutputItem::from(ProcessOutput {
            process: p,
            stdout: vec![b'a', 0xe2, 0x82],
            stderr: Vec::new(),
            from: cursor(10, 0),
            next: cursor(13, 0),
            done: false,
        });
        assert_eq!(item.data, base64(&[b'a', 0xe2, 0x82]));
        assert_eq!((item.from_stdout, item.next_stdout), (10, 13));
        assert!(item.pty && !item.done);
        assert_eq!(item.tunnel, "0000000000000001");
    }

    #[test]
    fn the_transfer_names_the_tunnel_with_its_text_id() {
        let t = TransferItem::from(TransferProgress {
            tunnel: TunnelId(255),
            upload: true,
            path: "/a".into(),
            done: 1,
            total: 2,
            finished: false,
        });
        assert_eq!(t.tunnel, "00000000000000ff");
    }
}
