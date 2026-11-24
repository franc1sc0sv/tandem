//! Tunnels between the two machines (DESIGN §8, DECISIONS D8, D9): the model and the messages.
//!
//! A tunnel is opened by a local client (Claude through `tandem mcp`, or a person through the CLI)
//! on the **source** machine; its state and resources live on the **destination**. Between the
//! two agents they go over a stream of their own ([`crate::Channel::Tunnel`]): the source sends a
//! [`TunnelUp`] and the destination answers with [`TunnelDown`]. The
//! [`TunnelUp::Attach`] request turns the stream into the tunnel's data channel (data
//! priority, T-101): the output of each flow comes down through it with its byte position, so
//! on reconnecting the source asks "from byte N" and nothing is lost or repeated.
//!
//! The logic (registry, ring buffer, reconnection) is in `tandem-tunnel`.

use std::fmt;

use serde::{Deserialize, Serialize};
use tandem_core::Machine;

/// Identity of a tunnel. The source picks it at random: this way opening is idempotent (a
/// retry with the same id returns the same tunnel) and does not clash between the two machines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TunnelId(pub u64);

impl fmt::Display for TunnelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// The repo or folder the client works in; groups the tunnels in the UI.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Project {
    /// For display (the folder name, normally).
    pub name: String,
    /// Path on the source machine.
    pub root: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClientKind {
    /// `tandem mcp` (Claude).
    Mcp,
    /// `tandem` from a terminal.
    Cli,
}

/// Who opened the tunnel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Client {
    pub kind: ClientKind,
    /// The client's session (Claude's), to recognize its tunnels.
    pub session: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TunnelState {
    Open,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tunnel {
    pub id: TunnelId,
    /// The Claude session's, or the project name.
    pub title: String,
    pub project: Option<Project>,
    /// Where the client is.
    pub origin: Machine,
    /// Where the tunnel's processes, files and ports live.
    pub target: Machine,
    pub opened_by: Client,
    pub state: TunnelState,
    /// Milliseconds since 1970 (UTC), according to the source's clock.
    pub created: u64,
}

/// An output stream inside a tunnel (a process's output, for example). Its id is
/// unique within the tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FlowId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowInfo {
    pub id: FlowId,
    /// For display ("cargo test · stdout").
    pub label: String,
}

/// A process inside a tunnel (T-104). Its id is unique within the tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProcessId(pub u32);

impl fmt::Display for ProcessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "p{}", self.0)
    }
}

/// What to launch on the destination (`exec`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecSpec {
    /// The program; with `shell`, the command line that shell interprets.
    pub cmd: String,
    pub args: Vec<String>,
    /// A detected environment of the destination ([`crate::Environment::id`]: `zsh`, `pwsh`,
    /// `cmd`, `wsl:<distro>`), or `docker:<container>`. `None`: the program directly, without
    /// shell.
    pub shell: Option<String>,
    /// Working folder on the destination (the agent's if none).
    pub cwd: Option<String>,
    /// Variables added to the agent's.
    pub env: Vec<(String, String)>,
    /// Lets through the `env` variables that look like secrets (`GH_TOKEN`...). By default they are
    /// rejected (T-130); `SSH_AUTH_SOCK` and the agents never pass.
    #[serde(default)]
    pub allow_secret_env: bool,
    /// Run it in a terminal of this size (a pseudo-terminal: ConPTY on Windows): the program sees
    /// a TTY (colors, progress, line editing, no block buffering), stdout and stderr come
    /// together in the stdout flow, and it takes input ([`TunnelUp::Input`]). `None`: pipes, no
    /// input.
    #[serde(default)]
    pub pty: Option<PtySize>,
}

/// A terminal's size in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}

impl Default for PtySize {
    fn default() -> Self {
        PtySize {
            cols: 120,
            rows: 32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProcessState {
    Running,
    /// It finished on its own; `code` is `None` if a signal ended it.
    Exited {
        code: Option<i32>,
    },
    /// Killed by `kill` or by the tunnel closing (with its whole tree).
    Killed,
}

/// A tunnel's process as both machines see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelProcess {
    pub id: ProcessId,
    pub tunnel: TunnelId,
    /// For display: the program and its arguments.
    pub command: String,
    pub shell: Option<String>,
    pub cwd: Option<String>,
    /// The system's on the destination, if it gave one.
    pub pid: Option<u32>,
    pub stdout: FlowId,
    pub stderr: FlowId,
    /// Milliseconds since 1970 (UTC), according to the destination's clock.
    pub started: u64,
    /// Milliseconds it ran, once finished.
    pub duration: Option<u64>,
    pub state: ProcessState,
    /// It runs in a terminal and takes input ([`ExecSpec::pty`]).
    #[serde(default)]
    pub pty: bool,
}

impl TunnelProcess {
    pub fn is_running(&self) -> bool {
        self.state == ProcessState::Running
    }
}

/// How many bytes of each output the reader already has (`output { from_byte }`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputCursor {
    pub stdout: u64,
    pub stderr: u64,
}

/// A process's output from an [`OutputCursor`], with stdout and stderr separate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessOutput {
    pub process: TunnelProcess,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Where what is returned starts: what was asked for, or later if that was already discarded.
    pub from: OutputCursor,
    /// What to ask for next time.
    pub next: OutputCursor,
    /// The process finished and there is no output left to read.
    pub done: bool,
}

