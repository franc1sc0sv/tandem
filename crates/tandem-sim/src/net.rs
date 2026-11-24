//! The simulated network between the two agents: one link per type (USB4, Ethernet, Wi-Fi)
//! with in-memory streams that can be cut.
//!
//! Over each link runs `tandemd`'s real control protocol (`PeerClient` and
//! `peer::serve`) and `tandem-link`'s real link manager chooses which one the
//! steps go over. A cut imitates an unplugged cable: nothing fails, there is just
//! silence, and streams opened over that link stay dead even if the cable comes back.

use std::collections::BTreeMap;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use tandem_core::{Machine, State, Step};
use tandem_link::manager::{LinkCandidate, LinkKind, LinkManager, Prober};
use tandem_proto::ControlMsg;
use tandemd::ipc::Framed;
use tandemd::orchestrator::{LinkHold, PEER_NOT_CONNECTED, PeerLink, StepFuture};
use tandemd::{Agent, PeerClient, peer};
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};

use crate::world::{Power, World};

/// Size of each in-memory stream's buffer.
const WIRE_BUFFER: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
struct LinkWire {
    addr: SocketAddr,
    up: bool,
    /// Changes with every cut: streams of an earlier generation stay dead.
    generation: u64,
}

#[derive(Debug)]
struct NetInner {
    links: BTreeMap<LinkKind, LinkWire>,
}

/// The physical links between the two machines.
#[derive(Debug, Clone)]
pub struct SimNet {
    inner: Arc<Mutex<NetInner>>,
    world: World,
}

/// Simulated address of the other agent for each link (those of DESIGN §6).
pub fn link_addr(kind: LinkKind) -> SocketAddr {
    let net = match kind {
        LinkKind::Usb4 => 1,
        LinkKind::Ethernet => 2,
        LinkKind::Wifi => 3,
    };
    SocketAddr::from(([10, 77, net, 2], tandemd::config::DEFAULT_PORT))
}

