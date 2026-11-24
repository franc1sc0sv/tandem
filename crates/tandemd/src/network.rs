//! The agent's network (DESIGN §6 and §9): QUIC endpoint with pinning, peer
//! discovery, link manager and the orchestrator's [`PeerLink`] over the active link.
//!
//! It is the same thing `tandem-sim` builds with its in-memory network, over the real transport:
//!
//! - The endpoint serves each of the peer's connections with [`peer::accept_streams`]: the pings
//!   of its link manager and its step requests.
//! - With a paired peer, the link manager tries the config's fixed links
//!   and the address the peer announces on the LAN over mDNS ([`NetworkConfig::link_candidates`]).
//!   For each link it opens a connection with a ping stream and a request stream
//!   ([`AgentProber`]), or adopts the one the other agent opened over that path: that is the
//!   only one when the other side's firewall drops what this side sends (T-237). Both sides
//!   serve every connection, whoever opened it.
//! - While any link sees the other agent, the orchestrator has a [`ManagedPeer`]
//!   that sends each request over the active link; if none sees it, it goes back to having no
//!   peer, and powering on machine B waits for it to appear. The same feeds
//!   [`peer_watch`](crate::peer_watch) on both machines and what the UI sees as [`PeerStatus`].
//! - Without a paired peer there is no manager or discovery: the agent works alone and the
//!   UI receives [`PeerStatus::Unpaired`].
//! - The UI pairs and unpairs live ([`PairingControl`]): when pairing, the new
//!   peer starts to be followed; when unpairing, it is removed from trust, it is
//!   no longer followed and its connections are cut.
//!
//! [`NetworkConfig::link_candidates`]: crate::NetworkConfig::link_candidates

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, Weak};
use std::time::Duration;

use tandem_core::{Machine, Os, State, Step};
use tandem_link::discovery::MdnsDiscovery;
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_link::iface::{
    AssignNotices, Interfaces, ScopedDiscovery, Shadowed, SystemAssigner, SystemInterfaces,
    WiredChange, WiredResolver, follow_wired, kind_for_addr, relabel_with_shadowed, wired_changes,
};
use tandem_link::manager::{
    HealthConfig, LinkCandidate, LinkKind, LinkManager, LinkStatus, ManagerError, Prober,
    QuicProber, QuicSession,
};
use tandem_link::metrics;
use tandem_link::qos::LinkQos;
use tandem_link::transport::{Connection, Endpoint, TransportError};
use tandem_proto::{ControlMsg, Hello, Link, NetworkNotice, PairingStatus, Peer, PeerStatus};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::agent::{Agent, NO_NETWORK, PairingControl};
use crate::config::FALLBACK_PORTS;
use crate::orchestrator::{LinkHold, PEER_NOT_CONNECTED, PeerLink, StepFuture};
use crate::pairing::{self, PairingTimeouts};
use crate::peer::{self, PeerClient, PeerError};

/// How long to wait for the peer to learn of the connections closing on shutdown.
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// How to start the network. The normal one is [`NetOptions::default`]; tests choose the
/// address, turn off mDNS and speed up the health check.
#[derive(Debug, Clone)]
pub struct NetOptions {
    /// UDP address the endpoint listens on. By default, all interfaces on the
    /// config's port ([`default_bind`]).
    pub bind: Option<SocketAddr>,
    /// The ports tried, on the same IP, if the one in `bind` (or the config's) cannot
    /// be opened. The real one is announced over mDNS.
    pub fallback_ports: Vec<u16>,
    /// Announce and look for the peer on the LAN over mDNS.
    pub mdns: bool,
    pub health: HealthConfig,
    pub pairing: PairingTimeouts,
    /// Whether this agent dials the peer. Tests turn it off to play a peer whose firewall
    /// silently drops inbound UDP (T-237): the dial then hangs until the connect timeout.
    pub dial: bool,
    /// How long the peer must be silent before another machine's pairing offer is reported
    /// (T-345, [`crate::offer`]).
    pub stale_after: Duration,
    /// Tests feed here the pairing announcements the LAN would deliver (CI has no
    /// multicast); without it they come from mDNS if `mdns` is on.
    pub offers: Option<watch::Receiver<Vec<SocketAddr>>>,
}

impl Default for NetOptions {
    fn default() -> Self {
        NetOptions {
            bind: None,
            fallback_ports: FALLBACK_PORTS.to_vec(),
            mdns: true,
            health: HealthConfig::default(),
            pairing: PairingTimeouts::default(),
            dial: true,
            stale_after: crate::offer::DEFAULT_STALE_AFTER,
            offers: None,
        }
    }
}

/// Transport errors are large: they are boxed so as not to enlarge every `Result`.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("could not open the QUIC endpoint at {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: Box<TransportError>,
    },
    #[error(
        "could not open any UDP port for the agent at {ip} (tried: {}): {last}. \
         Another program is using them or the system reserves them; on Windows with WSL or \
         Hyper-V, `netsh interface ipv4 show excludedportrange protocol=udp` shows the \
         reserved ranges. Pick a free port in `[network] port` of tandem.toml and \
         restart the agent",
        list(tried)
    )]
    NoPort {
        ip: IpAddr,
        tried: Vec<u16>,
        /// The error of the last attempt.
        last: String,
    },
}

fn list(ports: &[u16]) -> String {
    ports
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// `first` and then `fallbacks`, without repeating: the ports to try.
pub fn port_candidates(first: u16, fallbacks: &[u16]) -> Vec<u16> {
    let mut out = vec![first];
    for p in fallbacks {
        if !out.contains(p) {
            out.push(*p);
        }
    }
    out
}

/// Opens with `bind` the first of `ports` that can be, on `ip`. It only keeps trying if
/// the socket could not be opened (busy, reserved or without permission); any other error
/// (identity, TLS) does not depend on the port and is returned right away.
pub fn bind_first<T>(
    ip: IpAddr,
    ports: &[u16],
    mut bind: impl FnMut(SocketAddr) -> Result<T, TransportError>,
) -> Result<(T, SocketAddr), NetError> {
    let mut last = String::from("there were no ports to try");
    for (i, port) in ports.iter().enumerate() {
        let addr = SocketAddr::new(ip, *port);
        match bind(addr) {
            Ok(t) => {
                if i > 0 {
                    tracing::warn!(
                        "port {} could not be opened ({last}): the agent uses {port}",
                        ports[0]
                    );
                }
                return Ok((t, addr));
            }
            Err(TransportError::Bind(e)) => {
                tracing::debug!(%addr, "could not open: {e}");
                last = e.to_string();
            }
            Err(source) => {
                return Err(NetError::Bind {
                    addr,
                    source: Box::new(source),
                });
            }
        }
    }
    Err(NetError::NoPort {
        ip,
        tried: ports.to_vec(),
        last,
    })
}

/// Every interface on `port`, dual-stack (`[::]`): the configured cable peer is usually an
/// IPv4 link-local while the one learned by mDNS is an IPv6 link-local, and an IPv4-only
/// endpoint refuses to dial the latter (T-196). The endpoint makes the socket accept IPv4
/// too, and falls back to `0.0.0.0` on a host without IPv6.
pub fn default_bind(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port)
}

/// The manager's link, as the UI sees it.
pub fn link_of(kind: LinkKind) -> Link {
    match kind {
        LinkKind::Usb4 => Link::Usb4,
        LinkKind::Ethernet => Link::Ethernet,
        LinkKind::Wifi => Link::Wifi,
    }
}

/// The running network. Dropping it closes the connections and stops the link manager,
/// the discovery and the pairing in progress.
pub struct Network {
    shared: Arc<Shared>,
}

