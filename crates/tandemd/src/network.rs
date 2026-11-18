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
//!   ([`AgentProber`]).
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
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, Weak};
use std::time::Duration;

use tandem_core::{Machine, Os, State, Step};
use tandem_link::discovery::MdnsDiscovery;
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_link::iface::{
    AssignNotices, Interfaces, ScopedDiscovery, SystemAssigner, SystemInterfaces, WiredChange,
    WiredResolver, follow_wired, kind_for_addr, relabel, wired_changes,
};
use tandem_link::manager::{
    HealthConfig, LinkCandidate, LinkKind, LinkManager, ManagerError, Prober, QuicProber,
    QuicSession,
};
use tandem_link::metrics;
use tandem_link::qos::LinkQos;
use tandem_link::transport::{Connection, Endpoint, TransportError};
use tandem_proto::{ControlMsg, Hello, Link, NetworkNotice, PairingStatus, Peer, PeerStatus};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

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
}

impl Default for NetOptions {
    fn default() -> Self {
        NetOptions {
            bind: None,
            fallback_ports: FALLBACK_PORTS.to_vec(),
            mdns: true,
            health: HealthConfig::default(),
            pairing: PairingTimeouts::default(),
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

/// All interfaces on `port`. IPv6 only if some fixed link is IPv6: an IPv6
/// socket does not receive IPv4 on every system (on Windows it is IPv6-only by default), and
/// the direct links of DESIGN §6 are IPv4.
pub fn default_bind(port: u16, statics: &[LinkCandidate]) -> SocketAddr {
    let ip = if statics.iter().any(|l| l.addr.is_ipv6()) {
        IpAddr::V6(Ipv6Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    };
    SocketAddr::new(ip, port)
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

/// The connections the other agent opened, to cut them when unpairing: the
/// pinning only looks at trust in the handshake.
#[derive(Default)]
struct Conns {
    next: AtomicU64,
    open: Mutex<BTreeMap<u64, Connection>>,
}

impl Conns {
    fn add(&self, conn: Connection) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        lock(&self.open).insert(id, conn);
        id
    }

    fn remove(&self, id: u64) {
        lock(&self.open).remove(&id);
    }

    fn close_from(&self, key: &PublicKey) {
        for conn in lock(&self.open).values().filter(|c| c.peer() == *key) {
            conn.close("unpaired");
        }
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
        let wanted = opts
            .bind
            .unwrap_or_else(|| default_bind(net_cfg.port, &net_cfg.peers));
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
            Some(peer) => shared.follow(peer),
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
    fn follow(self: &Arc<Self>, peer: TrustedPeer) {
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
        let tasks = vec![
            tokio::spawn(supervise(
                agent.clone(),
                self.endpoint.clone(),
                name,
                discovery,
                self.opts.health,
                reachable_tx,
                link,
                self.interfaces.clone(),
            )),
            // Both watch the other: either can be left alone (DESIGN §3.7).
            tokio::spawn(crate::peer_watch::watch(agent.clone(), reachable)),
        ];
        *following = Some(Following {
            key: public_key,
            tasks,
        });
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
            .map(|a| a.handle.client);
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
            Some(client) => {
                client.send(ControlMsg::Unpaired);
                let conns = self.conns.clone();
                tokio::spawn(async move {
                    let _ = tokio::time::timeout(GOODBYE_WAIT, client.flush()).await;
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
                    shared.follow(peer);
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
                let id = conns.add(conn.clone());
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
    let ip = |a: SocketAddr| match a.ip() {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    };
    statics
        .iter()
        .find(|c| ip(c.addr) == ip(remote))
        .map_or(LinkKind::Wifi, |c| c.kind)
}

/// Builds the link manager with the candidates of each moment and follows its active
/// link. If what is discovered on the LAN changes the candidates, it is rebuilt: the
/// manager does not admit new links while running, and the fixed links' connections are
/// reopened in one ping.
#[allow(clippy::too_many_arguments)]
async fn supervise(
    agent: Arc<Agent>,
    endpoint: Arc<Endpoint>,
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
    let mut scoped_seen = scoped_snapshot(&raw(&discovered));
    loop {
        // The cables already plugged in enter with the manager; the ones that appear or
        // disconnect later are followed by `follow_wired`. What is configured by hand rules.
        let side = agent.machine();
        let port = agent.config().network.port;
        let fixed = relabel(&candidates, &interfaces.list());
        let fixed_kinds: Vec<LinkKind> = fixed.iter().map(|c| c.kind).collect();
        let resolver = Arc::new(WiredResolver {
            side,
            os: local_os(),
            port,
            discovery: scoped.clone(),
            assigner: Arc::new(SystemAssigner),
            notices: notices.clone(),
        });
        let resolve = move |i: &tandem_link::iface::NetInterface| {
            if fixed_kinds.contains(&i.kind.link_kind()) {
                return None;
            }
            resolver.resolve(i)
        };
        let mut spawned = fixed;
        let mut applied = BTreeMap::new();
        for change in wired_changes(&mut applied, &interfaces.list(), &resolve) {
            if let WiredChange::Add(c) = change {
                spawned.push(c);
            }
        }
        let managed = match spawn_manager(
            &agent,
            &endpoint,
            &spawned,
            health,
            &interfaces,
            &active_link,
        ) {
            Ok(m) => Some(m),
            Err(ManagerError::NoCandidates) => {
                tracing::info!("no links with the peer: waiting to discover it on the LAN");
                None
            }
            Err(e) => {
                tracing::error!("could not start the link manager: {e}");
                None
            }
        };
        let mut links = managed.as_ref().map(|m| m.links.watch());
        let _follow = managed
            .as_ref()
            .map(|m| AbortOnDrop(follow_wired(&*interfaces, m.links.control(), resolve)));
        let mut installed = false;
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
                        tracing::info!(?next, "the links with the peer changed");
                        candidates = next;
                        break;
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
    agent: &Agent,
    endpoint: &Arc<Endpoint>,
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
                    Ok(()) => tracing::info!("the other machine no longer trusts this one: unpaired"),
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
    .with_rejected(rejected);
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
        let client = Arc::new(PeerClient::open(&conn, self.me).await?);
        // The first thing the other side knows: how this side classifies the connection.
        client.send(ControlMsg::LinkClass { kind: kind.into() });
        Ok(AgentSession {
            quic,
            handle: PeerHandle { conn, client },
        })
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
        let opened = self.open(addr).await;
        if let Err(PeerError::Transport(e)) = &opened {
            tracing::debug!(%addr, error = ?e, "connection to the other agent failed");
            if e.is_peer_rejection()
                && let Some(rejected) = &self.rejected
            {
                rejected();
            }
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

    fn link(kind: LinkKind, addr: &str) -> LinkCandidate {
        LinkCandidate {
            kind,
            addr: addr.parse().unwrap(),
        }
    }

    #[test]
    fn binds_ipv4_unless_a_static_link_is_ipv6() {
        assert_eq!(default_bind(47770, &[]), "0.0.0.0:47770".parse().unwrap());
        let v4 = [link(LinkKind::Usb4, "10.77.1.2:47770")];
        assert_eq!(default_bind(47770, &v4), "0.0.0.0:47770".parse().unwrap());
        let mixed = [
            link(LinkKind::Usb4, "10.77.1.2:47770"),
            link(LinkKind::Ethernet, "[fe80::1]:47770"),
        ];
        assert_eq!(default_bind(1, &mixed), "[::]:1".parse().unwrap());
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

    #[test]
    fn every_link_kind_has_its_ui_name() {
        assert_eq!(
            LinkKind::ALL.map(link_of),
            [Link::Usb4, Link::Ethernet, Link::Wifi]
        );
    }
}
