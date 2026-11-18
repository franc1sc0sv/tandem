//! The other agent over the control plane: asking it for steps ([`PeerClient`]) and serving
//! the ones it asks for ([`serve`]).
//!
//! Each agent opens its own request stream toward the other and serves all the
//! streams the other opens, so either of the two can orchestrate. The
//! served streams also answer pings: they work just the same for the one that opens the
//! link manager.
//!
//! The logic goes over [`ControlChannel`]: in the agent it is the QUIC control stream
//! and in tests it can be an in-memory stream.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tandem_core::{Machine, State, Step};
use tandem_input::session::CursorEvent;
use tandem_link::manager::{LinkKind, Snapshot};
use tandem_link::qos::StreamClass;
use tandem_link::transport::{Connection, ControlStream, TransportError};
use tandem_proto::net::MEASURE_SECS;
use tandem_proto::{Channel, ControlMsg, Hello, Peer, ProtoError};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::agent::{Agent, IncomingChannel};
use crate::ipc::Framed;
use crate::orchestrator::{PEER_LOST, PeerLink, StepFuture};

/// Control messages in both directions.
pub trait ControlChannel: Send + 'static {
    fn send(&mut self, msg: &ControlMsg) -> impl Future<Output = Result<(), String>> + Send;

    /// `None` if the other side closed. It must be cancel-safe without losing messages.
    fn recv(&mut self) -> impl Future<Output = Result<Option<ControlMsg>, String>> + Send;
}

impl ControlChannel for ControlStream {
    async fn send(&mut self, msg: &ControlMsg) -> Result<(), String> {
        ControlStream::send(self, msg)
            .await
            .map_err(|e| e.to_string())
    }

    async fn recv(&mut self) -> Result<Option<ControlMsg>, String> {
        ControlStream::recv(self).await.map_err(|e| e.to_string())
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send + 'static> ControlChannel for Framed<S> {
    async fn send(&mut self, msg: &ControlMsg) -> Result<(), String> {
        Framed::send(self, msg).await.map_err(|e| e.to_string())
    }

    async fn recv(&mut self) -> Result<Option<ControlMsg>, String> {
        Framed::recv(self).await.map_err(|e| e.to_string())
    }
}

enum Outgoing {
    Step(Step, oneshot::Sender<Result<(), String>>),
    Msg(ControlMsg),
    /// Notifies when everything before it has already gone out over the stream ([`PeerClient::flush`]).
    Flush(oneshot::Sender<()>),
    /// Like `Msg`, but counted in [`PeerClient::lossy`]: it is discarded if the stream lags.
    Lossy(ControlMsg),
}

/// How many discardable notices wait at once in a [`PeerClient`] before it starts
/// dropping the new ones. At 30 samples per second, two pending are about 70 ms of lag.
pub const LOSSY_BACKLOG: usize = 2;

/// Asks the other agent for steps over a stream of its own. Dropping it closes the stream.
pub struct PeerClient {
    tx: mpsc::UnboundedSender<Outgoing>,
    /// Discardable notices in the queue (the live cursor, T-128).
    lossy: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl PeerClient {
    /// Takes the channel and serves it in a task of its own. Call inside a tokio runtime.
    pub fn spawn<C: ControlChannel>(chan: C) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let lossy = Arc::new(AtomicUsize::new(0));
        PeerClient {
            tx,
            task: tokio::spawn(client_loop(chan, rx, lossy.clone())),
            lossy,
        }
    }

    /// Opens the request stream over an already authenticated QUIC connection.
    pub async fn open(conn: &Connection, me: Machine) -> Result<Self, PeerError> {
        let (stream, theirs) = conn.open_control(&Hello::new(Peer::Agent(me))).await?;
        check_peer(me, theirs.peer)?;
        Ok(Self::spawn(stream))
    }
}

impl Drop for PeerClient {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl PeerLink for PeerClient {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
        Box::pin(async move {
            let (reply, result) = oneshot::channel();
            self.tx
                .send(Outgoing::Step(step.clone(), reply))
                .map_err(|_| PEER_LOST.to_owned())?;
            result.await.map_err(|_| PEER_LOST.to_owned())?
        })
    }