/// What the network, the pairing and whoever serves its requests share.
struct Shared {
    agent: Arc<Agent>,
    identity: Identity,
    endpoint: Arc<Endpoint>,
    local_addr: SocketAddr,
    /// With a lock: pairing from the UI adds and removes peers live, and
    /// the endpoint's pinning sees it on new connections.
    trust: Arc<RwLock<TrustStore>>,
    opts: NetOptions,
    conns: Arc<Conns>,
    accept: JoinHandle<()>,
    /// The system's interfaces: the wired links and the type of each
    /// connection come from them (T-094).
    interfaces: Arc<SystemInterfaces>,
    /// The peer being followed (link manager, mDNS and `peer_watch`), if any.
    peer: Mutex<Option<Following>>,
    pairing: Mutex<Option<PairingRun>>,
    /// The active link with the followed peer, for the input engine. It lives here and
    /// not in each `supervise`: the peer can be paired or unpaired live, and
    /// whoever listens from startup has to see every new link.
    active: LinkFeed,
    /// Carries the desk's `[data]` to each path's `Qos`.
    desk_follow: JoinHandle<()>,
}

/// The active link and who can write it: only the `supervise` of the peer that is
/// followed now. One that was already aborted may be midway through a poll; without the
/// generation it could restore the link of a forgotten peer.
struct LinkFeed {
    tx: watch::Sender<Option<ActiveLink>>,
    generation: AtomicU64,
}

impl LinkFeed {
    fn new() -> Self {
        LinkFeed {
            tx: watch::channel(None).0,
            generation: AtomicU64::new(0),
        }
    }

    /// Invalidates the previous writer and leaves the link at `None`. Returns the
    /// new writer's generation.
    fn reset(&self) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.tx.send_replace(None);
        generation
    }

    /// Publishes `link` if `generation` is still the current one and something changed. The
    /// comparison goes inside the channel's lock: `reset` writes with that same
    /// lock, so whatever is published before is overwritten by the `None`.
    fn publish(&self, generation: u64, link: Option<ActiveLink>) {
        self.tx.send_if_modified(|old| {
            if self.generation.load(Ordering::SeqCst) != generation || same_link(old, &link) {
                return false;
            }
            *old = link;
            true
        });
    }
}

/// What `supervise` uses to publish the active link.
struct LinkWriter {
    shared: Weak<Shared>,
    generation: u64,
}

impl LinkWriter {
    fn publish(&self, link: Option<ActiveLink>) {
        if let Some(shared) = self.shared.upgrade() {
            shared.active.publish(self.generation, link);
        }
    }
}

struct Following {
    key: PublicKey,
    tasks: Vec<JoinHandle<()>>,
}

struct PairingRun {
    answers: mpsc::UnboundedSender<pairing::Answer>,
    task: JoinHandle<()>,
}

/// The connections with the other agent that this side serves: the ones the other agent
/// opened and the ones this side dialled. They are cut when unpairing (the pinning only looks at trust in the
/// handshake), and the ones the other opened are also a path this side can use when it
/// cannot dial the other (T-237).
struct Conns {
    next: AtomicU64,
    open: Mutex<BTreeMap<u64, Tracked>>,
    /// Bumped when the other agent opens a connection: `supervise` checks whether its path
    /// is one the link manager follows.
    arrived: watch::Sender<u64>,
}

struct Tracked {
    conn: Connection,
    kind: LinkKind,
    /// Opened by the other agent: this side may adopt it as its path.
    inbound: bool,
}

impl Default for Conns {
    fn default() -> Self {
        Conns {
            next: AtomicU64::new(0),
            open: Mutex::new(BTreeMap::new()),
            arrived: watch::channel(0).0,
        }
    }
}

impl Conns {
    fn add(&self, conn: Connection, kind: LinkKind, inbound: bool) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        lock(&self.open).insert(
            id,
            Tracked {
                conn,
                kind,
                inbound,
            },
        );
        if inbound {
            self.arrived.send_modify(|n| *n = n.wrapping_add(1));
        }
        id
    }

    fn remove(&self, id: u64) {
        lock(&self.open).remove(&id);
    }

    fn close_from(&self, key: &PublicKey) {
        self.close_from_except(key, usize::MAX);
    }

    /// Like [`Conns::close_from`], except the connection with quinn's id `keep`.
    fn close_from_except(&self, key: &PublicKey, keep: usize) {
        for t in lock(&self.open)
            .values()
            .filter(|t| t.conn.peer() == *key && t.conn.id() != keep)
        {
            t.conn.close("unpaired");
        }
    }

    /// A live connection `key` opened toward this side over the path of `addr`: the one from
    /// the same IP or, failing that, the one over the same kind of link. The newest wins.
    fn inbound(&self, key: &PublicKey, addr: SocketAddr, kind: LinkKind) -> Option<Connection> {
        let open = lock(&self.open);
        let live = || {
            open.values()
                .rev()
                .filter(|t| t.inbound && t.conn.peer() == *key && !t.conn.is_closed())
        };
        let ip = unmapped(addr.ip());
        live()
            .find(|t| unmapped(t.conn.remote_address().ip()) == ip)
            .or_else(|| live().find(|t| t.kind == kind))
            .map(|t| t.conn.clone())
    }

    /// The paths over which `key` has a live connection toward this side.
    fn inbound_paths(&self, key: &PublicKey) -> Vec<LinkCandidate> {
        lock(&self.open)
            .values()
            .filter(|t| t.inbound && t.conn.peer() == *key && !t.conn.is_closed())
            .map(|t| {
                let addr = t.conn.remote_address();
                LinkCandidate {
                    kind: t.kind,
                    // A dual-stack socket shows IPv4 peers mapped: dialled back as IPv4.
                    addr: match unmapped(addr.ip()) {
                        ip @ IpAddr::V4(_) => SocketAddr::new(ip, addr.port()),
                        IpAddr::V6(_) => addr,
                    },
                }
            })
            .collect()
    }
}

fn unmapped(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}

/// How long to wait for the "left the desk" notice to go out before closing the connection.
const GOODBYE_WAIT: Duration = Duration::from_millis(500);

/// If another thread panicked with the lock, what is inside is still usable.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the UI receives when asking to pair with an already paired peer.
pub fn already_paired(name: &str) -> String {
    format!("already paired with {name}: unpair it first")
}
pub const PAIRING_IN_PROGRESS: &str = "a pairing is already in progress";
pub const NOTHING_TO_CONFIRM: &str = "there is no code to confirm";
pub const NOT_PAIRED: &str = "there is no paired peer";

/// The link everything goes over at this moment: the input engine opens its
/// streams there and sends its notices.
#[derive(Clone)]
pub struct ActiveLink {
    pub kind: LinkKind,
    pub handle: PeerHandle,
}

impl ActiveLink {
    /// The same session: same link and same connection.
    pub fn same(&self, other: &ActiveLink) -> bool {
        self.kind == other.kind && Arc::ptr_eq(&self.handle.client, &other.handle.client)
    }
}

fn same_link(a: &Option<ActiveLink>, b: &Option<ActiveLink>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.same(b),
        (None, None) => true,
        _ => false,
    }
}

