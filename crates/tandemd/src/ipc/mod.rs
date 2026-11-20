//! Local API for the UI: Unix socket on macOS (and Linux) and named pipe on Windows,
//! accessible only to the current user (DESIGN §9).
//!
//! It speaks the UI <-> agent messages of `tandem-proto`: each side sends its [`Hello`]
//! (the agent first), and then the UI sends [`UiRequest`] and the agent answers with
//! [`AgentToUi`]. The agent also pushes [`AgentToUi::State`] every time the state changes
//! and [`AgentToUi::Txn`] with the progress of each power on or off, [`AgentToUi::Power`]
//! when the other machine is considered off without anyone turning it off, [`AgentToUi::Peer`]
//! when the peer or the link to it changes, [`AgentToUi::Pairing`] with each pairing
//! step, [`AgentToUi::Input`] when the keyboard and mouse change (roles, session,
//! missing permissions), [`AgentToUi::Desk`] when the shared desk changes,
//! [`AgentToUi::DeskChanged`] when that change came from the other machine (the notice with
//! "Undo") and [`AgentToUi::Machines`] when what either of the two announces changes
//! (modules and environments included). A connection that asked for
//! [`UiCommand::WatchResources`] also receives [`AgentToUi::Resources`] from both
//! machines while it stays open, and one that asked for [`UiCommand::WatchNetwork`] receives
//! [`AgentToUi::Network`] once per second (T-102); one that asked for
//! [`UiCommand::WatchCursor`] receives [`AgentToUi::Cursor`] (at 30 Hz at most) and
//! [`AgentToUi::Crossed`] (T-128). [`UiCommand::MeasureLink`] is
//! answered when the test finishes, without holding up the notices of that connection.
//! [`AgentToUi::Update`] goes when the version of either of the two changes.
//! [`AgentToUi::ShowWindow`] goes only to UI connections, when Tandem is opened from the system
//! and the agent gets the request (T-350).
//!
//! The transport goes behind [`Listener`]: the logic is tested with [`mem`] and the
//! real socket or pipe in the integration tests.

