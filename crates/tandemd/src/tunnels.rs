//! Tunnels in the agent (T-103): `tandem-tunnel` over QUIC and the IPC.
//!
//! - Each tunnel request goes over a stream of its own ([`Channel::Tunnel`]) on the
//!   active link's connection ([`QuicDialer`]); the other agent serves it with [`serve`]. A tunnel's data
//!   channel moves to data priority, with the pause and cap of its path
//!   (T-101).
//! - [`follow_link`] hands the source each new active link: if the path changes, the
//!   followers resume the output over the new one from the last received byte.
//! - [`handle`]: the IPC requests. Opening, closing, launching processes and killing them can only be
//!   done by a client
//!   ([`Peer::Client`](tandem_proto::Peer::Client): `tandem mcp` or the CLI), which runs as
//!   the user and comes in through the local IPC, which only accepts that user (DESIGN §11). The
//!   UI only watches (D9): it can read the process list and their output.

use std::path::Path;
use std::sync::Arc;

use tandem_core::Machine;
use tandem_files::ForwardSpec;
use tandem_link::qos::Qos;
use tandem_link::transport::{Connection, ControlReceiver, ControlStream, DataSender};
use tandem_proto::{Channel, Client, ClientKind, Reply, TunnelDown, TunnelUp, UiCommand};
use tandem_tunnel::{BoxFuture, Dialer, OpenSpec, OriginWire, TargetWire, WireError};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::Agent;
use crate::network::ActiveLink;
use crate::peer::open_channel;

/// What the agent answers if the UI (or a request without a client) wants to open or close.
pub const CLIENTS_ONLY: &str =
    "tunnels are opened and closed by a client (`tandem mcp` or `tandem`), not the UI";

/// Opens the tunnel streams over a connection with the other agent.
pub struct QuicDialer {
    conn: Connection,
    me: Machine,
}

impl QuicDialer {
    pub fn new(conn: Connection, me: Machine) -> Self {
        QuicDialer { conn, me }
    }
}

impl Dialer for QuicDialer {
    fn dial(&self) -> BoxFuture<'_, Result<Box<dyn OriginWire>, WireError>> {
        Box::pin(async move {
            let stream = open_channel(&self.conn, self.me, Channel::Tunnel)
                .await
                .map_err(WireError::new)?;
            let wire: Box<dyn OriginWire> = Box::new(QuicOrigin {
                qos: self.conn.qos().clone(),
                mode: Mode::Control(stream),
            });
            Ok(wire)
        })
    }
}

/// The source side: control for requests, data from `start_data` (files and
/// ports send bytes in both directions, not only the destination).
struct QuicOrigin {
    qos: Qos,
    mode: Mode,
}

impl OriginWire for QuicOrigin {
    fn send(&mut self, msg: TunnelUp) -> BoxFuture<'_, Result<(), WireError>> {
        Box::pin(async move {
            let r = match &mut self.mode {
                Mode::Control(s) => s.send(&msg).await,
                Mode::Data(tx, _) => tx.send(&msg).await,
                Mode::Gone => return Err(WireError::new(GONE)),
            };
            r.map_err(WireError::new)
        })
    }

    fn recv(&mut self) -> BoxFuture<'_, Result<Option<TunnelDown>, WireError>> {
        Box::pin(async move {
            let r = match &mut self.mode {
                Mode::Control(s) => s.recv().await,
                Mode::Data(_, rx) => rx.recv().await,
                Mode::Gone => return Err(WireError::new(GONE)),
            };
            r.map_err(WireError::new)
        })
    }

    fn start_data(&mut self) -> Result<(), WireError> {
        self.mode.switch_to_data(&self.qos)
    }
}

/// The destination side: control for requests, data from `Attach`.
struct QuicTarget {
    qos: Qos,
    mode: Mode,
}

enum Mode {
    Control(ControlStream),
    Data(DataSender, ControlReceiver),
    /// Only during the switch to data, if it failed.
    Gone,
}

const GONE: &str = "tunnel stream unusable";