impl Network {
    /// Opens the endpoint and, if there is a paired peer in `trust`, starts looking for it.
    /// Serves the agent's pairing requests. It must be called inside
    /// a tokio runtime.
    pub fn start(
        agent: Arc<Agent>,
        identity: &Identity,
        trust: TrustStore,
        opts: NetOptions,
    ) -> Result<Self, NetError> {
        let net_cfg = &agent.config().network;
        let wanted = opts.bind.unwrap_or_else(|| default_bind(net_cfg.port));
        // The first peer in trust: for now an agent pairs with just one.
        let paired = trust.peers().next().cloned();
        let trust = Arc::new(RwLock::new(trust));
        let ports = port_candidates(wanted.port(), &opts.fallback_ports);
        let (endpoint, addr) = bind_first(wanted.ip(), &ports, |addr| {
            Endpoint::bind(addr, identity, trust.clone())
        })?;
        let endpoint = Arc::new(endpoint);
        let local_addr = endpoint.local_addr().map_err(|source| NetError::Bind {
            addr,
            source: Box::new(source),
        })?;
        tracing::info!(addr = %local_addr, "QUIC endpoint listening");
        agent.set_port(local_addr.port());

        let conns = Arc::new(Conns::default());
        let interfaces = Arc::new(SystemInterfaces::spawn());
        let accept = tokio::spawn(accept_loop(
            endpoint.clone(),
            agent.clone(),
            conns.clone(),
            interfaces.clone(),
        ));
        let shared = Arc::new(Shared {
            agent: agent.clone(),
            identity: identity.clone(),
            endpoint,
            local_addr,
            trust,
            opts,
            conns,
            accept,
            interfaces,
            peer: Mutex::new(None),
            pairing: Mutex::new(None),
            active: LinkFeed::new(),
            desk_follow: follow_desk(&agent),
        });
        agent.set_pairing(Arc::new(Pairer(Arc::downgrade(&shared))));
        match paired {
            Some(peer) => shared.follow(peer, Settled::from_disk()),
            None => {
                tracing::info!("no paired peer: the agent works alone");
                agent.set_peer_status(PeerStatus::Unpaired);
            }
        }
        Ok(Network { shared })
    }

    /// The active link with the other agent; `None` if none sees it (or there is no peer).
    pub fn active_link(&self) -> watch::Receiver<Option<ActiveLink>> {
        self.shared.active.tx.subscribe()
    }

    /// The endpoint's real address (with the chosen port if 0 was requested).
    pub fn local_addr(&self) -> SocketAddr {
        self.shared.local_addr
    }

    /// Closes the connections and waits a bit for the peer to learn of it, so it does not
    /// have to wait for its pings to time out.
    pub async fn shutdown(self) {
        self.shared.stop();
        if tokio::time::timeout(CLOSE_TIMEOUT, self.shared.endpoint.wait_idle())
            .await
            .is_err()
        {
            tracing::debug!("the peer did not confirm the connections closing in time");
        }
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        self.shared.stop();
    }
}

impl Shared {
    fn stop(&self) {
        self.accept.abort();
        self.desk_follow.abort();
        if let Some(f) = lock(&self.peer).as_ref() {
            for t in &f.tasks {
                t.abort();
            }
        }
        if let Some(p) = lock(&self.pairing).as_ref() {
            p.task.abort();
        }
        self.endpoint.close();
    }

    /// Starts following `peer`: mDNS, link manager and the peer watch (`peer_watch`).
    /// First, this machine's side is set to what the two keys say (DECISIONS A1).
    /// `settled` says when a rejection from the other agent means it forgot this one.
    fn follow(self: &Arc<Self>, peer: TrustedPeer, settled: Settled) {
        let TrustedPeer { public_key, name } = peer;
        tracing::info!(peer = %name, key = %public_key.to_hex(), "peer paired");
        let agent = &self.agent;
        if let Some(side) =
            Machine::of_keys(self.identity.public_key().as_bytes(), public_key.as_bytes())
        {
            agent.adopt_side(side);
        }
        agent.set_peer_status(PeerStatus::Paired {
            name: name.clone(),
            link: None,
        });
        let discovery = if self.opts.mdns {
            match MdnsDiscovery::start(
                &self.identity.public_key(),
                self.local_addr.port(),
                [public_key],
            ) {
                Ok(d) => Some(d),
                // Without multicast the fixed links remain: not a reason not to start.
                Err(e) => {
                    tracing::warn!("mDNS unavailable, only the fixed links are used: {e}");
                    None
                }
            }
        } else {
            None
        };
        let (reachable_tx, reachable) = watch::channel(false);
        let offers = self.offer_feed(public_key);
        let mut following = lock(&self.peer);
        // First the previous one is silenced: its link is not the new peer's.
        if let Some(p) = following.take() {
            for t in p.tasks {
                t.abort();
            }
        }
        let link = LinkWriter {
            shared: Arc::downgrade(self),
            generation: self.active.reset(),
        };
        let tasks: Vec<JoinHandle<()>> = vec![
            tokio::spawn(supervise(
                agent.clone(),
                self.endpoint.clone(),
                PeerSide {
                    conns: self.conns.clone(),
                    key: public_key,
                    dial: self.opts.dial,
                    settled: Arc::new(settled),
                },
                name,
                discovery,
                self.opts.health,
                reachable_tx,
                link,
                self.interfaces.clone(),
            )),
            // Both watch the other: either can be left alone (DESIGN §3.7).
            tokio::spawn(crate::peer_watch::watch(agent.clone(), reachable)),
        ]
        .into_iter()
        .chain(offers.map(|(feed, keep)| {
            let (agent, after) = (agent.clone(), self.opts.stale_after);
            tokio::spawn(async move {
                // The announcements stop being watched when the peer is forgotten.
                let _keep = keep;
                crate::offer::watch(agent, feed, after).await;
            })
        }))
        .collect();
        *following = Some(Following {
            key: public_key,
            tasks,
        });
    }

    /// Where the pairing announcements of machines other than `peer` come from (T-345), with
    /// what has to stay alive for them to keep arriving.
    #[allow(clippy::type_complexity)]
    fn offer_feed(
        &self,
        peer: PublicKey,
    ) -> Option<(watch::Receiver<Vec<SocketAddr>>, Option<MdnsDiscovery>)> {
        if let Some(feed) = self.opts.offers.clone() {
            return Some((feed, None));
        }
        if !self.opts.mdns {
            return None;
        }
        match MdnsDiscovery::pairing_offers(&self.identity.public_key(), [peer]) {
            Ok(d) => Some((d.peer_addrs(), Some(d))),
            Err(e) => {
                tracing::warn!("no mDNS: a pairing offer from another machine cannot be seen: {e}");
                None
            }
        }
    }

    /// Forgets the peer: removes it from trust, stops looking for it and cuts its
    /// connections. If trust cannot be saved, nothing changes.
    fn forget(&self, notify: bool) -> Result<(), String> {
        let mut following = lock(&self.peer);
        let Some(key) = following.as_ref().map(|f| f.key) else {
            return Err(NOT_PAIRED.into());
        };
        {
            let mut trust = self.trust.write().unwrap_or_else(|e| e.into_inner());
            trust
                .remove_and_save(&key)
                .map_err(|e| format!("could not save trust: {e}"))?;
        }
        // With a link, the other machine is told so it also forgets the key
        // (T-152); the connection is closed once the notice has gone out.
        let goodbye = notify
            .then(|| self.active.tx.borrow().clone())
            .flatten()
            .map(|a| a.handle);
        if let Some(f) = following.take() {
            for t in f.tasks {
                t.abort();
            }
        }
        // Without a peer there is no link: the input engine closes its session.
        self.active.reset();
        drop(following);
        // `supervise` does not get to release the peer when aborted.
        self.agent.orchestrator().clear_peer();
        match goodbye {
            Some(PeerHandle { conn, client }) => {
                client.send(ControlMsg::Unpaired);
                let conns = self.conns.clone();
                tokio::spawn(async move {
                    let _ = tokio::time::timeout(GOODBYE_WAIT, client.flush()).await;
                    // Closing drops what is still unsent: the notice's connection is left for
                    // the other side to close once it reads it, with a cap.
                    conns.close_from_except(&key, conn.id());
                    tokio::time::sleep(GOODBYE_WAIT).await;
                    conns.close_from(&key);
                });
            }
            None => self.conns.close_from(&key),
        }
        tracing::info!(key = %key.to_hex(), "unpaired");
        self.agent.set_peer_status(PeerStatus::Unpaired);
        self.agent.set_pairing_status(PairingStatus::Idle);
        Ok(())
    }