use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tandem_core::{Machine, State};
use tandem_input::session::CursorEvent;
use tandem_proto::{
    AgentToUi, FrameDecoder, Hello, Peer, ProtoError, Reply, UiCommand, UiRequest, decode,
    decode_hello, encode_frame,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;
use tokio::time::{self, Interval, MissedTickBehavior};

use crate::agent::{Agent, CursorWatch, ResourceWatch};

pub mod mem;
#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::{UnixIpcListener as PlatformListener, bind, connect, connect_as, default_endpoint};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{PipeListener as PlatformListener, bind, connect, connect_as, default_endpoint};

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("I/O error in the IPC: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Proto(#[from] ProtoError),
    #[error("the other side closed the connection")]
    Closed,
    #[error("expected {expected}, {got:?} connected")]
    UnexpectedPeer { expected: &'static str, got: Peer },
    #[error("unexpected reply: {0:?}")]
    UnexpectedReply(Box<Reply>),
    #[error("a tandemd is already listening on {0}")]
    AlreadyRunning(PathBuf),
    #[error("{0} exists and is not a socket; it is not deleted")]
    NotASocket(PathBuf),
}

/// Source of UI connections.
pub trait Listener: Send {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    /// The next already accepted connection (and, on the real socket, from the right user).
    fn accept(&mut self) -> impl Future<Output = io::Result<Self::Stream>> + Send;
}

/// Framed `tandem-proto` messages over a byte stream.
#[derive(Debug)]
pub struct Framed<S> {
    io: S,
    decoder: FrameDecoder,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Framed<S> {
    pub fn new(io: S) -> Self {
        Framed {
            io,
            decoder: FrameDecoder::new(),
        }
    }

    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<(), IpcError> {
        self.io.write_all(&encode_frame(msg)?).await?;
        self.io.flush().await?;
        Ok(())
    }

    /// `None` if the other side closed cleanly, between two messages.
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<Option<T>, IpcError> {
        match self.recv_frame().await? {
            Some(p) => Ok(Some(decode(&p)?)),
            None => Ok(None),
        }
    }

    /// Reads the opening [`Hello`], rejecting another protocol version.
    pub async fn recv_hello(&mut self) -> Result<Hello, IpcError> {
        let p = self.recv_frame().await?.ok_or(IpcError::Closed)?;
        Ok(decode_hello(&p)?)
    }

    /// It can be canceled (for example in a `select!`) without losing bytes: what was read is
    /// kept in the decoder before the next wait point.
    async fn recv_frame(&mut self) -> Result<Option<Vec<u8>>, IpcError> {
        let mut buf = [0u8; 4096];
        loop {
            if let Some(frame) = self.decoder.next_frame()? {
                return Ok(Some(frame));
            }
            let n = self.io.read(&mut buf).await?;
            if n == 0 {
                return if self.decoder.buffered() == 0 {
                    Ok(None)
                } else {
                    Err(io::Error::from(io::ErrorKind::UnexpectedEof).into())
                };
            }
            self.decoder.push(&buf[..n]);
        }
    }
}

/// Serves a UI connection until it closes or shutdown arrives.
pub async fn serve_connection<S>(
    io: S,
    agent: Arc<Agent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), IpcError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut conn = Framed::new(io);
    conn.send(&Hello::new(Peer::Agent(agent.machine()))).await?;
    let hello = conn.recv_hello().await?;
    // The UI or a client (`tandem mcp`, `tandem`): both run as the user, the only one
    // the IPC lets in. Only a client opens and closes tunnels.
    let client = match hello.peer {
        Peer::Ui => None,
        Peer::Client(kind) => Some(kind),
        Peer::Agent(_) => {
            return Err(IpcError::UnexpectedPeer {
                expected: "the UI or a client",
                got: hello.peer,
            });
        }
    };
    // Only the UI counts as "a UI is open" and hears the show requests (T-350).
    let mut ui = client.is_none().then(|| agent.ui().connect());
    let mut states = agent.subscribe();
    states.mark_unchanged();
    let mut txns = agent.subscribe_txns();
    let mut power = agent.subscribe_power();
    let mut drag = agent.drag().subscribe();
    let mut peer = agent.subscribe_peer();
    peer.mark_unchanged();
    let mut pairing = agent.subscribe_pairing();
    pairing.mark_unchanged();
    let mut input = agent.subscribe_input();
    input.mark_unchanged();
    // The keyboard probe (T-166). Without an initial state: the window arms the probe on entering the
    // step, and arming already emits the current state.
    let mut key_probe = agent.subscribe_key_probe();
    key_probe.mark_unchanged();
    let mut desk = agent.subscribe_desk();
    desk.mark_unchanged();
    let mut caps = agent.subscribe_capabilities();
    caps.mark_unchanged();
    let mut desk_changes = agent.subscribe_desk_changes();
    let mut monitors = agent.subscribe_monitors();
    monitors.mark_unchanged();
    let mut tunnels = agent.tunnels().subscribe();
    tunnels.mark_unchanged();
    // Live resources, if this connection asked for them: they stop being sampled when it closes.
    let mut resources: Option<ResourceWatch> = None;
    // The live cursor, if this connection asked for it: without connections, the engine does not sample it.
    let mut cursor: Option<CursorWatch> = None;
    let mut update = agent.subscribe_update();
    update.mark_unchanged();
    // The network once per second, if this connection asked for it (the Network screen visible).
    let mut network: Option<Interval> = None;
    // The bandwidth measurement this connection asked for, with its request's id: it takes
    // a few seconds and meanwhile the connection keeps serving the rest.
    let mut measuring: Option<(u64, Measurement)> = None;
    // This connection's file copies (T-106), with their reply channel and the progress
    // of this agent's transfers.
    let mut jobs = tokio::task::JoinSet::new();
    let (jobs_tx, mut jobs_rx) = tokio::sync::mpsc::unbounded_channel::<(u64, Reply)>();
    let mut progress = agent.files().subscribe_progress();
    loop {
        tokio::select! {
            req = conn.recv::<UiRequest>() => {
                let Some(req) = req? else { return Ok(()) };
                tracing::debug!(id = req.id, command = ?req.command, "UI request");
                let reply = match req.command {
                    UiCommand::WatchResources { on } => {
                        resources = on.then(|| agent.watch_resources());
                        Reply::Done
                    }
                    UiCommand::WatchNetwork { on } => {
                        network = on.then(network_ticker);
                        Reply::Done
                    }
                    UiCommand::WatchCursor { on } => {
                        // Asking twice does not add up: the connection counts once.
                        if on && cursor.is_none() {
                            cursor = Some(agent.watch_cursor());
                        } else if !on {
                            cursor = None;
                        }
                        Reply::Done
                    }
                    UiCommand::MeasureLink { .. } if measuring.is_some() => {
                        Reply::Rejected("A path is already being measured".into())
                    }
                    UiCommand::MeasureLink { path } => {
                        let agent = agent.clone();
                        let run: Measurement = Box::pin(async move { agent.measure_link(path).await });
                        measuring = Some((req.id, run));
                        continue;
                    }
                    command if crate::tunnels::is_slow(&command) => {
                        // A copy takes a while: it runs apart and the connection keeps serving. If the
                        // connection closes, `jobs` is dropped and the copy is interrupted (what was
                        // received stays on disk and resumes).
                        let (agent, done, id) = (agent.clone(), jobs_tx.clone(), req.id);
                        jobs.spawn(async move {
                            let reply = crate::tunnels::handle(&agent, client, command).await;
                            let _ = done.send((id, reply));
                        });
                        continue;
                    }
                    command => crate::tunnels::handle(&agent, client, command).await,
                };
                conn.send(&AgentToUi::Reply { id: req.id, reply }).await?;
            }
            () = next_tick(&mut network) => {
                conn.send(&AgentToUi::Network(Box::new(agent.network_status()))).await?;
            }
            Some((id, reply)) = jobs_rx.recv() => {
                conn.send(&AgentToUi::Reply { id, reply }).await?;
            }
            Some(_) = jobs.join_next(), if !jobs.is_empty() => {}
            p = progress.recv() => match p {
                Ok(p) => conn.send(&AgentToUi::Transfer(p)).await?,
                // If this connection lagged, old progress is lost and nothing happens.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return Ok(()),
            },
            (id, reply) = measured(&mut measuring) => {
                measuring = None;
                conn.send(&AgentToUi::Reply { id, reply }).await?;
            }
            event = next_cursor(&mut cursor) => match event {
                Some(CursorEvent::Sample(sample)) => conn.send(&AgentToUi::Cursor(sample)).await?,
                Some(CursorEvent::Crossed { from, to, edge }) => {
                    conn.send(&AgentToUi::Crossed { from, to, edge }).await?;
                }
                None => cursor = None,
            },
            sample = next_resources(&mut resources) => match sample {
                Some((machine, r)) => {
                    conn.send(&AgentToUi::Resources { machine, resources: Box::new(r) }).await?;
                }
                None => resources = None,
            },
            changed = states.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let state = states.borrow_and_update().clone();
                conn.send(&AgentToUi::State(state)).await?;
            }
            event = txns.recv() => match event {
                Ok(event) => conn.send(&AgentToUi::Txn(event)).await?,
                // The UI resynchronizes with the next pushed state.
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!("the UI lagged and lost {n} transaction notices");
                }
                Err(RecvError::Closed) => return Ok(()),
            },
            event = drag.recv() => match event {
                Ok(status) => conn.send(&AgentToUi::Drag(status)).await?,
                // The next notice replaces the lost one.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return Ok(()),
            },
            event = power.recv() => match event {
                Ok(event) => conn.send(&AgentToUi::Power(event)).await?,
                // The pushed state already says the other machine is off.
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!("the UI lagged and lost {n} power notices");
                }
                Err(RecvError::Closed) => return Ok(()),
            },
            changed = peer.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let status = peer.borrow_and_update().clone();
                conn.send(&AgentToUi::Peer(status)).await?;
            }
            changed = pairing.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let status = pairing.borrow_and_update().clone();
                conn.send(&AgentToUi::Pairing(status)).await?;
            }
            changed = input.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let status = input.borrow_and_update().clone();
                conn.send(&AgentToUi::Input(status)).await?;
            }
            changed = key_probe.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let status = *key_probe.borrow_and_update();
                conn.send(&AgentToUi::KeyProbe(status)).await?;
            }
            changed = desk.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let d = desk.borrow_and_update().clone();
                conn.send(&AgentToUi::Desk(d)).await?;
            }
            event = desk_changes.recv() => match event {
                Ok(c) => {
                    conn.send(&AgentToUi::DeskChanged {
                        by: c.by,
                        rev: c.rev,
                        summary: c.summary,
                        undo_to: c.undo_to,
                    })
                    .await?;
                }
                // The new desk already arrived pushed; only the notice is lost.
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!("the UI lagged and lost {n} desk change notices");
                }
                Err(RecvError::Closed) => return Ok(()),
            },
            changed = caps.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let c = caps.borrow_and_update().clone();
                conn.send(&machines(c)).await?;
            }
            changed = monitors.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let views = monitors.borrow_and_update().clone();
                conn.send(&AgentToUi::Monitors(views)).await?;
            }
            changed = update.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let u = update.borrow_and_update().clone();
                conn.send(&AgentToUi::Update(u)).await?;
            }
            changed = tunnels.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                tunnels.borrow_and_update();
                conn.send(&AgentToUi::Tunnels(agent.tunnels().view())).await?;
            }
            () = shown(&mut ui) => conn.send(&AgentToUi::ShowWindow).await?,
            () = stopped(&mut shutdown) => return Ok(()),
        }
    }
}