impl Mode {
    /// Switches to data (minimum priority, with the pause and cap of `qos`); if it already is, nothing.
    fn switch_to_data(&mut self, qos: &Qos) -> Result<(), WireError> {
        match std::mem::replace(self, Mode::Gone) {
            Mode::Control(s) => {
                let (tx, rx) = s.into_data(qos).map_err(WireError::new)?;
                *self = Mode::Data(tx, rx);
                Ok(())
            }
            other => {
                *self = other;
                Ok(())
            }
        }
    }
}

impl TargetWire for QuicTarget {
    async fn recv(&mut self) -> Result<Option<TunnelUp>, WireError> {
        let r = match &mut self.mode {
            Mode::Control(s) => s.recv().await,
            Mode::Data(_, rx) => rx.recv().await,
            Mode::Gone => return Err(WireError::new(GONE)),
        };
        r.map_err(WireError::new)
    }

    async fn send(&mut self, msg: &TunnelDown) -> Result<(), WireError> {
        let r = match &mut self.mode {
            Mode::Control(s) => s.send(msg).await,
            Mode::Data(tx, _) => tx.send(msg).await,
            Mode::Gone => return Err(WireError::new(GONE)),
        };
        r.map_err(WireError::new)
    }

    fn start_data(&mut self) -> Result<(), WireError> {
        self.mode.switch_to_data(&self.qos)
    }
}

/// Serves a [`Channel::Tunnel`] stream the other agent opened over `conn`.
pub async fn serve(stream: ControlStream, conn: &Connection, agent: &Agent) -> Result<(), String> {
    let wire = QuicTarget {
        qos: conn.qos().clone(),
        mode: Mode::Control(stream),
    };
    agent
        .workspaces()
        .serve(agent.files(), &agent.tunnels().host, wire)
        .await
        .map_err(|e| e.to_string())
}

/// Carries each new active link to the tunnels' source, until `links` closes.
pub fn follow_link(
    agent: Arc<Agent>,
    mut links: watch::Receiver<Option<ActiveLink>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut current: Option<ActiveLink> = None;
        loop {
            let link = links.borrow_and_update().clone();
            let same = match (&current, &link) {
                (Some(a), Some(b)) => a.same(b),
                (None, None) => true,
                _ => false,
            };
            if !same {
                let dialer = link.as_ref().map(|l| {
                    Arc::new(QuicDialer::new(l.handle.conn.clone(), agent.machine()))
                        as Arc<dyn Dialer>
                });
                agent.tunnels().origin.set_link(dialer);
                current = link;
            }
            if links.changed().await.is_err() {
                return;
            }
        }
    })
}