/// From the source to the destination. Each stream carries a single request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TunnelUp {
    /// Open (or, if it already exists with that id, return) the tunnel. Answers
    /// [`TunnelDown::Opened`].
    Open(Tunnel),
    /// Close the tunnel and everything of its on the destination. Answers [`TunnelDown::Closed`].
    Close(TunnelId),
    /// The destination's tunnels. Answers [`TunnelDown::List`].
    List,
    /// Follow the tunnel's output: from here the stream is its data channel. `from` says
    /// how many bytes of each flow the source already has; the ones not listed, from 0.
    Attach {
        tunnel: TunnelId,
        from: Vec<(FlowId, u64)>,
    },
    /// Files (T-106), over their own stream.
    Files(crate::FilesUp),
    /// Ports (T-106), over their own stream.
    Port(crate::PortUp),
    /// Launch a process in the tunnel (T-104). Answers [`TunnelDown::Started`].
    Exec { tunnel: TunnelId, spec: ExecSpec },
    /// Kill a process with its whole tree. Answers [`TunnelDown::Killed`].
    Kill {
        tunnel: TunnelId,
        process: ProcessId,
    },
    /// The tunnel's git workspace (T-107).
    Workspace(crate::WorkspaceUp),
    /// Input for a terminal process: `bytes` go at position `offset` of its input. The
    /// destination keeps only what it did not have yet, so a resend after a cut never types
    /// twice. Answers [`TunnelDown::InputAck`].
    Input {
        tunnel: TunnelId,
        process: ProcessId,
        offset: u64,
        bytes: Vec<u8>,
    },
    /// New size for a terminal process. Answers [`TunnelDown::Resized`].
    Resize {
        tunnel: TunnelId,
        process: ProcessId,
        size: PtySize,
    },
}

/// From the destination to the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TunnelDown {
    Opened(Tunnel),
    Closed(TunnelId),
    List(Vec<Tunnel>),
    /// The request could not be handled (unknown tunnel, the destination is not this machine).
    Refused(String),
    /// Over the data channel: a flow the source did not yet know.
    Flow(FlowInfo),
    /// Bytes of `flow` from position `offset`.
    Data {
        flow: FlowId,
        offset: u64,
        bytes: Vec<u8>,
    },
    /// Bytes `from..to` of `flow` are gone: the ring buffer discarded them while
    /// nobody was reading. What follows starts at `to`.
    Lost {
        flow: FlowId,
        from: u64,
        to: u64,
    },
    /// `flow` ended with `total` bytes and all of them have been sent.
    End {
        flow: FlowId,
        total: u64,
    },
    /// The tunnel closed and all its output has been sent: the data channel ends here.
    TunnelClosed,
    /// Files (T-106), over their own stream.
    Files(crate::FilesDown),
    /// Ports (T-106), over their own stream.
    Port(crate::PortDown),
    /// The process that was launched.
    Started(TunnelProcess),
    Killed(ProcessId),
    /// Over the data channel: a tunnel process, every time it changes (ends).
    Process(TunnelProcess),
    /// The tunnel's git workspace (T-107).
    Workspace(crate::WorkspaceDown),
    /// The process's input now reaches byte `next`.
    InputAck {
        process: ProcessId,
        next: u64,
    },
    Resized(ProcessId),
}

/// A tunnel in the UI, with the processes running in it (finished ones leave
/// the list; through MCP they can be queried a while longer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelView {
    pub tunnel: Tunnel,
    pub running: Vec<TunnelProcess>,
}

impl From<Tunnel> for TunnelView {
    fn from(tunnel: Tunnel) -> Self {
        TunnelView {
            tunnel,
            running: Vec::new(),
        }
    }
}

/// A project's tunnels, for the UI (read only, D9). `project` is `None` for
/// the ones opened without a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectTunnels {
    pub project: Option<Project>,
    pub tunnels: Vec<TunnelView>,
}