async fn shown(ui: &mut Option<crate::ui_open::UiConnection>) {
    match ui {
        Some(ui) => ui.shown().await,
        None => std::future::pending().await,
    }
}

/// A bandwidth measurement in progress ([`Agent::measure_link`]).
type Measurement = Pin<Box<dyn Future<Output = Reply> + Send>>;

/// How often [`AgentToUi::Network`] goes to whoever asked for it.
pub const NETWORK_EVERY: Duration = Duration::from_secs(1);

/// The first right away (the screen is drawn now), then one per second.
fn network_ticker() -> Interval {
    let mut tick = time::interval(NETWORK_EVERY);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    tick
}

/// The next network send; it never arrives if the connection did not ask for it.
async fn next_tick(tick: &mut Option<Interval>) {
    match tick {
        Some(t) => {
            t.tick().await;
        }
        None => std::future::pending().await,
    }
}

/// The end of the measurement in progress, with its request's id; it never arrives without a measurement.
async fn measured(m: &mut Option<(u64, Measurement)>) -> (u64, Reply) {
    match m {
        Some((id, run)) => (*id, run.await),
        None => std::future::pending().await,
    }
}

/// The next resources sample; it never ends if the connection did not ask for them.
async fn next_resources(
    watch: &mut Option<ResourceWatch>,
) -> Option<(Machine, tandem_proto::Resources)> {
    match watch {
        Some(w) => w.next().await,
        None => std::future::pending().await,
    }
}