    fn paired_name(&self) -> Option<String> {
        let key = lock(&self.peer).as_ref().map(|f| f.key)?;
        let trust = self.trust.read().unwrap_or_else(|e| e.into_inner());
        Some(
            trust
                .verify(&key)
                .map(|p| p.name.clone())
                .unwrap_or_default(),
        )
    }

    fn pairing_running(&self) -> bool {
        lock(&self.pairing)
            .as_ref()
            .is_some_and(|p| !p.task.is_finished())
    }

    fn start_pairing(self: &Arc<Self>) -> Result<(), String> {
        if let Some(name) = self.paired_name() {
            return Err(already_paired(&name));
        }
        let mut slot = lock(&self.pairing);
        if slot.as_ref().is_some_and(|p| !p.task.is_finished()) {
            return Err(PAIRING_IN_PROGRESS.into());
        }
        let (answers, mut rx) = mpsc::unbounded_channel();
        let ctx = pairing::Ctx {
            agent: self.agent.clone(),
            identity: self.identity.clone(),
            trust: self.trust.clone(),
            bind_ip: self.local_addr.ip(),
            port: self.local_addr.port(),
            fallback_ports: self.opts.fallback_ports.clone(),
            mdns: self.opts.mdns,
            timeouts: self.opts.pairing,
        };
        // Before returning: the UI that asked to start already sees that it is searching.
        self.agent.set_pairing_status(PairingStatus::Searching);
        let shared = self.clone();
        let task = tokio::spawn(async move {
            let outcome = pairing::run(&ctx, &mut rx).await;
            let status = match outcome {
                pairing::Outcome::Paired(peer) => {
                    let name = peer.name.clone();
                    shared.follow(peer, Settled::just_paired());
                    PairingStatus::Done { peer: name }
                }
                pairing::Outcome::Cancelled => PairingStatus::Idle,
                pairing::Outcome::Failed(reason) => {
                    tracing::warn!("not paired: {reason}");
                    PairingStatus::Failed { reason }
                }
            };
            shared.agent.set_pairing_status(status);
        });
        *slot = Some(PairingRun { answers, task });
        Ok(())
    }

    fn answer(&self, answer: pairing::Answer) -> bool {
        lock(&self.pairing)
            .as_ref()
            .filter(|p| !p.task.is_finished())
            .is_some_and(|p| p.answers.send(answer).is_ok())
    }
}

/// What the agent uses to pair. Weak: the network belongs to whoever started it, and without
/// it requests are rejected.
struct Pairer(Weak<Shared>);

impl Pairer {
    fn shared(&self) -> Result<Arc<Shared>, String> {
        self.0.upgrade().ok_or_else(|| NO_NETWORK.to_owned())
    }
}

impl PairingControl for Pairer {
    fn start(&self) -> Result<(), String> {
        self.shared()?.start_pairing()
    }

    fn confirm(&self) -> Result<(), String> {
        let shared = self.shared()?;
        let waiting = matches!(
            shared.agent.pairing_status(),
            PairingStatus::Comparing {
                confirmed: false,
                ..
            }
        );
        if waiting && shared.answer(pairing::Answer::Confirm) {
            Ok(())
        } else {
            Err(NOTHING_TO_CONFIRM.into())
        }
    }

    fn cancel(&self) -> Result<(), String> {
        let shared = self.shared()?;
        if !shared.answer(pairing::Answer::Cancel) && !shared.pairing_running() {
            // Nothing in progress: the UI closes an already shown result.
            shared.agent.set_pairing_status(PairingStatus::Idle);
        }
        Ok(())
    }

    fn unpair(&self) -> Result<(), String> {
        self.shared()?.forget(true)
    }

    fn peer_left(&self) -> Result<(), String> {
        self.shared()?.forget(false)
    }
}

/// Serves each incoming connection. Pinning already rejected whoever is not in trust.
async fn accept_loop(
    endpoint: Arc<Endpoint>,
    agent: Arc<Agent>,
    conns: Arc<Conns>,
    interfaces: Arc<SystemInterfaces>,
) {
    while let Some(conn) = endpoint.accept().await {
        match conn {
            Ok(conn) => {
                tracing::debug!(from = %conn.remote_address(), "connection from the other agent");
                // What this side sends over it (replies, the other's clipboard) shares
                // the cap and pause of its path with the connection this agent
                // opens (T-101).
                let statics = agent.config().network.link_candidates(&[]);
                // The type comes from the interface it arrived on, not from the configuration.
                let remote = conn.remote_address();
                let kind = kind_for_addr(&interfaces.list(), remote)
                    .unwrap_or_else(|| kind_of_remote(&statics, remote));
                let conn = conn.with_qos(agent.link_qos().get(kind));
                let (agent, conns) = (agent.clone(), conns.clone());
                let id = conns.add(conn.clone(), kind, true);
                tokio::spawn(async move {
                    peer::accept_streams(conn, agent).await;
                    conns.remove(id);
                });
            }
            Err(e) => tracing::debug!("incoming connection rejected: {e}"),
        }
    }
}

/// The desk's `[data]`, every time it changes (from the UI, the other machine or by hand), in
/// the agent's [`LinkQos`]: it reaches all connections, opened and accepted (T-101).
fn follow_desk(agent: &Agent) -> JoinHandle<()> {
    let qos = agent.link_qos().clone();
    let mut desk = agent.subscribe_desk();
    tokio::spawn(async move {
        loop {
            let data = desk.borrow_and_update().data;
            qos.set_prefs(data);
            if desk.changed().await.is_err() {
                return;
            }
        }
    })
}

/// Which path a connection opened by the other agent came over: the fixed-address one
/// with the same IP; if there is none, the LAN (Wi-Fi), which is the one discovered over mDNS. When in
/// doubt, the Wi-Fi cap: a capped cable is only slower, an uncapped Wi-Fi
/// adds latency to the input.
fn kind_of_remote(statics: &[LinkCandidate], remote: SocketAddr) -> LinkKind {
    statics
        .iter()
        .find(|c| unmapped(c.addr.ip()) == unmapped(remote.ip()))
        .map_or(LinkKind::Wifi, |c| c.kind)
}