/// Groups by project, in project order (the ones without a project last) and, within each
/// one, oldest to newest.
pub fn group_by_project(
    tunnels: impl IntoIterator<Item = impl Into<TunnelView>>,
) -> Vec<ProjectTunnels> {
    let mut groups: Vec<ProjectTunnels> = Vec::new();
    for t in tunnels {
        let t: TunnelView = t.into();
        match groups.iter_mut().find(|g| g.project == t.tunnel.project) {
            Some(g) => g.tunnels.push(t),
            None => groups.push(ProjectTunnels {
                project: t.tunnel.project.clone(),
                tunnels: vec![t],
            }),
        }
    }
    for g in &mut groups {
        g.tunnels.sort_by_key(|t| (t.tunnel.created, t.tunnel.id));
    }
    // `None` < `Some` in `Option`: reversed to leave them last.
    groups.sort_by(|a, b| match (&a.project, &b.project) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode, encode};

    fn tunnel(id: u64, project: Option<&str>, created: u64) -> Tunnel {
        Tunnel {
            id: TunnelId(id),
            title: format!("t{id}"),
            project: project.map(|p| Project {
                name: p.to_owned(),
                root: format!("/src/{p}"),
            }),
            origin: Machine::A,
            target: Machine::B,
            opened_by: Client {
                kind: ClientKind::Mcp,
                session: "s".into(),
            },
            state: TunnelState::Open,
            created,
        }
    }

    #[test]
    fn groups_by_project_with_projectless_last() {
        let groups = group_by_project([
            tunnel(1, None, 5),
            tunnel(2, Some("tandem"), 9),
            tunnel(3, Some("api"), 1),
            tunnel(4, Some("tandem"), 2),
        ]);
        let shape: Vec<(Option<String>, Vec<u64>)> = groups
            .iter()
            .map(|g| {
                (
                    g.project.as_ref().map(|p| p.name.clone()),
                    g.tunnels.iter().map(|t| t.tunnel.id.0).collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (Some("api".into()), vec![3]),
                (Some("tandem".into()), vec![4, 2]),
                (None, vec![1]),
            ]
        );
    }

    #[test]
    fn messages_round_trip() {
        let up = TunnelUp::Attach {
            tunnel: TunnelId(7),
            from: vec![(FlowId(1), 1 << 40)],
        };
        assert_eq!(decode::<TunnelUp>(&encode(&up).unwrap()).unwrap(), up);
        let down = TunnelDown::Data {
            flow: FlowId(3),
            offset: 99,
            bytes: vec![0, 255, 10],
        };
        assert_eq!(decode::<TunnelDown>(&encode(&down).unwrap()).unwrap(), down);
        let open = TunnelUp::Open(tunnel(9, Some("x"), 3));
        assert_eq!(decode::<TunnelUp>(&encode(&open).unwrap()).unwrap(), open);
        let exec = TunnelUp::Exec {
            tunnel: TunnelId(1),
            spec: ExecSpec {
                cmd: "cargo test".into(),
                // A non-ASCII argument (Unicode test data).
                args: vec!["--".into(), "ñ".into()],
                shell: Some("wsl:Ubuntu-24.04".into()),
                cwd: Some("/src".into()),
                env: vec![("RUST_LOG".into(), "debug".into())],
                allow_secret_env: false,
                pty: Some(PtySize { cols: 80, rows: 24 }),
            },
        };
        assert_eq!(decode::<TunnelUp>(&encode(&exec).unwrap()).unwrap(), exec);
        let input = TunnelUp::Input {
            tunnel: TunnelId(1),
            process: ProcessId(2),
            offset: 1 << 33,
            bytes: b"ls\r\x1b[A".to_vec(),
        };
        assert_eq!(decode::<TunnelUp>(&encode(&input).unwrap()).unwrap(), input);
        let resize = TunnelUp::Resize {
            tunnel: TunnelId(1),
            process: ProcessId(2),
            size: PtySize {
                cols: 300,
                rows: 90,
            },
        };
        assert_eq!(
            decode::<TunnelUp>(&encode(&resize).unwrap()).unwrap(),
            resize
        );
        for down in [
            TunnelDown::InputAck {
                process: ProcessId(2),
                next: 77,
            },
            TunnelDown::Resized(ProcessId(2)),
        ] {
            assert_eq!(decode::<TunnelDown>(&encode(&down).unwrap()).unwrap(), down);
        }
        let ws = TunnelUp::Workspace(crate::WorkspaceUp::Pack {
            tunnel: TunnelId(1),
            commit: Some("wip".into()),
        });
        assert_eq!(decode::<TunnelUp>(&encode(&ws).unwrap()).unwrap(), ws);
        let ws = TunnelDown::Workspace(crate::WorkspaceDown::Packed {
            bundle: Some("/x/back.bundle".into()),
            base: "abc".into(),
            commits: vec![crate::CommitInfo {
                id: "def".into(),
                // A non-ASCII summary (Unicode test data).
                summary: "ñ".into(),
                author: "me".into(),
            }],
            committed_pending: false,
        });
        assert_eq!(decode::<TunnelDown>(&encode(&ws).unwrap()).unwrap(), ws);
        let process = TunnelDown::Process(process_sample());
        assert_eq!(
            decode::<TunnelDown>(&encode(&process).unwrap()).unwrap(),
            process
        );
    }

    pub(crate) fn process_sample() -> TunnelProcess {
        TunnelProcess {
            id: ProcessId(3),
            tunnel: TunnelId(7),
            command: "cargo test".into(),
            shell: Some("pwsh".into()),
            cwd: None,
            pid: Some(4242),
            stdout: FlowId(5),
            stderr: FlowId(6),
            started: 1_763_400_000_000,
            duration: Some(1500),
            state: ProcessState::Exited { code: Some(101) },
            pty: true,
        }
    }

    #[test]
    fn id_is_shown_in_fixed_hex() {
        assert_eq!(TunnelId(0xab).to_string(), "00000000000000ab");
    }
}