/// The next cursor event; it never ends if the connection did not ask for it.
async fn next_cursor(watch: &mut Option<CursorWatch>) -> Option<CursorEvent> {
    match watch {
        Some(w) => w.next().await,
        None => std::future::pending().await,
    }
}

/// What the two machines announce, as a notice for the UI.
pub fn machines(c: tandem_proto::DeskCapabilities) -> AgentToUi {
    AgentToUi::Machines {
        local: Box::new(c.here),
        remote: c.peer.map(Box::new),
    }
}

/// Ends when shutdown is requested or whoever would request it disappears.
async fn stopped(shutdown: &mut watch::Receiver<bool>) {
    // The `Ref` that `wait_for` returns is not `Send`: it is dropped right here.
    let _ = shutdown.wait_for(|stop| *stop).await.map(|_| ());
}

/// IPC client: used by the tests and, later, the UI.
#[derive(Debug)]
pub struct IpcClient<S> {
    conn: Framed<S>,
    agent: Machine,
    next_id: u64,
    /// Notices that arrived while waiting for a reply.
    pending: VecDeque<AgentToUi>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> IpcClient<S> {
    pub async fn handshake(io: S) -> Result<Self, IpcError> {
        Self::handshake_as(io, Peer::Ui).await
    }

    /// Like [`Self::handshake`], introducing itself as `peer`: `tandem mcp` and the CLI do so
    /// as [`Peer::Client`] to be able to open and close tunnels.
    pub async fn handshake_as(io: S, peer: Peer) -> Result<Self, IpcError> {
        let mut conn = Framed::new(io);
        conn.send(&Hello::new(peer)).await?;
        let hello = conn.recv_hello().await?;
        let Peer::Agent(agent) = hello.peer else {
            return Err(IpcError::UnexpectedPeer {
                expected: "an agent",
                got: hello.peer,
            });
        };
        Ok(IpcClient {
            conn,
            agent,
            next_id: 0,
            pending: VecDeque::new(),
        })
    }

    /// The agent's machine on the other side.
    pub fn agent(&self) -> Machine {
        self.agent
    }

    pub async fn request(&mut self, command: UiCommand) -> Result<Reply, IpcError> {
        let id = self.next_id;
        self.next_id += 1;
        self.conn.send(&UiRequest { id, command }).await?;
        loop {
            match self
                .conn
                .recv::<AgentToUi>()
                .await?
                .ok_or(IpcError::Closed)?
            {
                AgentToUi::Reply { id: got, reply } if got == id => return Ok(reply),
                other => self.pending.push_back(other),
            }
        }
    }

    pub async fn get_state(&mut self) -> Result<State, IpcError> {
        match self.request(UiCommand::GetState).await? {
            Reply::State(s) => Ok(s),
            other => Err(IpcError::UnexpectedReply(Box::new(other))),
        }
    }

    /// The agent's next spontaneous notice; `None` if it closed.
    pub async fn next_event(&mut self) -> Result<Option<AgentToUi>, IpcError> {
        if let Some(ev) = self.pending.pop_front() {
            return Ok(Some(ev));
        }
        self.conn.recv().await
    }
}

#[cfg(test)]
mod tests {
    use tandem_proto::PROTOCOL_VERSION;
    use tokio::io::duplex;

    use super::*;
    use tandem_proto::TxnEvent;

    use crate::agent::tests::agent;
    use crate::orchestrator::STEP_NOT_IMPLEMENTED;

    fn start(dir: &std::path::Path) -> (Arc<Agent>, tokio::io::DuplexStream, watch::Sender<bool>) {
        let agent = Arc::new(agent(dir));
        let (client, server) = duplex(64);
        let (stop, stop_rx) = watch::channel(false);
        let a = agent.clone();
        tokio::spawn(async move { serve_connection(server, a, stop_rx).await });
        (agent, client, stop)
    }