/// Builds the link manager with the candidates of each moment and follows its active
/// link. Once it runs, what is discovered on the LAN only hands it new links or new
/// addresses ([`links_to_add`]); it is never rebuilt for a discovery change: that cut every
/// healthy connection and the input and clipboard sessions over them, and mDNS on the PC
/// flickers every few seconds (T-237, 2026-11-24). It is only built again when there was
/// none (no links yet).
#[allow(clippy::too_many_arguments)]
async fn supervise(
    agent: Arc<Agent>,
    endpoint: Arc<Endpoint>,
    peer: PeerSide,
    name: String,
    discovery: Option<MdnsDiscovery>,
    health: HealthConfig,
    reachable: watch::Sender<bool>,
    active_link: LinkWriter,
    interfaces: Arc<SystemInterfaces>,
) {
    let mut discovered = discovery.as_ref().map(MdnsDiscovery::peer_addrs);
    let raw = |d: &Option<watch::Receiver<Vec<SocketAddr>>>| {
        d.as_ref().map(|d| d.borrow().clone()).unwrap_or_default()
    };
    // The LAN (Wi-Fi) does not include the link-local ones with an interface: they belong to the cables and the
    // `WiredResolver` resolves them.
    let lan = |d: &Option<watch::Receiver<Vec<SocketAddr>>>| {
        let mut all = raw(d);
        all.retain(|a| !matches!(a, SocketAddr::V6(v6) if v6.scope_id() != 0));
        all
    };
    let mut candidates = agent.config().network.link_candidates(&lan(&discovered));
    // The other agent for each cable: its link-local comes from mDNS, tied to the interface's `scope_id`.
    // Without discovery, an empty channel.
    let (_quiet, silent) = watch::channel(Vec::new());
    let scoped = Arc::new(ScopedDiscovery::new(discovered.clone().unwrap_or(silent)));
    let metrics = agent.net_metrics().clone();
    let notices = AssignNotices::with_sink(move |list| {
        metrics.set_notices(
            list.into_iter()
                .map(|n| NetworkNotice {
                    interface: n.interface,
                    message: n.message,
                    command: n.command,
                })
                .collect(),
        );
    });
    let new_resolver = || {
        Arc::new(WiredResolver {
            side: agent.machine(),
            os: local_os(),
            port: agent.config().network.port,
            discovery: scoped.clone(),
            assigner: Arc::new(SystemAssigner),
            notices: notices.clone(),
        })
    };
    // Paths the other agent reached this side over that no candidate covered (T-237).
    let mut extra: Vec<LinkCandidate> = Vec::new();
    let mut arrived = peer.conns.arrived.subscribe();
    let mut scoped_seen = scoped_snapshot(&raw(&discovered));
    loop {
        // The cables already plugged in enter with the manager; the ones that appear or
        // disconnect later are followed by `follow_wired`. What is configured by hand rules.
        let resolver = new_resolver();
        let plan = plan_links(&candidates, &extra, &interfaces.list(), &resolver);
        // Two candidates on one interface get the same kind: one is kept, never refused (T-223).
        for s in &plan.shadowed {
            tracing::warn!(
                kind = %s.kind,
                kept = %s.kept,
                dropped = %s.dropped,
                "two peers reach the same link: only the first one is used"
            );
        }
        let (spawned, fixed_kinds, shadowed) = (plan.links, plan.fixed_kinds, plan.shadowed);
        let resolve = move |i: &tandem_link::iface::NetInterface| {
            if fixed_kinds.contains(&i.kind.link_kind()) {
                return None;
            }
            resolver.resolve(i)
        };
        let managed = match spawn_manager(
            &agent,
            &endpoint,
            &peer,
            &spawned,
            health,
            &interfaces,
            &active_link,
        ) {
            Ok(m) => Ok(m),
            Err(ManagerError::NoCandidates) => {
                tracing::info!("no links with the peer: waiting to discover it on the LAN");
                Err(ManagerError::NoCandidates)
            }
            Err(e) => {
                tracing::error!("could not start the link manager: {e}");
                Err(e)
            }
        };
        agent
            .net_metrics()
            .set_link_notices(metrics::link_notices(&shadowed, managed.as_ref().err()));
        let managed = managed.ok();
        let mut links = managed.as_ref().map(|m| m.links.watch());
        let _follow = managed
            .as_ref()
            .map(|m| AbortOnDrop(follow_wired(&*interfaces, m.links.control(), resolve)));
        let mut installed = false;
        // An inbound connection that arrived before this manager existed is checked right away.
        arrived.mark_changed();
        loop {
            let current = links.as_ref().and_then(|l| {
                l.borrow().active.as_ref().map(|a| ActiveLink {
                    kind: a.kind,
                    handle: a.handle.clone(),
                })
            });
            let active = current.as_ref().map(|a| a.kind);
            active_link.publish(current);
            // Each healthy path, to measure its bandwidth even if it is not the active one (T-102).
            agent.set_path_conns(
                links
                    .as_ref()
                    .map(|l| {
                        l.borrow()
                            .handles
                            .iter()
                            .map(|(k, h)| (*k, h.conn.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
            );
            match (&managed, active.is_some(), installed) {
                (Some(m), true, false) => {
                    agent.set_peer(m.clone());
                    installed = true;
                }
                (_, false, true) => {
                    agent.orchestrator().clear_peer();
                    installed = false;
                }
                _ => {}
            }
            let up = active.is_some();
            reachable.send_if_modified(|r| {
                let changed = *r != up;
                *r = up;
                changed
            });
            agent.set_peer_status(PeerStatus::Paired {
                name: name.clone(),
                link: active.map(link_of),
            });
            tokio::select! {
                changed = changed(&mut links) => {
                    if changed.is_err() {
                        // The manager does not finish on its own; if it did, there is nothing left to follow.
                        links = None;
                    }
                }
                changed = arrived.changed() => {
                    if changed.is_err() {
                        continue;
                    }
                    // A path the other agent reached this side over and the manager does not
                    // follow: it enters, so its connection is adopted (T-237).
                    let followed: Vec<LinkKind> = managed
                        .as_ref()
                        .map(|m| m.links.links().iter().map(|l| l.kind).collect())
                        .unwrap_or_default();
                    let mut rebuild = false;
                    let inbound = peer.conns.inbound_paths(&peer.key);
                    // It dialled this side with its pinning: it trusts this machine.
                    if !inbound.is_empty() {
                        peer.settled.accepted();
                    }
                    for c in inbound {
                        if followed.contains(&c.kind) || extra.iter().any(|e| e.kind == c.kind) {
                            continue;
                        }
                        tracing::info!(link = %c.kind, addr = %c.addr, "the other agent reached this side over a new path");
                        extra.push(c);
                        match &managed {
                            Some(m) => {
                                let _ = m.links.control().add(c);
                            }
                            None => rebuild = true,
                        }
                    }
                    if rebuild {
                        break;
                    }
                }
                changed = changed(&mut discovered) => {
                    if changed.is_err() {
                        discovered = None;
                        continue;
                    }
                    let now = lan(&discovered);
                    let next = agent.config().network.link_candidates(&now);
                    // A cable that answers over mDNS changes who can be spoken to
                    // over it: it is rebuilt so that `resolve` sees it.
                    let seen = scoped_snapshot(&raw(&discovered));
                    if next != candidates || seen != scoped_seen {
                        scoped_seen = seen;
                        candidates = next;
                        let want =
                            plan_links(&candidates, &extra, &interfaces.list(), &new_resolver())
                                .links;
                        let Some(m) = &managed else {
                            tracing::info!(next = ?want, "the links with the peer changed");
                            break;
                        };
                        // A running manager is never rebuilt for this: it would cut every
                        // healthy connection and the sessions over them. It is handed what it
                        // does not follow yet and keeps a healthy link's connection.
                        let add = links_to_add(&want, &m.links.links());
                        if add.is_empty() {
                            tracing::debug!(?want, "discovery changed, the links did not");
                            continue;
                        }
                        tracing::info!(?add, "new links with the peer");
                        let control = m.links.control();
                        for c in add {
                            let _ = control.add(c);
                        }
                    }
                }
            }
        }
        if installed {
            agent.orchestrator().clear_peer();
        }
        active_link.publish(None);
        agent.set_path_conns(BTreeMap::new());
    }
}

/// What the manager should follow, from [`plan_links`].
struct LinkPlan {
    links: Vec<LinkCandidate>,
    /// The kinds that are fixed, which `follow_wired` leaves alone.
    fixed_kinds: Vec<LinkKind>,
    /// Candidates left out because another one reaches the same link (T-223).
    shadowed: Vec<Shadowed>,
}

/// The links the manager should follow: the configured and discovered ones relabelled by
/// their interface, the cables `resolver` finds, and the paths the other agent reached this
/// side over that none of those covers (T-237).
fn plan_links(
    candidates: &[LinkCandidate],
    extra: &[LinkCandidate],
    list: &[tandem_link::iface::NetInterface],
    resolver: &WiredResolver,
) -> LinkPlan {
    let (fixed, shadowed) = relabel_with_shadowed(candidates, list);
    let fixed_kinds: Vec<LinkKind> = fixed.iter().map(|c| c.kind).collect();
    let resolve = |i: &tandem_link::iface::NetInterface| {
        if fixed_kinds.contains(&i.kind.link_kind()) {
            return None;
        }
        resolver.resolve(i)
    };
    let mut links = fixed;
    let mut applied = BTreeMap::new();
    for change in wired_changes(&mut applied, list, &resolve) {
        if let WiredChange::Add(c) = change {
            links.push(c);
        }
    }
    for c in extra {
        if !links.iter().any(|s| s.kind == c.kind) {
            links.push(*c);
        }
    }
    LinkPlan {
        links,
        fixed_kinds,
        shadowed,
    }
}

/// What of `want` the manager does not follow yet: a new kind, or a kind on another address
/// (the manager keeps a healthy link's connection and dials that address once it drops).
/// Nothing is ever removed this way: a cable leaves when its interface does.
fn links_to_add(want: &[LinkCandidate], have: &[LinkStatus]) -> Vec<LinkCandidate> {
    want.iter()
        .filter(|c| !have.iter().any(|l| l.kind == c.kind && l.addr == c.addr))
        .copied()
        .collect()
}

/// The discovered link-locals (one per cable): when they change, resolve again.
fn scoped_snapshot(lan: &[SocketAddr]) -> Vec<SocketAddr> {
    let mut v: Vec<SocketAddr> = lan
        .iter()
        .copied()
        .filter(|a| matches!(a, SocketAddr::V6(v6) if v6.scope_id() != 0))
        .collect();
    v.sort();
    v
}

/// Waits for the next change; without a receiver, it never finishes.
async fn changed<T>(rx: &mut Option<watch::Receiver<T>>) -> Result<(), watch::error::RecvError> {
    match rx {
        Some(rx) => rx.changed().await,
        None => std::future::pending().await,
    }
}

fn spawn_manager(
    agent: &Arc<Agent>,
    endpoint: &Arc<Endpoint>,
    peer: &PeerSide,
    candidates: &[LinkCandidate],
    health: HealthConfig,
    interfaces: &Arc<SystemInterfaces>,
    link: &LinkWriter,
) -> Result<Arc<ManagedPeer>, ManagerError> {
    // If the other no longer trusts us, the key is also forgotten here: it ends up `Unpaired`.
    let shared = link.shared.clone();
    let rejected: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        if let Some(shared) = shared.upgrade() {
            tokio::spawn(async move {
                match shared.forget(false) {
                    Ok(()) => {
                        tracing::info!("the other machine no longer trusts this one: unpaired")
                    }
                    Err(e) => tracing::debug!("there was no peer to forget: {e}"),
                }
            });
        }
    });
    let prober = AgentProber::new(
        endpoint.clone(),
        agent.machine(),
        candidates,
        agent.link_qos().clone(),
    )
    .with_interfaces(interfaces.clone())
    .with_rejected(rejected)
    .with_peer(agent.clone(), peer.clone(), health.connect_timeout / 2);
    let (links, events) = LinkManager::spawn(candidates, prober, health)?;
    // The Network screen's metrics follow this manager while it lives (T-102).
    tokio::spawn(metrics::follow(
        agent.net_metrics().clone(),
        links.watch(),
        events,
        agent.link_qos().clone(),
        metrics::SAMPLE_EVERY,
    ));
    tracing::info!(?candidates, "link manager running");
    Ok(Arc::new(ManagedPeer { links }))
}

/// What the link manager hands over of the active link: the connection and the
/// request stream opened over it.
#[derive(Clone)]
pub struct PeerHandle {
    pub conn: Connection,
    pub client: Arc<PeerClient>,
}

pub struct AgentSession {
    quic: QuicSession,
    handle: PeerHandle,
}

/// The followed peer, as the link manager's prober needs it.
#[derive(Clone)]
struct PeerSide {
    conns: Arc<Conns>,
    key: PublicKey,
    /// See [`NetOptions::dial`].
    dial: bool,
    settled: Arc<Settled>,
}

/// How long, after pairing here, a rejection from the other agent is still taken as "it has not
/// saved the pairing yet" if it never accepted this side: past that, the other side most likely
/// failed to save it, and both are left unpaired instead of one waiting forever.
const PAIRING_SETTLE: Duration = Duration::from_secs(60);

/// Whether a rejection from the other agent means it forgot this machine (T-152) or only that
/// it has not saved the pairing yet (T-291). The two sides save trust on their own once both
/// codes are confirmed, and nothing orders one save before the other side's first dial: on a slow
/// machine the side that finished first dialled the other, was rejected and unpaired for good.
#[derive(Debug)]
struct Settled {
    /// The other agent took a connection with this side's key since it was followed.
    accepted: AtomicBool,
    /// When it was paired here; `None` if it came from trust on disk (it was settled long ago).
    paired_at: Option<Instant>,
}

impl Settled {
    fn from_disk() -> Self {
        Settled {
            accepted: AtomicBool::new(false),
            paired_at: None,
        }
    }

    fn just_paired() -> Self {
        Settled {
            accepted: AtomicBool::new(false),
            paired_at: Some(Instant::now()),
        }
    }

    fn accepted(&self) {
        self.accepted.store(true, Ordering::Relaxed);
    }

    /// A rejection now means the other agent no longer trusts this one.
    fn rejection_means_forgotten(&self, now: Instant) -> bool {
        self.accepted.load(Ordering::Relaxed)
            || self
                .paired_at
                .is_none_or(|t| now.saturating_duration_since(t) >= PAIRING_SETTLE)
    }
}

/// What lets the prober serve and adopt connections (T-237).
struct Adoption {
    agent: Arc<Agent>,
    peer: PeerSide,
    /// The most an inbound connection can take to answer before dialling instead.
    timeout: Duration,
}

/// [`QuicProber`] plus the request stream: each healthy link already has a way
/// to ask the other agent for steps. The other side serves the two streams with
/// [`peer::accept_streams`].
pub struct AgentProber {
    quic: QuicProber,
    me: Machine,
    /// Which path each address goes over, to give its connection the path's `Qos`.
    kinds: Vec<LinkCandidate>,
    qos: LinkQos,
    /// To classify the wired links that appear after building the prober.
    interfaces: Option<Arc<dyn Interfaces>>,
    /// Called when the other machine rejects our key for not being trusted.
    rejected: Option<Arc<dyn Fn() + Send + Sync>>,
    adoption: Option<Adoption>,
}

/// Aborts the task when dropped (a loose `JoinHandle` leaves it running).
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// This machine's system, to show the exact command of the backup IP.
fn local_os() -> Os {
    if cfg!(windows) {
        Os::Windows
    } else {
        Os::MacOs
    }
}

impl AgentProber {
    async fn open(&self, addr: SocketAddr) -> Result<AgentSession, PeerError> {
        let quic = self.quic.connect(addr).await?;
        let kind = self.kind_of(addr);
        let conn = QuicProber::handle(&quic).with_qos(self.qos.get(kind));
        // The other agent may use this connection as its own path (it cannot always dial this
        // side): what it opens over it is served here, as on the ones it opens (T-237).
        if let Some(a) = &self.adoption {
            let id = a.peer.conns.add(conn.clone(), kind, false);
            let (agent, conns, served) = (a.agent.clone(), a.peer.conns.clone(), conn.clone());
            tokio::spawn(async move {
                peer::accept_streams(served, agent).await;
                conns.remove(id);
            });
        }
        self.session(quic, conn, kind).await
    }

    /// The request stream over `conn`, whoever opened it.
    async fn session(
        &self,
        quic: QuicSession,
        conn: Connection,
        kind: LinkKind,
    ) -> Result<AgentSession, PeerError> {
        let client = Arc::new(PeerClient::open(&conn, self.me).await?);
        // The first thing the other side knows: how this side classifies the connection.
        client.send(ControlMsg::LinkClass { kind: kind.into() });
        Ok(AgentSession {
            quic,
            handle: PeerHandle { conn, client },
        })
    }

    /// A session over the connection the other agent opened over this path, if there is
    /// one. That is the path when this side cannot dial (its packets are dropped on the way
    /// or by the other's firewall), and when both can, using it means one connection
    /// instead of two. One that does not answer in time is closed, so it is not tried again.
    async fn adopt(&self, addr: SocketAddr, kind: LinkKind) -> Option<AgentSession> {
        let a = self.adoption.as_ref()?;
        let conn = a.peer.conns.inbound(&a.peer.key, addr, kind)?;
        let conn = conn.with_qos(self.qos.get(kind));
        let from = conn.remote_address();
        let opened = tokio::time::timeout(a.timeout, async {
            let quic = self.quic.session_over(conn.clone()).await?;
            self.session(quic, conn.clone(), kind).await
        })
        .await;
        match opened {
            Ok(Ok(s)) => {
                tracing::info!(link = %kind, %from, "using the connection the other agent opened");
                Some(s)
            }
            Ok(Err(e)) => {
                tracing::debug!(%from, "the other agent's connection did not take a session: {e}");
                conn.close("no session");
                None
            }
            Err(_) => {
                tracing::debug!(%from, "the other agent's connection did not answer in time");
                conn.close("no answer");
                None
            }
        }
    }

    /// Serves what the other agent opens over the connections this side dials, and adopts
    /// the ones it opens when this side cannot dial it (T-237).
    #[must_use]
    fn with_peer(mut self, agent: Arc<Agent>, peer: PeerSide, timeout: Duration) -> Self {
        self.adoption = Some(Adoption {
            agent,
            peer,
            timeout,
        });
        self
    }

    /// What to do if the other machine rejects our key.
    #[must_use]
    pub fn with_rejected(mut self, f: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.rejected = Some(f);
        self
    }

    pub fn new(
        endpoint: Arc<Endpoint>,
        me: Machine,
        candidates: &[LinkCandidate],
        qos: LinkQos,
    ) -> Self {
        AgentProber {
            quic: QuicProber::new(endpoint, Hello::new(Peer::Agent(me))),
            me,
            kinds: candidates.to_vec(),
            qos,
            interfaces: None,
            rejected: None,
            adoption: None,
        }
    }

    #[must_use]
    pub fn with_interfaces(mut self, interfaces: Arc<dyn Interfaces>) -> Self {
        self.interfaces = Some(interfaces);
        self
    }

    /// The type of the connection to `addr`: the known candidate's or, if it appeared later,
    /// its interface's. When in doubt, Wi-Fi.
    fn kind_of(&self, addr: SocketAddr) -> LinkKind {
        self.kinds
            .iter()
            .find(|c| c.addr == addr)
            .map(|c| c.kind)
            .or_else(|| {
                let list = self.interfaces.as_ref()?.list();
                kind_for_addr(&list, addr)
            })
            .unwrap_or(LinkKind::Wifi)
    }
}

impl Prober for AgentProber {
    type Session = AgentSession;
    type Handle = PeerHandle;
    type Error = PeerError;

    async fn connect(&self, addr: SocketAddr) -> Result<AgentSession, PeerError> {
        // The other agent's connection first: it is the only one when this side's packets
        // never reach the other, and dialling would only find that out at the timeout.
        if let Some(s) = self.adopt(addr, self.kind_of(addr)).await {
            if let Some(a) = &self.adoption {
                a.peer.settled.accepted();
            }
            return Ok(s);
        }
        if self.adoption.as_ref().is_some_and(|a| !a.peer.dial) {
            // As a firewall that drops everything: no answer until the manager's timeout.
            return std::future::pending().await;
        }
        let opened = self.open(addr).await;
        let settled = self.adoption.as_ref().map(|a| &a.peer.settled);
        match &opened {
            Ok(_) => {
                if let Some(s) = settled {
                    s.accepted();
                }
            }
            Err(PeerError::Transport(e)) => {
                tracing::debug!(%addr, error = ?e, "connection to the other agent failed");
                if e.is_peer_rejection() {
                    if settled.is_none_or(|s| s.rejection_means_forgotten(Instant::now())) {
                        if let Some(rejected) = &self.rejected {
                            rejected();
                        }
                    } else {
                        tracing::info!(
                            %addr,
                            "the other machine does not accept this one yet: it is still saving the pairing"
                        );
                    }
                }
            }
            Err(_) => {}
        }
        opened
    }

    async fn ping(&self, s: &mut AgentSession) -> Result<(), PeerError> {
        Ok(self.quic.ping(&mut s.quic).await?)
    }

    fn handle(s: &AgentSession) -> PeerHandle {
        s.handle.clone()
    }
}

/// The other agent seen by the orchestrator: each request goes over the active link at
/// that moment and fails with `PEER_LOST` if that link drops before the reply.
pub struct ManagedPeer {
    links: LinkManager<PeerHandle>,
}

impl ManagedPeer {
    pub fn manager(&self) -> &LinkManager<PeerHandle> {
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
        Some((link_of(active.kind), rtt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-291: a rejection right after pairing here is the other side still saving the pairing;
    /// from a peer on disk, or once the other accepted this side, it means it forgot this one.
    #[test]
    fn a_rejection_means_forgotten_only_once_the_pairing_settled() {
        let now = Instant::now();
        assert!(Settled::from_disk().rejection_means_forgotten(now));
        let fresh = Settled::just_paired();
        let paired = fresh.paired_at.unwrap();
        assert!(!fresh.rejection_means_forgotten(paired));
        assert!(!fresh.rejection_means_forgotten(paired + PAIRING_SETTLE / 2));
        // The other side never saved it: both end up unpaired instead of one waiting forever.
        assert!(fresh.rejection_means_forgotten(paired + PAIRING_SETTLE));
        fresh.accepted();
        assert!(fresh.rejection_means_forgotten(paired));
    }

    fn link(kind: LinkKind, addr: &str) -> LinkCandidate {
        LinkCandidate {
            kind,
            addr: addr.parse().unwrap(),
        }
    }

    #[test]
    fn binds_dual_stack_even_when_every_static_link_is_ipv4() {
        assert_eq!(default_bind(47770), "[::]:47770".parse().unwrap());
    }

    #[tokio::test]
    async fn the_default_endpoint_dials_an_ipv6_peer_with_ipv4_static_links() {
        use std::sync::Arc;
        use tandem_link::identity::{Identity, PublicKey};
        use tandem_link::transport::{Endpoint, PeerTrust};
        struct Anyone;
        impl PeerTrust for Anyone {
            fn is_trusted(&self, _: &PublicKey) -> bool {
                true
            }
        }
        // The hardware run: peer configured as `169.254.x.y`, IPv6 link-local from mDNS.
        let ours = Endpoint::bind(
            default_bind(0),
            &Identity::from_seed([1; 32]),
            Arc::new(Anyone),
        )
        .unwrap();
        let theirs = Endpoint::bind(
            "[::1]:0".parse().unwrap(),
            &Identity::from_seed([2; 32]),
            Arc::new(Anyone),
        )
        .unwrap();
        let port = theirs.local_addr().unwrap().port();
        let to = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port);
        let dialled = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(ours.connect(to), theirs.accept())
        })
        .await
        .unwrap();
        dialled.0.unwrap();
        dialled.1.unwrap().unwrap();
    }

    #[test]
    fn an_incoming_connection_takes_the_kind_of_its_static_address_or_wifi() {
        let statics = [
            link(LinkKind::Usb4, "10.77.1.2:47770"),
            link(LinkKind::Ethernet, "[fe80::1]:47770"),
        ];
        let from = |a: &str| kind_of_remote(&statics, a.parse().unwrap());
        // The origin port does not matter: the other agent can go out through another.
        assert_eq!(from("10.77.1.2:52000"), LinkKind::Usb4);
        assert_eq!(from("[::ffff:10.77.1.2]:47770"), LinkKind::Usb4);
        assert_eq!(from("[fe80::1]:1"), LinkKind::Ethernet);
        assert_eq!(from("192.168.1.20:47770"), LinkKind::Wifi);
        assert_eq!(
            kind_of_remote(&[], "10.77.1.2:1".parse().unwrap()),
            LinkKind::Wifi
        );
    }

    fn status(kind: LinkKind, addr: &str) -> LinkStatus {
        LinkStatus {
            kind,
            addr: addr.parse().unwrap(),
            rtt: None,
        }
    }

    /// A discovery change never rebuilds a running manager (which cut every connection and
    /// the input and clipboard sessions over them, ~680 times a day on 2026-11-23): only the
    /// links it does not follow yet, or follows on another address, are handed to it, and it
    /// keeps a healthy link's connection. Nothing is removed for a discovery change: a cable
    /// leaves when its interface does, and an empty mDNS answer removes nothing.
    #[test]
    fn a_discovery_change_only_adds_what_the_manager_does_not_follow() {
        let have = [
            status(LinkKind::Usb4, "[fe80::c0c:1e4f:2370:bfc5%21]:52000"),
            status(LinkKind::Wifi, "192.168.1.191:52000"),
        ];
        let same = [
            link(LinkKind::Wifi, "192.168.1.191:52000"),
            link(LinkKind::Usb4, "[fe80::c0c:1e4f:2370:bfc5%21]:52000"),
        ];
        assert!(links_to_add(&same, &have).is_empty());
        // Fewer links (an mDNS answer without the cable, or none at all): nothing to do.
        assert!(links_to_add(&same[..1], &have).is_empty());
        assert!(links_to_add(&[], &have).is_empty());
        // Another address for a followed link and a new link: both go to the manager.
        let moved = link(LinkKind::Usb4, "[fe80::1c46:56e:2a49:fb65%21]:52000");
        let eth = link(LinkKind::Ethernet, "10.0.0.2:52000");
        assert_eq!(
            links_to_add(&[moved, same[0], eth], &have),
            vec![moved, eth]
        );
    }

    struct Anyone;
    impl tandem_link::transport::PeerTrust for Anyone {
        fn is_trusted(&self, _: &PublicKey) -> bool {
            true
        }
    }

    /// `n` connections from the agent with `seed` to a fresh endpoint, as that endpoint
    /// accepts them, and what keeps them open while the test runs: the endpoints and the
    /// dialling side's handles (dropping them closes the connection).
    async fn inbound_from(
        seed: u8,
        n: usize,
    ) -> (Vec<Connection>, (Endpoint, Endpoint, Vec<Connection>)) {
        let ours = Endpoint::bind(
            "127.0.0.1:0".parse().unwrap(),
            &Identity::from_seed([100; 32]),
            Arc::new(Anyone),
        )
        .unwrap();
        let theirs = Endpoint::bind(
            "127.0.0.1:0".parse().unwrap(),
            &Identity::from_seed([seed; 32]),
            Arc::new(Anyone),
        )
        .unwrap();
        let to = ours.local_addr().unwrap();
        let (mut accepted, mut dialled) = (Vec::new(), Vec::new());
        for _ in 0..n {
            let (out, got) = tokio::join!(theirs.connect(to), ours.accept());
            dialled.push(out.unwrap());
            accepted.push(got.unwrap().unwrap());
        }
        (accepted, (ours, theirs, dialled))
    }

    #[tokio::test]
    async fn an_inbound_connection_is_found_by_ip_then_by_kind_and_the_newest_wins() {
        let (accepted, _keep) = inbound_from(7, 2).await;
        let key = accepted[0].peer();
        let conns = Conns::default();
        let first = conns.add(accepted[0].clone(), LinkKind::Usb4, true);
        conns.add(accepted[1].clone(), LinkKind::Usb4, true);
        let from = accepted[0].remote_address();
        // Same IP (any port): the newest of the two.
        let got = conns.inbound(&key, from, LinkKind::Wifi).unwrap();
        assert_eq!(got.id(), accepted[1].id());
        // Another IP: by kind, and not over another kind.
        let elsewhere: SocketAddr = "10.9.9.9:1".parse().unwrap();
        assert!(conns.inbound(&key, elsewhere, LinkKind::Usb4).is_some());
        assert!(conns.inbound(&key, elsewhere, LinkKind::Ethernet).is_none());
        // Not another peer's, not a closed one, not one this side dialled.
        let stranger = Identity::from_seed([8; 32]).public_key();
        assert!(conns.inbound(&stranger, from, LinkKind::Usb4).is_none());
        accepted[1].close("test");
        assert_eq!(
            conns.inbound(&key, from, LinkKind::Usb4).unwrap().id(),
            accepted[0].id()
        );
        conns.remove(first);
        assert!(conns.inbound(&key, from, LinkKind::Usb4).is_none());
        let dialled = Conns::default();
        dialled.add(accepted[0].clone(), LinkKind::Usb4, false);
        assert!(dialled.inbound(&key, from, LinkKind::Usb4).is_none());
        assert!(dialled.inbound_paths(&key).is_empty());
    }

    #[tokio::test]
    async fn an_inbound_path_is_dialled_back_as_plain_ipv4() {
        let (accepted, _keep) = inbound_from(9, 1).await;
        let key = accepted[0].peer();
        let conns = Conns::default();
        let arrived = conns.arrived.subscribe();
        conns.add(accepted[0].clone(), LinkKind::Ethernet, true);
        assert!(arrived.has_changed().unwrap(), "supervise is told");
        let paths = conns.inbound_paths(&key);
        assert_eq!(
            paths,
            [LinkCandidate {
                kind: LinkKind::Ethernet,
                addr: accepted[0].remote_address(),
            }]
        );
        assert!(paths[0].addr.is_ipv4());
        assert_eq!(
            unmapped("::ffff:169.254.93.252".parse().unwrap()),
            "169.254.93.252".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn every_link_kind_has_its_ui_name() {
        assert_eq!(
            LinkKind::ALL.map(link_of),
            [Link::Usb4, Link::Ethernet, Link::Wifi]
        );
    }
}
