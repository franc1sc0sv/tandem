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