    #[tokio::test]
    async fn client_gets_state() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        assert_eq!(c.agent(), Machine::A);
        assert_eq!(c.get_state().await.unwrap(), agent.state());
    }

    /// T-105: resources reach only the connection that asked for them, from both machines, and
    /// stop arriving when it asks not to.
    #[tokio::test(start_paused = true)]
    async fn resources_flow_only_while_the_connection_watches() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        assert_eq!(
            c.request(UiCommand::WatchResources { on: true })
                .await
                .unwrap(),
            Reply::Done
        );
        let Some(AgentToUi::Resources { machine, .. }) = c.next_event().await.unwrap() else {
            panic!("resources were expected");
        };
        assert_eq!(machine, Machine::A);
        let mut theirs = tandem_proto::Resources::default();
        theirs.cpu.load = 420;
        agent.receive_peer_resources(theirs.clone());
        loop {
            match c.next_event().await.unwrap() {
                Some(AgentToUi::Resources {
                    machine: Machine::A,
                    ..
                }) => continue,
                Some(AgentToUi::Resources { machine, resources }) => {
                    assert_eq!((machine, *resources), (Machine::B, theirs));
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(
            c.request(UiCommand::WatchResources { on: false })
                .await
                .unwrap(),
            Reply::Done
        );
        // What arrived before the reply runs out; afterwards, nothing.
        let mut after = 0;
        while let Ok(ev) =
            tokio::time::timeout(std::time::Duration::from_secs(5), c.next_event()).await
        {
            assert!(matches!(ev.unwrap(), Some(AgentToUi::Resources { .. })));
            after += 1;
            assert!(after < 5, "resources keep arriving");
        }
    }

    /// T-102: without a subscription the network does not arrive; with it, once per second; when it is removed,
    /// it stops arriving.
    #[tokio::test(start_paused = true)]
    async fn network_flows_only_while_the_connection_watches() {
        let dir = tempfile::tempdir().unwrap();
        let (_agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        assert!(
            time::timeout(Duration::from_secs(5), c.next_event())
                .await
                .is_err(),
            "something arrived without a subscription"
        );
        assert_eq!(
            c.request(UiCommand::WatchNetwork { on: true })
                .await
                .unwrap(),
            Reply::Done
        );
        let t0 = time::Instant::now();
        for _ in 0..3 {
            let Some(AgentToUi::Network(s)) = c.next_event().await.unwrap() else {
                panic!("the network was expected");
            };
            assert_eq!(s.paths.len(), 3);
        }
        // The first right away and then one per second.
        assert_eq!(t0.elapsed(), 2 * NETWORK_EVERY);
        assert_eq!(
            c.request(UiCommand::WatchNetwork { on: false })
                .await
                .unwrap(),
            Reply::Done
        );
        let mut after = 0;
        while let Ok(ev) = time::timeout(Duration::from_secs(5), c.next_event()).await {
            assert!(matches!(ev.unwrap(), Some(AgentToUi::Network(_))));
            after += 1;
            assert!(after < 3, "the network keeps arriving");
        }
    }

    /// T-128: the cursor reaches only the connection that asked for it, the engine only samples it
    /// while some connection asks (counted per window), and when the connection closes it stops.
    #[tokio::test]
    async fn the_cursor_flows_only_to_connections_that_watch_and_is_counted_per_window() {
        use tandem_proto::CursorSample;

        let dir = tempfile::tempdir().unwrap();
        let (agent, io, stop) = start(dir.path());
        let mut watching = IpcClient::handshake(io).await.unwrap();
        let (client, server) = duplex(64);
        let (a, stopped) = (agent.clone(), stop.subscribe());
        tokio::spawn(async move { serve_connection(server, a, stopped).await });
        let mut idle = IpcClient::handshake(client).await.unwrap();

        let sample = |x| CursorSample {
            machine: Machine::A,
            screen: "mac-1".into(),
            x,
            y: 1,
        };
        let gate = agent.cursor_gate();
        assert!(!gate.is_on());
        agent.publish_cursor(CursorEvent::Sample(sample(0)));

        // Two requests from the same connection count once; a connection that does not ask for it does not
        // receive it.
        for _ in 0..2 {
            assert_eq!(
                watching
                    .request(UiCommand::WatchCursor { on: true })
                    .await
                    .unwrap(),
                Reply::Done
            );
        }
        assert!(gate.is_on());
        agent.publish_cursor(CursorEvent::Sample(sample(1)));
        agent.publish_cursor(CursorEvent::Crossed {
            from: Machine::A,
            to: Machine::B,
            edge: tandem_core::Side::Right,
        });
        assert_eq!(
            watching.next_event().await.unwrap(),
            Some(AgentToUi::Cursor(sample(1)))
        );
        assert_eq!(
            watching.next_event().await.unwrap(),
            Some(AgentToUi::Crossed {
                from: Machine::A,
                to: Machine::B,
                edge: tandem_core::Side::Right
            })
        );
        assert!(
            time::timeout(Duration::from_millis(100), idle.next_event())
                .await
                .is_err(),
            "the cursor reached a connection that did not ask for it"
        );

        // A second window that watches and leaves does not turn off the first.
        assert_eq!(
            idle.request(UiCommand::WatchCursor { on: true })
                .await
                .unwrap(),
            Reply::Done
        );
        assert_eq!(
            idle.request(UiCommand::WatchCursor { on: false })
                .await
                .unwrap(),
            Reply::Done
        );
        assert!(gate.is_on(), "the other window keeps watching");

        // With `on: false` the last one turns off and the engine stops sampling.
        assert_eq!(
            watching
                .request(UiCommand::WatchCursor { on: false })
                .await
                .unwrap(),
            Reply::Done
        );
        assert!(!gate.is_on());

        // Closing the connection without notice also stops watching.
        watching
            .request(UiCommand::WatchCursor { on: true })
            .await
            .unwrap();
        assert!(gate.is_on());
        drop(watching);
        time::timeout(Duration::from_secs(5), async {
            while gate.is_on() {
                time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the closed connection kept counting");
    }

    /// Measuring without a connection over that path is rejected right away, over the same connection.
    #[tokio::test]
    async fn measuring_a_path_without_connection_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let (_agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        let reply = c
            .request(UiCommand::MeasureLink {
                path: tandem_proto::Link::Wifi,
            })
            .await
            .unwrap();
        assert!(matches!(reply, Reply::Rejected(_)), "{reply:?}");
    }

    #[tokio::test]
    async fn peer_status_is_answered_and_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        assert_eq!(
            c.request(UiCommand::GetPeer).await.unwrap(),
            Reply::Peer(tandem_proto::PeerStatus::Unpaired)
        );
        let paired = tandem_proto::PeerStatus::Paired {
            name: "PC".into(),
            link: Some(tandem_proto::Link::Usb4),
        };
        agent.set_peer_status(paired.clone());
        assert_eq!(c.next_event().await.unwrap(), Some(AgentToUi::Peer(paired)));
    }

    #[tokio::test]
    async fn pairing_status_is_answered_and_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        assert_eq!(
            c.request(UiCommand::GetPairing).await.unwrap(),
            Reply::Pairing(tandem_proto::PairingStatus::Idle)
        );
        let comparing = tandem_proto::PairingStatus::Comparing {
            code: "012345".into(),
            peer: "PC".into(),
            confirmed: false,
        };
        agent.set_pairing_status(comparing.clone());
        assert_eq!(
            c.next_event().await.unwrap(),
            Some(AgentToUi::Pairing(comparing))
        );
    }

    #[tokio::test]
    async fn input_status_is_answered_and_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        let initial = tandem_proto::InputStatus::new(Machine::A);
        assert_eq!(
            c.request(UiCommand::GetInput).await.unwrap(),
            Reply::Input(initial.clone())
        );
        let missing = tandem_proto::InputStatus {
            missing: vec![tandem_proto::InputPermission::Accessibility],
            ..initial
        };
        agent.set_input_status(missing.clone());
        // Permissions also change what this machine announces (T-084).
        let mut seen = Vec::new();
        while seen.len() < 2 {
            seen.push(c.next_event().await.unwrap());
        }
        assert!(seen.contains(&Some(AgentToUi::Input(missing))), "{seen:?}");
        assert!(
            seen.iter().any(|e| matches!(
                e,
                Some(AgentToUi::Machines { local, .. }) if local.input.missing == [tandem_proto::InputPermission::Accessibility]
            )),
            "{seen:?}"
        );
    }

    #[tokio::test]
    async fn power_change_progress_is_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        let r = c
            .request(UiCommand::SetPower {
                machine: Machine::B,
                on: true,
            })
            .await
            .unwrap();
        let Reply::Accepted { txn } = r else {
            panic!("Accepted was expected: {r:?}");
        };
        // Without backends, the first step fails with a clear reason and the state does not change.
        let mut events = Vec::new();
        loop {
            match c.next_event().await.unwrap().unwrap() {
                AgentToUi::Txn(e) => {
                    let end = matches!(e, TxnEvent::Failed { .. });
                    events.push(e);
                    if end {
                        break;
                    }
                }
                other => panic!("unexpected notice: {other:?}"),
            }
        }
        assert!(matches!(&events[0], TxnEvent::Started { txn: t, .. } if *t == txn));
        assert_eq!(events[1], TxnEvent::StepStarted { txn, index: 0 });
        assert!(matches!(
            &events[2],
            TxnEvent::StepFailed { index: 0, error, .. } if error.contains(STEP_NOT_IMPLEMENTED)
        ));
        assert_eq!(
            events[3],
            TxnEvent::Failed {
                txn,
                state: agent.state()
            }
        );
        assert_eq!(c.get_state().await.unwrap(), agent.state());
    }

    // T-350: opening Tandem through the agent shows the window of the connected UI.
    #[tokio::test]
    async fn show_window_goes_only_to_ui_connections() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (_stop, stop_rx) = watch::channel(false);
        let (ui_io, ui_server) = duplex(4096);
        tokio::spawn(serve_connection(ui_server, agent.clone(), stop_rx.clone()));
        let (cli_io, cli_server) = duplex(4096);
        tokio::spawn(serve_connection(cli_server, agent.clone(), stop_rx));
        let mut ui = IpcClient::handshake(ui_io).await.unwrap();
        let mut cli = IpcClient::handshake_as(cli_io, Peer::Client(tandem_proto::ClientKind::Cli))
            .await
            .unwrap();
        // Once a request was answered, each connection is registered.
        ui.get_state().await.unwrap();
        cli.get_state().await.unwrap();
        assert_eq!(agent.ui().connected(), 1, "only the UI counts");

        assert_eq!(agent.ui().show(), 1);
        let event = time::timeout(Duration::from_secs(1), ui.next_event()).await;
        assert!(
            matches!(event, Ok(Ok(Some(AgentToUi::ShowWindow)))),
            "{event:?}"
        );
        let nothing = time::timeout(Duration::from_millis(100), cli.next_event()).await;
        assert!(nothing.is_err(), "the client got {nothing:?}");

        drop(ui);
        let deadline = time::Instant::now() + Duration::from_secs(1);
        while agent.ui().connected() > 0 && time::Instant::now() < deadline {
            time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(agent.ui().connected(), 0, "a closed UI no longer counts");
    }

    #[tokio::test]
    async fn state_changes_are_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        // After the greeting, the server's subscription already exists if a request was answered.
        c.get_state().await.unwrap();
        let mut s = agent.state();
        s.on.b = true;
        agent.set_state(s.clone()).unwrap();
        assert_eq!(c.next_event().await.unwrap(), Some(AgentToUi::State(s)));
    }

    #[tokio::test]
    async fn power_events_are_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        c.get_state().await.unwrap();
        let ev = tandem_proto::PowerEvent::PeerLost {
            machine: Machine::B,
            reason: "no links".into(),
        };
        agent.emit_power(ev.clone());
        assert_eq!(c.next_event().await.unwrap(), Some(AgentToUi::Power(ev)));
    }