/// An IPC request. `client` is who greeted: `None` for the UI.
pub async fn handle(agent: &Arc<Agent>, client: Option<ClientKind>, cmd: UiCommand) -> Reply {
    match cmd {
        UiCommand::OpenTunnel {
            title,
            project,
            target,
            session,
            workspace,
        } => {
            let Some(kind) = client else {
                return Reply::Rejected(CLIENTS_ONLY.into());
            };
            // Without a title, the project's (DESIGN §8).
            let title = match (title.trim(), &project) {
                ("", Some(p)) => p.name.clone(),
                (t, _) => t.to_owned(),
            };
            let spec = OpenSpec {
                title,
                project,
                origin: agent.machine(),
                target,
                opened_by: Client { kind, session },
            };
            let origin = &agent.tunnels().origin;
            let tunnel = match origin.open(spec).await {
                Ok(t) => t,
                Err(e) => return Reply::Rejected(e.to_string()),
            };
            let mut skipped = Vec::new();
            let mut history_secrets = Vec::new();
            if let Some(ws) = workspace {
                let opened = agent
                    .workspaces()
                    .open(agent.files(), origin, &tunnel, &ws)
                    .await;
                match opened {
                    Ok(o) => {
                        skipped = o.skipped;
                        history_secrets = o.history_secrets;
                    }
                    Err(e) => {
                        // A half-built tunnel (without its repo) is no use to anyone.
                        let _ = origin.close(tunnel.id).await;
                        return Reply::Rejected(format!("could not prepare the workspace: {e}"));
                    }
                }
            }
            Reply::Tunnel {
                tunnel: Box::new(tunnel),
                skipped,
                history_secrets,
            }
        }
        UiCommand::CloseTunnel { id } => {
            if client.is_none() {
                return Reply::Rejected(CLIENTS_ONLY.into());
            }
            agent.workspaces().forget(id);
            match agent.tunnels().origin.close(id).await {
                Ok(()) => Reply::Done,
                Err(e) => Reply::Rejected(e.to_string()),
            }
        }
        cmd @ (UiCommand::FilesPut { .. }
        | UiCommand::FilesGet { .. }
        | UiCommand::PortForward { .. }
        | UiCommand::StopForward { .. }) => {
            if client.is_none() {
                return Reply::Rejected(CLIENTS_ONLY.into());
            }
            files_and_ports(agent, cmd).await
        }
        UiCommand::BringBack {
            tunnel,
            commit,
            name,
        } => {
            if client.is_none() {
                return Reply::Rejected(CLIENTS_ONLY.into());
            }
            let (files, origin) = (agent.files(), &agent.tunnels().origin);
            match agent
                .workspaces()
                .bring_back(files, origin, tunnel, commit, name)
                .await
            {
                Ok(r) => Reply::BroughtBack(Box::new(r)),
                Err(e) => Reply::Rejected(e.to_string()),
            }
        }
        UiCommand::Exec { tunnel, spec } => {
            if client.is_none() {
                return Reply::Rejected(CLIENTS_ONLY.into());
            }
            match agent.tunnels().origin.exec(tunnel, spec).await {
                Ok(p) => Reply::Process(Box::new(p)),
                Err(e) => Reply::Rejected(e.to_string()),
            }
        }
        UiCommand::Kill { tunnel, process } => {
            if client.is_none() {
                return Reply::Rejected(CLIENTS_ONLY.into());
            }
            match agent.tunnels().origin.kill(tunnel, process).await {
                Ok(()) => Reply::Done,
                Err(e) => Reply::Rejected(e.to_string()),
            }
        }
        other => agent.handle(other),
    }
}

/// Files and ports (T-106): what a client asks over an open tunnel.
async fn files_and_ports(agent: &Arc<Agent>, cmd: UiCommand) -> Reply {
    let (files, origin) = (agent.files(), &agent.tunnels().origin);
    let rejected = |e: tandem_files::FilesError| Reply::Rejected(e.to_string());
    match cmd {
        UiCommand::FilesPut {
            tunnel,
            local,
            remote,
            include,
        } => match files
            .put_with(origin, tunnel, Path::new(&local), &remote, &include)
            .await
        {
            Ok(r) => Reply::Files(r),
            Err(e) => rejected(e),
        },
        UiCommand::FilesGet {
            tunnel,
            remote,
            local,
        } => match files.get(origin, tunnel, &remote, Path::new(&local)).await {
            Ok(r) => Reply::Files(r),
            Err(e) => rejected(e),
        },
        UiCommand::PortForward {
            tunnel,
            local_port,
            remote_host,
            remote_port,
            direction,
        } => {
            let spec = ForwardSpec {
                tunnel,
                local_port,
                remote_host,
                remote_port,
                direction,
            };
            match files.forward(origin, spec).await {
                Ok(info) => Reply::Forward(info),
                Err(e) => rejected(e),
            }
        }
        UiCommand::StopForward { id } => {
            if files.forwards().stop(id) {
                Reply::Done
            } else {
                Reply::Rejected(format!("there is no forward {id}"))
            }
        }
        _ => Reply::Rejected("unknown files or ports request".into()),
    }
}

/// The requests that take a while (file copies): the IPC runs them in its own task so as not to
/// stall the connection meanwhile.
pub fn is_slow(cmd: &UiCommand) -> bool {
    matches!(
        cmd,
        UiCommand::FilesPut { .. }
            | UiCommand::FilesGet { .. }
            | UiCommand::BringBack { .. }
            | UiCommand::OpenTunnel {
                workspace: Some(_),
                ..
            }
    )
}