impl SimNet {
    /// The given links, all with the cable plugged in.
    pub fn new(world: World, kinds: &[LinkKind]) -> Self {
        let links = kinds
            .iter()
            .map(|&k| {
                (
                    k,
                    LinkWire {
                        addr: link_addr(k),
                        up: true,
                        generation: 0,
                    },
                )
            })
            .collect();
        SimNet {
            inner: Arc::new(Mutex::new(NetInner { links })),
            world,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, NetInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn candidates(&self) -> Vec<LinkCandidate> {
        self.lock()
            .links
            .iter()
            .map(|(&kind, l)| LinkCandidate { kind, addr: l.addr })
            .collect()
    }

    /// Unplugs a link: the streams opened over it do not work again.
    pub fn cut(&self, kind: LinkKind) {
        if let Some(l) = self.lock().links.get_mut(&kind) {
            l.up = false;
            l.generation += 1;
        }
        tracing::info!(link = %kind, "link cut (simulated)");
    }

    /// Plugs in a cable that was not there (or the one that was cut): the link exists and answers.
    pub fn plug(&self, kind: LinkKind) {
        let mut inner = self.lock();
        let wire = inner.links.entry(kind).or_insert(LinkWire {
            addr: link_addr(kind),
            up: true,
            generation: 0,
        });
        wire.up = true;
    }

    /// Plugs a link back in: it works for new connections.
    pub fn restore(&self, kind: LinkKind) {
        if let Some(l) = self.lock().links.get_mut(&kind) {
            l.up = true;
        }
    }

    /// A machine shut down: all connections with its agent die.
    pub(crate) fn drop_all(&self) {
        for l in self.lock().links.values_mut() {
            l.generation += 1;
        }
    }

    pub fn is_up(&self, kind: LinkKind) -> bool {
        self.lock().links.get(&kind).is_some_and(|l| l.up)
    }

    /// Whether a new connection over `kind` would reach the other agent, and with what generation.
    fn reachable(&self, kind: LinkKind) -> Option<u64> {
        let link = *self.lock().links.get(&kind)?;
        let both = [Machine::A, Machine::B]
            .iter()
            .all(|m| self.world.power_of(*m) == Power::On);
        (link.up && both).then_some(link.generation)
    }

    fn alive(&self, kind: LinkKind, generation: u64) -> bool {
        self.reachable(kind) == Some(generation)
    }

    fn kind_of(&self, addr: SocketAddr) -> Option<LinkKind> {
        self.lock()
            .links
            .iter()
            .find(|(_, l)| l.addr == addr)
            .map(|(k, _)| *k)
    }

    /// A pair of streams joined by `kind`.
    fn wire_pair(&self, kind: LinkKind, generation: u64) -> (SimWire, SimWire) {
        let (a, b) = tokio::io::duplex(WIRE_BUFFER);
        let wire = |io| SimWire {
            io,
            net: self.clone(),
            kind,
            generation,
        };
        (wire(a), wire(b))
    }
}

/// One end of a stream over a simulated link.
///
/// With the link dead everything waits forever, without waking anyone:
/// this is how an unplugged cable looks from the software.
#[derive(Debug)]
pub struct SimWire {
    io: DuplexStream,
    net: SimNet,
    kind: LinkKind,
    generation: u64,
}

impl SimWire {
    fn dead(&self) -> bool {
        !self.net.alive(self.kind, self.generation)
    }
}

impl AsyncRead for SimWire {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.dead() {
            return Poll::Pending;
        }
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl AsyncWrite for SimWire {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.dead() {
            return Poll::Pending;
        }
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.dead() {
            return Poll::Pending;
        }
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct NetError(String);

/// What the link manager hands over for the active link: where to ask for steps.
#[derive(Clone)]
pub struct SimHandle {
    pub kind: LinkKind,
    client: Arc<PeerClient>,
}

pub struct SimSession {
    ping: Framed<SimWire>,
    nonce: u64,
    handle: SimHandle,
}

/// Opens connections with the agent on the other side over the simulated network.
pub struct SimProber {
    net: SimNet,
    far: Arc<Agent>,
}

impl SimProber {
    pub fn new(net: SimNet, far: Arc<Agent>) -> Self {
        SimProber { net, far }
    }
}

impl Prober for SimProber {
    type Session = SimSession;
    type Handle = SimHandle;
    type Error = NetError;

    async fn connect(&self, addr: SocketAddr) -> Result<SimSession, NetError> {
        let kind = self
            .net
            .kind_of(addr)
            .ok_or_else(|| NetError(format!("{addr} is not a simulated link")))?;
        let Some(generation) = self.net.reachable(kind) else {
            // Without a cable or with the other machine off nobody answers: the manager cuts it off.
            return std::future::pending().await;
        };
        // One stream for the manager's pings and another for step requests, like
        // QUIC's two control streams.
        let (ping, ping_far) = self.net.wire_pair(kind, generation);
        let (steps, steps_far) = self.net.wire_pair(kind, generation);
        for far in [ping_far, steps_far] {
            let agent = self.far.clone();
            tokio::spawn(async move {
                if let Err(e) = peer::serve(Framed::new(far), agent).await {
                    tracing::debug!("simulated stream ended: {e}");
                }
            });
        }
        Ok(SimSession {
            ping: Framed::new(ping),
            nonce: 0,
            handle: SimHandle {
                kind,
                client: Arc::new(PeerClient::spawn(Framed::new(steps))),
            },
        })
    }

    async fn ping(&self, s: &mut SimSession) -> Result<(), NetError> {
        s.nonce = s.nonce.wrapping_add(1);
        let nonce = s.nonce;
        let err = |e: tandemd::ipc::IpcError| NetError(e.to_string());
        s.ping
            .send(&ControlMsg::Ping { nonce })
            .await
            .map_err(err)?;
        loop {
            match s.ping.recv::<ControlMsg>().await.map_err(err)? {
                Some(ControlMsg::Pong { nonce: n }) if n == nonce => return Ok(()),
                Some(_) => {}
                None => return Err(NetError("the other side closed".into())),
            }
        }
    }

    fn handle(s: &SimSession) -> SimHandle {
        s.handle.clone()
    }
}

/// The other agent as seen by the orchestrator: each request goes over the link active at
/// that moment, the one the link manager chose, and fails with `PEER_LOST` if that
/// link drops before the answer.
pub struct ManagedPeer {
    links: LinkManager<SimHandle>,
}

impl ManagedPeer {
    pub fn new(links: LinkManager<SimHandle>) -> Self {
        ManagedPeer { links }
    }

    pub fn active_link(&self) -> Option<LinkKind> {
        self.links.active().map(|a| a.kind)
    }

    pub fn manager(&self) -> &LinkManager<SimHandle> {
        &self.links
    }
}

impl PeerLink for ManagedPeer {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
        let active = self.links.active();
        let links = self.links.watch();
        Box::pin(async move {
            match active {
                Some(a) => peer::over_link(links, a.kind, a.handle.client.execute(step)).await,
                None => Err(PEER_NOT_CONNECTED.to_owned()),
            }
        })
    }

    fn publish_state(&self, state: &State) {
        if let Some(a) = self.links.active() {
            a.handle.client.publish_state(state);
        }
    }

    fn send(&self, msg: ControlMsg) {
        if let Some(a) = self.links.active() {
            a.handle.client.send(msg);
        }
    }

    fn send_lossy(&self, msg: ControlMsg) {
        if let Some(a) = self.links.active() {
            a.handle.client.send_lossy(msg);
        }
    }

    fn hold_link(&self) -> Option<LinkHold> {
        Some(Box::new(self.links.hold()))
    }

    fn rtt(&self) -> Option<(tandem_proto::Link, std::time::Duration)> {
        let active = self.links.active()?;
        let rtt = self
            .links
            .links()
            .into_iter()
            .find(|l| l.kind == active.kind)
            .and_then(|l| l.rtt)?;
        Some((tandemd::network::link_of(active.kind), rtt))
    }
}