    #[tokio::test]
    async fn pushed_state_during_request_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        c.get_state().await.unwrap();
        let mut s = agent.state();
        s.on.b = true;
        agent.set_state(s.clone()).unwrap();
        // The notice arrives before the reply; it is not lost.
        assert_eq!(c.get_state().await.unwrap(), s);
        assert_eq!(c.next_event().await.unwrap(), Some(AgentToUi::State(s)));
    }

    #[tokio::test]
    async fn server_rejects_other_protocol_version() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (client, server) = duplex(64);
        let (_stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(serve_connection(server, agent, stop_rx));
        let mut c = Framed::new(client);
        c.send(&Hello {
            version: PROTOCOL_VERSION + 1,
            peer: Peer::Ui,
        })
        .await
        .unwrap();
        let err = task.await.unwrap().unwrap_err();
        assert!(
            matches!(err, IpcError::Proto(ProtoError::IncompatibleVersion { .. })),
            "{err}"
        );
    }

    #[tokio::test]
    async fn server_rejects_an_agent_as_ui() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (client, server) = duplex(64);
        let (_stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(serve_connection(server, agent, stop_rx));
        let mut c = Framed::new(client);
        c.send(&Hello::new(Peer::Agent(Machine::B))).await.unwrap();
        let err = task.await.unwrap().unwrap_err();
        assert!(matches!(err, IpcError::UnexpectedPeer { .. }), "{err}");
    }

    #[tokio::test]
    async fn client_rejects_a_ui_as_agent() {
        let (client, server) = duplex(64);
        tokio::spawn(async move {
            let mut s = Framed::new(server);
            s.send(&Hello::new(Peer::Ui)).await.unwrap();
            let _ = s.recv_hello().await;
        });
        let err = IpcClient::handshake(client).await.unwrap_err();
        assert!(matches!(err, IpcError::UnexpectedPeer { .. }), "{err}");
    }

    #[tokio::test]
    async fn shutdown_ends_the_connection() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (client, server) = duplex(64);
        let (stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(serve_connection(server, agent, stop_rx));
        let mut c = IpcClient::handshake(client).await.unwrap();
        c.get_state().await.unwrap();
        stop.send_replace(true);
        task.await.unwrap().unwrap();
        assert!(matches!(c.next_event().await, Ok(None)));
    }

    // QA round 1: a UI can send several requests without waiting for a reply; each
    // reply carries its id and they come out in order.
    #[tokio::test]
    async fn pipelined_requests_are_answered_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = Framed::new(io);
        c.send(&Hello::new(Peer::Ui)).await.unwrap();
        assert_eq!(c.recv_hello().await.unwrap().peer, Peer::Agent(Machine::A));
        let ids = [7u64, 3, u64::MAX, 3];
        for id in ids {
            c.send(&UiRequest {
                id,
                command: UiCommand::GetState,
            })
            .await
            .unwrap();
        }
        for id in ids {
            assert_eq!(
                c.recv::<AgentToUi>().await.unwrap(),
                Some(AgentToUi::Reply {
                    id,
                    reply: Reply::State(agent.state())
                })
            );
        }
    }

    #[tokio::test]
    async fn garbage_request_ends_only_that_connection() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (stop, stop_rx) = watch::channel(false);
        let (bad_client, bad_server) = duplex(64);
        let bad = tokio::spawn(serve_connection(bad_server, agent.clone(), stop_rx.clone()));
        let (good_client, good_server) = duplex(64);
        tokio::spawn(serve_connection(good_server, agent.clone(), stop_rx));

        let mut b = Framed::new(bad_client);
        b.send(&Hello::new(Peer::Ui)).await.unwrap();
        b.recv_hello().await.unwrap();
        // A well-formed frame whose content is not a `UiRequest`.
        b.io.write_all(&[3, 0, 0, 0, 0xFF, 0xFF, 0xFF])
            .await
            .unwrap();
        let err = bad.await.unwrap().unwrap_err();
        assert!(
            matches!(err, IpcError::Proto(ProtoError::Decode(_))),
            "{err}"
        );

        // The other UI is still served.
        let mut g = IpcClient::handshake(good_client).await.unwrap();
        assert_eq!(g.get_state().await.unwrap(), agent.state());
        drop(stop);
    }

    #[tokio::test]
    async fn oversized_frame_from_ui_is_rejected_without_reading_it() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (client, server) = duplex(64);
        let (_stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(serve_connection(server, agent, stop_rx));
        let mut c = Framed::new(client);
        c.send(&Hello::new(Peer::Ui)).await.unwrap();
        c.recv_hello().await.unwrap();
        // Only the header: it announces 4 GiB and the agent cuts without waiting for the body.
        c.io.write_all(&u32::MAX.to_le_bytes()).await.unwrap();
        let err = task.await.unwrap().unwrap_err();
        assert!(
            matches!(err, IpcError::Proto(ProtoError::FrameTooLarge { .. })),
            "{err}"
        );
    }

    #[tokio::test]
    async fn request_before_hello_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (client, server) = duplex(64);
        let (_stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(serve_connection(server, agent, stop_rx));
        let mut c = Framed::new(client);
        c.recv_hello().await.unwrap();
        // A request instead of the greeting: it is not served.
        c.send(&UiRequest {
            id: 0,
            command: UiCommand::GetState,
        })
        .await
        .unwrap();
        assert!(task.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn clean_close_between_messages_is_none() {
        let (a, b) = duplex(64);
        drop(b);
        let mut f = Framed::new(a);
        assert!(matches!(f.recv::<UiRequest>().await, Ok(None)));
    }

    #[tokio::test]
    async fn close_mid_frame_is_an_error() {
        let (a, mut b) = duplex(64);
        b.write_all(&[10, 0, 0, 0, 1]).await.unwrap();
        drop(b);
        let mut f = Framed::new(a);
        assert!(matches!(f.recv::<UiRequest>().await, Err(IpcError::Io(_))));
    }

    #[tokio::test]
    async fn hello_on_closed_stream_is_closed() {
        let (a, b) = duplex(64);
        drop(b);
        let mut f = Framed::new(a);
        assert!(matches!(f.recv_hello().await, Err(IpcError::Closed)));
    }

    #[tokio::test]
    async fn request_on_closed_connection_fails() {
        let (client, server) = duplex(64);
        tokio::spawn(async move {
            let mut s = Framed::new(server);
            s.send(&Hello::new(Peer::Agent(Machine::B))).await.unwrap();
            let _ = s.recv_hello().await;
            let _ = s.recv::<UiRequest>().await;
        });
        let mut c = IpcClient::handshake(client).await.unwrap();
        assert!(matches!(c.get_state().await, Err(IpcError::Closed)));
    }
}