    fn publish_state(&self, state: &State) {
        self.send(ControlMsg::State(state.clone()));
    }

    fn send(&self, msg: ControlMsg) {
        PeerClient::send(self, msg);
    }

    fn send_lossy(&self, msg: ControlMsg) {
        PeerClient::send_lossy(self, msg);
    }
}

impl PeerClient {
    /// Waits for what was sent so far to have gone out over the stream (not for the other to
    /// read it): to notify something right before closing the connection.
    pub async fn flush(&self) {
        let (done, sent) = oneshot::channel();
        if self.tx.send(Outgoing::Flush(done)).is_ok() {
            let _ = sent.await;
        }
    }

    /// Sends a notice without a reply (state, hub, screens). If the stream already closed
    /// it is lost: whoever notifies repeats it when the next link appears.
    pub fn send(&self, msg: ControlMsg) {
        let _ = self.tx.send(Outgoing::Msg(msg));
    }

    /// Like [`PeerClient::send`] for what replaces itself (the cursor samples): if
    /// [`LOSSY_BACKLOG`] such notices already wait because the stream lagged, this one is discarded
    /// instead of accumulating.
    pub fn send_lossy(&self, msg: ControlMsg) {
        if self.lossy.fetch_add(1, Ordering::AcqRel) >= LOSSY_BACKLOG {
            self.lossy.fetch_sub(1, Ordering::AcqRel);
            return;
        }
        if self.tx.send(Outgoing::Lossy(msg)).is_err() {
            self.lossy.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

async fn client_loop<C: ControlChannel>(
    mut chan: C,
    mut rx: mpsc::UnboundedReceiver<Outgoing>,
    lossy: Arc<AtomicUsize>,
) {
    // On exit the pending replies are dropped: whoever waits sees [`PEER_LOST`].
    let mut pending: HashMap<u64, oneshot::Sender<Result<(), String>>> = HashMap::new();
    let mut next_id = 0u64;
    loop {
        // Sending goes outside the `select!`: cutting it midway would misalign the stream.
        let out = tokio::select! {
            out = rx.recv() => match out {
                Some(out) => out,
                None => return,
            },
            msg = chan.recv() => {
                match msg {
                    Ok(Some(ControlMsg::StepResult { id, outcome })) => {
                        // If whoever asked already got tired of waiting, nobody listens.
                        if let Some(reply) = pending.remove(&id) {
                            let _ = reply.send(outcome.into());
                        }
                    }
                    Ok(Some(ControlMsg::Ping { nonce })) => {
                        if let Err(e) = chan.send(&ControlMsg::Pong { nonce }).await {
                            tracing::debug!("request stream to the other agent: {e}");
                            return;
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => return,
                    Err(e) => {
                        tracing::debug!("request stream to the other agent: {e}");
                        return;
                    }
                }
                continue;
            }
        };
        let sent = match out {
            Outgoing::Step(step, reply) => {
                let id = next_id;
                next_id = next_id.wrapping_add(1);
                pending.retain(|_, r| !r.is_closed());
                pending.insert(id, reply);
                chan.send(&ControlMsg::ExecuteStep { id, step }).await
            }
            Outgoing::Msg(msg) => chan.send(&msg).await,
            Outgoing::Flush(done) => {
                let _ = done.send(());
                Ok(())
            }
            Outgoing::Lossy(msg) => {
                // It leaves the queue: the one arriving now no longer waits behind this one.
                lossy.fetch_sub(1, Ordering::AcqRel);
                chan.send(&msg).await
            }
        };
        if let Err(e) = sent {
            tracing::debug!("request stream to the other agent: {e}");
            return;
        }
    }
}

/// A request to the other agent that travels over link `kind`: fails with [`PEER_LOST`]
/// as soon as the link manager considers that link down (DESIGN §6: without waiting
/// for long timeouts). The drop is noticed in a ping, about 1.25 s; the step's
/// maximum time is 10 s. This way the transaction undoes (or continues) right away over the next
/// link.
pub async fn over_link<H, F, T>(
    mut links: watch::Receiver<Snapshot<H>>,
    kind: LinkKind,
    request: F,
) -> Result<T, String>
where
    F: Future<Output = Result<T, String>>,
{
    // The manager takes at least a ping to consider the link healthy again, so
    // the snapshot with the link down is not lost between two reads.
    let lost = links.wait_for(|s| !s.links.iter().any(|l| l.kind == kind && l.rtt.is_some()));
    tokio::select! {
        // If the reply already arrived, it holds even if the link dropped afterwards.
        biased;
        r = request => r,
        // If the manager closed there is no link either.
        _ = lost => Err(PEER_LOST.to_owned()),
    }
}

/// Serves a stream opened by the other agent until it closes it.
///
/// Steps run one at a time and in order: this way the inverse of a step that timed out
/// on the other side never runs before the step itself. If the inverse arrives over another
/// stream (the link dropped), the orchestrator's turn makes it wait all the same.
pub async fn serve<C: ControlChannel>(chan: C, agent: Arc<Agent>) -> Result<(), String> {
    serve_from(chan, agent, None).await
}

async fn serve_from<C: ControlChannel>(
    mut chan: C,
    agent: Arc<Agent>,
    mut first: Option<ControlMsg>,
) -> Result<(), String> {
    loop {
        let msg = match first.take() {
            Some(msg) => msg,
            None => match chan.recv().await? {
                Some(msg) => msg,
                None => return Ok(()),
            },
        };
        respond(&mut chan, &agent, msg).await?;
    }
}

async fn respond<C: ControlChannel>(
    chan: &mut C,
    agent: &Arc<Agent>,
    msg: ControlMsg,
) -> Result<(), String> {
    {
        match msg {
            ControlMsg::ExecuteStep { id, step } => {
                let outcome = agent.orchestrator().execute_for_peer(&step).await;
                chan.send(&ControlMsg::StepResult {
                    id,
                    outcome: outcome.into(),
                })
                .await?;
            }
            ControlMsg::Ping { nonce } => chan.send(&ControlMsg::Pong { nonce }).await?,
            ControlMsg::GetState => chan.send(&ControlMsg::State(agent.state())).await?,
            // The final state of a transaction of the other agent.
            ControlMsg::State(state) => {
                agent.orchestrator().release_peer_lease();
                if let Err(e) = agent.set_state(state) {
                    tracing::warn!("could not save the state the other agent sent: {e}");
                }
            }
            ControlMsg::Hub(report) => {
                agent.inbox().hub.send_replace(Some(report));
            }
            ControlMsg::Screens(screens) => {
                agent.inbox().screens.send_replace(Some(screens));
            }
            ControlMsg::Capabilities(caps) => agent.set_peer_capabilities(*caps),
            ControlMsg::Desk(sync) => agent.receive_desk(*sync),
            ControlMsg::KeyProbe(msg) => agent.receive_key_probe(msg),
            ControlMsg::Keyboard(machine) => agent.receive_keyboard(machine),
            ControlMsg::Monitors(reports) => agent.receive_monitor_reports(reports),
            ControlMsg::WatchResources { on } => agent.receive_watch_resources(on),
            ControlMsg::Resources(r) => agent.receive_peer_resources(*r),
            ControlMsg::Rescan => agent.receive_rescan(),
            ControlMsg::Unpaired => agent.receive_unpaired(),
            ControlMsg::Update(msg) => agent.receive_update(msg),
            ControlMsg::Focus(msg) => agent.inbox().deliver_focus(msg),
            ControlMsg::Drag(msg) => agent.drag().deliver(msg),
            ControlMsg::WatchCursor { on } => agent.receive_watch_cursor(on),
            ControlMsg::Cursor(sample) => agent.receive_peer_cursor(CursorEvent::Sample(sample)),
            ControlMsg::Crossed { from, to, edge } => {
                agent.receive_peer_cursor(CursorEvent::Crossed { from, to, edge });
            }
            // It only counts as a first message ([`accept_streams`]).
            ControlMsg::Open(channel) => {
                return Err(format!(
                    "channel {channel:?} opened midway through a stream"
                ));
            }
            // Replies belong to the requester.
            // Served in `serve_stream`, which has the connection.
            ControlMsg::LinkClass { .. } => {}
            ControlMsg::Pong { .. } | ControlMsg::StepResult { .. } => {}
        }
    }
    Ok(())
}

/// Opens a stream of its own of `channel` over a connection with the other agent
/// ([`ControlMsg::Open`]); the other side delivers it with [`crate::agent::PeerInbox`].
/// Each channel has its priority ([`StreamClass::of`]): keys do not wait behind
/// a clipboard image.
pub async fn open_channel(
    conn: &Connection,
    me: Machine,
    channel: Channel,
) -> Result<ControlStream, PeerError> {
    let (mut stream, theirs) = conn.open_control(&Hello::new(Peer::Agent(me))).await?;
    stream.set_class(StreamClass::of(channel))?;
    check_peer(me, theirs.peer)?;
    stream.send(&ControlMsg::Open(channel)).await?;
    Ok(stream)
}

/// Serves each stream the other agent opens on `conn`, until the connection closes.
pub async fn accept_streams(conn: Connection, agent: Arc<Agent>) {
    let me = agent.machine();
    let hello = Hello::new(Peer::Agent(me));
    loop {
        let (stream, theirs) = match conn.accept_control(&hello).await {
            Ok(s) => s,
            // Another version (for example, an older one without a capabilities announcement): it is not
            // spoken to, and the log says why and what to do.
            Err(TransportError::Proto(e @ ProtoError::IncompatibleVersion { theirs, .. })) => {
                tracing::warn!("rejecting the other agent: {e}");
                agent.peer_protocol_mismatch(theirs);
                return;
            }
            Err(e) => {
                tracing::debug!("connection with the other agent ended: {e}");
                // Its classification holds per connection: the next one brings its own.
                conn.qos().clear_peer_link_of(conn.id());
                return;
            }
        };
        if let Err(e) = check_peer(me, theirs.peer) {
            tracing::warn!("{e}");
            continue;
        }
        let (agent, conn) = (agent.clone(), conn.clone());
        tokio::spawn(async move {
            if let Err(e) = serve_stream(stream, conn, agent).await {
                tracing::debug!("stream from the other agent ended: {e}");
            }
        });
    }
}

/// A stream the other agent opened: one of a channel of its own is delivered whole to the input
/// engine; any other is for requests (or pings) and is served here.
async fn serve_stream(
    mut stream: ControlStream,
    conn: Connection,
    agent: Arc<Agent>,
) -> Result<(), String> {
    let mut first = match stream.recv::<ControlMsg>().await {
        Ok(Some(first)) => first,
        Ok(None) => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    // The other side says how it classifies this connection before anything else (T-094).
    if let ControlMsg::LinkClass { kind } = first {
        apply_link_class(&conn, kind.into());
        first = match stream.recv::<ControlMsg>().await {
            Ok(Some(next)) => next,
            Ok(None) => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
    }
    if let ControlMsg::Open(Channel::Tunnel) = first {
        // Tunnels do not go to the input engine: their module serves them.
        return crate::tunnels::serve(stream, &conn, &agent).await;
    }
    if first == ControlMsg::Open(Channel::Measure) {
        return answer_measure(stream).await;
    }
    if let ControlMsg::Open(channel) = first {
        // The priority belongs to whoever writes: what this side sends over the channel too.
        if let Err(e) = stream.set_class(StreamClass::of(channel)) {
            return Err(e.to_string());
        }
        agent.inbox().deliver(IncomingChannel {
            channel,
            conn,
            stream,
        });
        return Ok(());
    }
    serve_from(stream, agent, Some(first)).await
}

/// Stores the other end's classification in the path's `Qos`, which computes the effective
/// label (the worse of the two). It is stored apart from its own, so it recovers when
/// the other announces a cable again, and is not lost if its own is not yet known.
fn apply_link_class(conn: &Connection, theirs: LinkKind) {
    let qos = conn.qos();
    let before = qos.link();
    qos.set_peer_link_from(conn.id(), theirs);
    let label = qos.link();
    if label != before {
        tracing::info!(theirs = %theirs, ?label, "the other end sees the link as worse: new label");
    }
}

/// The most a bandwidth test can take, end to end: the test and
/// the time it takes for what was left in flight to drain.
pub const MEASURE_TIMEOUT: Duration = Duration::from_secs(MEASURE_SECS + 10);

/// Measures the bandwidth of `conn` toward the other agent (T-102): uncapped data for
/// [`MEASURE_SECS`] over a data stream; the other side counts. In bytes per second. It is
/// cut if the path's `Qos` pauses the data.
pub async fn measure(conn: &Connection, me: Machine) -> Result<u64, String> {
    let run = async {
        let stream = open_channel(conn, me, Channel::Measure)
            .await
            .map_err(|e| e.to_string())?;
        let report = stream
            .measure(Duration::from_secs(MEASURE_SECS), conn.qos())
            .await
            .map_err(|e| e.to_string())?;
        report
            .bytes_per_sec()
            .ok_or_else(|| "nothing arrived to measure".to_owned())
    };
    tokio::time::timeout(MEASURE_TIMEOUT, run)
        .await
        .map_err(|_| "the test did not finish in time".to_owned())?
}

/// The other side of [`measure`].
async fn answer_measure(stream: ControlStream) -> Result<(), String> {
    match tokio::time::timeout(MEASURE_TIMEOUT, stream.answer_measure()).await {
        Ok(Ok(report)) => {
            tracing::debug!(?report, "bandwidth test answered");
            Ok(())
        }
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("the bandwidth test did not finish in time".to_owned()),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("expected the agent of {expected}, it greeted {got:?}")]
    UnexpectedPeer { expected: Machine, got: Peer },
}

/// The other side has to be the other machine's agent.
fn check_peer(me: Machine, got: Peer) -> Result<(), PeerError> {
    if got == Peer::Agent(me.other()) {
        Ok(())
    } else {
        Err(PeerError::UnexpectedPeer {
            expected: me.other(),
            got,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tandem_core::Plan;
    use tokio::io::duplex;

    use super::*;
    use crate::config::fixtures;
    use crate::orchestrator::tests::FakeSteps;
    use crate::store::StateStore;

    fn agent(
        dir: &std::path::Path,
        machine: Machine,
        steps: FakeSteps,
    ) -> (Arc<Agent>, Arc<FakeSteps>) {
        let mut cfg = fixtures::desk();
        cfg.machine = machine;
        let steps = Arc::new(steps);
        let a = Agent::with_local_steps(
            cfg,
            StateStore::new(dir.join(format!("{machine}.json"))),
            steps.clone(),
        );
        (Arc::new(a), steps)
    }

    /// The Mac asks the PC for steps over an in-memory stream.
    fn link(mac: &Arc<Agent>, pc: &Arc<Agent>) -> JoinHandle<Result<(), String>> {
        let (a, b) = duplex(64 * 1024);
        mac.set_peer(Arc::new(PeerClient::spawn(Framed::new(a))));
        tokio::spawn(serve(Framed::new(b), pc.clone()))
    }

    /// A step that falls to the PC.
    fn pc_off() -> Step {
        Step::PowerOff {
            machine: Machine::B,
        }
    }

    #[tokio::test]
    async fn remote_step_runs_on_the_peer() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, mac_steps) = agent(dir.path(), Machine::A, FakeSteps::default());
        let (pc, pc_steps) = agent(dir.path(), Machine::B, FakeSteps::default());
        let _serve = link(&mac, &pc);
        mac.orchestrator().dispatch(&pc_off()).await.unwrap();
        assert_eq!(pc_steps.steps(), [pc_off()]);
        assert!(mac_steps.steps().is_empty());
    }

    #[tokio::test]
    async fn published_state_reaches_the_peer() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, _) = agent(dir.path(), Machine::A, FakeSteps::default());
        let (pc, _) = agent(dir.path(), Machine::B, FakeSteps::default());
        let _serve = link(&mac, &pc);
        // The Mac's: it is on (the PC already was for itself).
        let mut s = pc.state();
        s.on.a = true;
        let mut rx = pc.subscribe();
        mac.orchestrator().peer().publish_state(&s);
        rx.wait_for(|got| *got == s).await.unwrap();
    }

    #[tokio::test]
    async fn closed_stream_fails_pending_and_new_steps() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, _) = agent(dir.path(), Machine::A, FakeSteps::default());
        let (a, b) = duplex(1024);
        mac.set_peer(Arc::new(PeerClient::spawn(Framed::new(a))));
        // The other side reads the request and closes without answering.
        tokio::spawn(async move {
            let mut f = Framed::new(b);
            let _ = f.recv::<ControlMsg>().await;
        });
        let peer = mac.orchestrator().peer();
        assert_eq!(peer.execute(&pc_off()).await.unwrap_err(), PEER_LOST);
        assert_eq!(peer.execute(&pc_off()).await.unwrap_err(), PEER_LOST);
    }

    #[tokio::test]
    async fn serve_answers_ping_and_state() {
        let dir = tempfile::tempdir().unwrap();
        let (pc, _) = agent(dir.path(), Machine::B, FakeSteps::default());
        let (a, b) = duplex(64 * 1024);
        let task = tokio::spawn(serve(Framed::new(b), pc.clone()));
        let mut f = Framed::new(a);
        f.send(&ControlMsg::Ping { nonce: 5 }).await.unwrap();
        assert_eq!(f.recv().await.unwrap(), Some(ControlMsg::Pong { nonce: 5 }));
        f.send(&ControlMsg::GetState).await.unwrap();
        assert_eq!(f.recv().await.unwrap(), Some(ControlMsg::State(pc.state())));
        // A step that does not fall to the PC is rejected without being executed.
        f.send(&ControlMsg::ExecuteStep {
            id: 1,
            step: Step::PowerOn {
                machine: Machine::B,
            },
        })
        .await
        .unwrap();
        assert!(matches!(
            f.recv().await.unwrap(),
            Some(ControlMsg::StepResult {
                id: 1,
                outcome: tandem_proto::StepOutcome::Failed(_)
            })
        ));
        drop(f);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn serve_keeps_the_peers_input_reports() {
        let dir = tempfile::tempdir().unwrap();
        let (pc, _) = agent(dir.path(), Machine::B, FakeSteps::default());
        let (a, b) = duplex(64 * 1024);
        let task = tokio::spawn(serve(Framed::new(b), pc.clone()));
        let client = PeerClient::spawn(Framed::new(a));
        let report = tandem_proto::HubReport {
            hub: true,
            server: Machine::A,
        };
        let screens = vec![tandem_proto::ScreenRect {
            id: "mac-1".into(),
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            scale_milli: 1000,
        }];
        let (mut hub, mut scr) = (pc.inbox().hub.subscribe(), pc.inbox().screens.subscribe());
        client.send(ControlMsg::Hub(report));
        client.send(ControlMsg::Screens(screens.clone()));
        hub.wait_for(|h| *h == Some(report)).await.unwrap();
        scr.wait_for(|s| s.as_ref() == Some(&screens))
            .await
            .unwrap();
        // Opening a channel midway through the request stream cuts it.
        client.send(ControlMsg::Open(Channel::Input));
        assert!(task.await.unwrap().is_err());
    }

    #[test]
    fn peer_must_be_the_other_machine() {
        assert!(check_peer(Machine::A, Peer::Agent(Machine::B)).is_ok());
        assert!(check_peer(Machine::A, Peer::Agent(Machine::A)).is_err());
        assert!(check_peer(Machine::B, Peer::Ui).is_err());
    }

    /// The remote shutdown hangs on the PC: the Mac takes it as done after 10 s (most
    /// likely it is shutting down).
    #[tokio::test(start_paused = true)]
    async fn a_remote_power_off_that_hangs_is_taken_as_done() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, _) = agent(dir.path(), Machine::A, FakeSteps::default());
        let (pc, pc_steps) = agent(
            dir.path(),
            Machine::B,
            FakeSteps {
                delays: vec![(pc_off(), Duration::from_secs(3600))],
                ..Default::default()
            },
        );
        let _serve = link(&mac, &pc);
        let mut state = mac.state();
        state.on.b = true;
        let plan = Plan::power_off(&state, Machine::B);
        let start = tokio::time::Instant::now();
        mac.orchestrator().run(1, &mut state, &plan).await.unwrap();
        assert_eq!(start.elapsed(), Duration::from_secs(10));
        assert!(!state.on.b);
        assert_eq!(pc_steps.steps(), [pc_off()]);
    }

    /// QA round 2: while the PC does steps for the Mac it cannot start a change
    /// of its own; the Mac's final state frees it.
    #[tokio::test]
    async fn the_peers_final_state_ends_the_loan() {
        let dir = tempfile::tempdir().unwrap();
        let (pc, pc_steps) = agent(dir.path(), Machine::B, FakeSteps::default());
        let (a, b) = duplex(64 * 1024);
        let _task = tokio::spawn(serve(Framed::new(b), pc.clone()));
        let mut f = Framed::new(a);
        f.send(&ControlMsg::ExecuteStep {
            id: 1,
            step: pc_off(),
        })
        .await
        .unwrap();
        assert!(matches!(
            f.recv().await.unwrap(),
            Some(ControlMsg::StepResult {
                id: 1,
                outcome: tandem_proto::StepOutcome::Done
            })
        ));
        assert_eq!(pc_steps.steps(), [pc_off()]);
        assert_eq!(pc.orchestrator().try_begin(), None);

        // The Mac's final state: it is on (the PC already was for itself).
        let mut s = pc.state();
        assert!(s.is_on(Machine::B) && !s.is_on(Machine::A));
        s.on.a = true;
        let mut rx = pc.subscribe();
        f.send(&ControlMsg::State(s.clone())).await.unwrap();
        rx.wait_for(|got| *got == s).await.unwrap();
        assert!(pc.orchestrator().try_begin().is_some());
    }

    fn snapshot(up: &[LinkKind]) -> Snapshot<()> {
        Snapshot {
            active: None,
            links: [LinkKind::Usb4, LinkKind::Ethernet]
                .into_iter()
                .map(|kind| tandem_link::manager::LinkStatus {
                    kind,
                    addr: std::net::SocketAddr::from(([127, 0, 0, 1], 1)),
                    rtt: up.contains(&kind).then_some(Duration::from_millis(1)),
                })
                .collect(),
            handles: up.iter().map(|k| (*k, ())).collect(),
        }
    }

    /// T-064: the request does not wait for its maximum time if the manager considers its link down.
    #[tokio::test(start_paused = true)]
    async fn request_fails_when_its_link_goes_down() {
        let (tx, rx) = watch::channel(snapshot(&[LinkKind::Usb4, LinkKind::Ethernet]));
        let start = tokio::time::Instant::now();
        let request = over_link(
            rx,
            LinkKind::Usb4,
            std::future::pending::<Result<(), String>>(),
        );
        let cut = async {
            tokio::time::sleep(Duration::from_millis(1250)).await;
            // Another link dropping does not affect the request.
            tx.send_replace(snapshot(&[LinkKind::Usb4]));
            tokio::time::sleep(Duration::from_millis(100)).await;
            tx.send_replace(snapshot(&[LinkKind::Ethernet]));
            std::future::pending::<()>().await;
        };
        let r = tokio::select! {
            r = request => r,
            () = cut => unreachable!(),
        };
        assert_eq!(r.unwrap_err(), PEER_LOST);
        assert_eq!(start.elapsed(), Duration::from_millis(1350));
    }

    #[tokio::test(start_paused = true)]
    async fn request_answered_before_the_drop_keeps_its_result() {
        let (tx, rx) = watch::channel(snapshot(&[LinkKind::Usb4]));
        assert_eq!(
            over_link(rx.clone(), LinkKind::Usb4, async { Ok(()) }).await,
            Ok(())
        );
        assert_eq!(
            over_link(rx, LinkKind::Usb4, async {
                Err::<(), _>("fails".to_owned())
            })
            .await,
            Err("fails".to_owned())
        );
        // With the link already down there is nobody to ask anything of.
        tx.send_replace(snapshot(&[]));
        assert_eq!(
            over_link(
                tx.subscribe(),
                LinkKind::Usb4,
                std::future::pending::<Result<(), String>>()
            )
            .await,
            Err(PEER_LOST.to_owned())
        );
    }

    /// If the manager closes, the request is not left waiting either.
    #[tokio::test(start_paused = true)]
    async fn request_fails_when_the_manager_closes() {
        let (tx, rx) = watch::channel(snapshot(&[LinkKind::Usb4]));
        drop(tx);
        assert_eq!(
            over_link(
                rx,
                LinkKind::Usb4,
                std::future::pending::<Result<(), String>>()
            )
            .await,
            Err(PEER_LOST.to_owned())
        );
    }

    /// A channel that stalls until released and records what it manages to send.
    struct Stalled {
        open: Arc<tokio::sync::Semaphore>,
        sent: Arc<std::sync::Mutex<Vec<ControlMsg>>>,
    }

    impl ControlChannel for Stalled {
        async fn send(&mut self, msg: &ControlMsg) -> Result<(), String> {
            let _ = self.open.acquire().await.map_err(|e| e.to_string())?;
            self.sent.lock().unwrap().push(msg.clone());
            Ok(())
        }

        async fn recv(&mut self) -> Result<Option<ControlMsg>, String> {
            std::future::pending().await
        }
    }

    /// The live cursor (T-128) does not accumulate if the stream lags: the new ones are discarded
    /// and what matters (a crossing) continues over the usual path.
    #[tokio::test]
    async fn lossy_messages_are_dropped_while_the_stream_is_backed_up() {
        let open = Arc::new(tokio::sync::Semaphore::new(0));
        let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
        let client = PeerClient::spawn(Stalled {
            open: open.clone(),
            sent: sent.clone(),
        });
        let sample = |x: i32| {
            ControlMsg::Cursor(tandem_proto::CursorSample {
                machine: Machine::A,
                screen: String::new(),
                x,
                y: 0,
            })
        };
        // The first is taken by the task (it stays stuck sending); the next ones wait.
        client.send_lossy(sample(0));
        tokio::time::sleep(Duration::from_millis(50)).await;
        for x in 1..=20 {
            client.send_lossy(sample(x));
        }
        client.send(ControlMsg::Rescan);
        open.add_permits(1000);
        tokio::time::timeout(Duration::from_secs(5), async {
            while sent.lock().unwrap().len() < LOSSY_BACKLOG + 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let got = sent.lock().unwrap().clone();
        // The one in flight, the two that fit in the queue and the normal notice.
        assert_eq!(
            got,
            vec![sample(0), sample(1), sample(2), ControlMsg::Rescan],
            "{got:?}"
        );
        // With the queue empty again, it accepts again.
        client.send_lossy(sample(99));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(sent.lock().unwrap().last(), Some(&sample(99)));
    }
}
