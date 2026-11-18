//! Pairing from the UI (DESIGN §9): find the other agent, show the same
//! 6-digit code on both screens and, if the user confirms it on both,
//! save the other's key in trust.
//!
//! - It goes over a separate endpoint ([`Endpoint::bind_pairing`]), which accepts unknown
//!   keys and exists only while pairing. The normal endpoint still does not
//!   accept anyone who is not in trust.
//! - Both machines do the same (DECISIONS D1/D2: neither is special): they wait on
//!   their pairing port (and announce it over mDNS) and look for the other over the
//!   config's fixed links and over mDNS. When they find each other, both know both
//!   keys and with them the sides (A1): only the connection opened by `A` counts, and on it `A`
//!   starts the protocol. `B` cuts the ones it opens and stops looking. The user opens
//!   pairing on both, in any order.
//! - The protocol is the commitment one from [`tandem_link::identity`]; the keys that are
//!   exchanged have to be the same ones the connection authenticated.
//! - Each side sends its [`PairingMsg::Confirm`] when its user confirms and saves
//!   trust when it has both confirmations. Rejecting on either
//!   side (or the time running out) ends without saving anything on either.

use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tandem_core::Machine;
use tandem_link::discovery::MdnsDiscovery;
use tandem_link::identity::{
    Identity, IdentityError, Pairing, PairingAnswer, PairingCode, PairingCommitment,
    PairingInitiator, PairingNonce, PairingOffer, PairingResponder, PairingReveal, PublicKey,
    TrustStore, TrustedPeer,
};
use tandem_link::transport::{
    Connection, ControlReceiver, ControlSender, ControlStream, Endpoint, TransportError,
};
use tandem_proto::{Hello, PairingMsg, PairingStatus, Peer};
use tokio::sync::mpsc;

use crate::agent::Agent;

/// What is waited for in each phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairingTimeouts {
    /// Look for the other machine (which also has to be pairing).
    pub search: Duration,
    /// Wait for both users to confirm the code.
    pub confirm: Duration,
}

impl Default for PairingTimeouts {
    fn default() -> Self {
        PairingTimeouts {
            search: Duration::from_secs(120),
            confirm: Duration::from_secs(120),
        }
    }
}

/// The three protocol messages, not counting the user's wait.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait for a connection attempt before giving it up; meanwhile, others
/// keep being launched (see [`look`]).
const CONNECT_ATTEMPT: Duration = Duration::from_secs(10);
const RETRY_EVERY: Duration = Duration::from_millis(300);
/// How long to wait for the other side to receive the last message before cutting.
const CLOSE_WAIT: Duration = Duration::from_secs(2);
/// The other's name is shown in the UI: an absurd one is trimmed.
const MAX_NAME_CHARS: usize = 64;

/// What this machine's user answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Confirm,
    Cancel,
}

/// How a pairing ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Saved in trust.
    Paired(TrustedPeer),
    /// This machine's user canceled or rejected it.
    Cancelled,
    /// Not paired; the reason is for the UI.
    Failed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error("{0}")]
    Transport(#[from] TransportError),
    #[error("{0}")]
    Identity(#[from] IdentityError),
    #[error("expected the agent of machine {expected}, got {got:?}")]
    UnexpectedPeer { expected: Machine, got: Peer },
    #[error("the other end has the same key as this machine")]
    SameKey,
    #[error("the pairing key is not the connection's")]
    KeyMismatch,
    #[error("unexpected pairing message")]
    Unexpected,
    #[error("the other machine cut the pairing")]
    Closed,
    #[error("the other machine stopped responding")]
    Timeout,
}

/// What a pairing needs.
pub struct Ctx {
    pub agent: Arc<Agent>,
    pub identity: Identity,
    pub trust: Arc<RwLock<TrustStore>>,
    /// The normal endpoint's IP: the pairing one uses the same family.
    pub bind_ip: IpAddr,
    /// The normal endpoint's real port (it may not be the config's, if it was busy).
    pub port: u16,
    /// The network's reserve ones ([`crate::network::NetOptions::fallback_ports`]).
    pub fallback_ports: Vec<u16>,
    pub mdns: bool,
    pub timeouts: PairingTimeouts,
}

/// Where this machine waits when pairing, in order: the config's `pairing_port` if it was
/// fixed, otherwise the one after the agent's real port; then the one after each reserve port.
/// The other finds it through the mDNS announcement, which carries the real port.
pub fn pairing_ports(net: &crate::NetworkConfig, port: u16, fallbacks: &[u16]) -> Vec<u16> {
    let next = |p: u16| p.checked_add(1).unwrap_or(p - 1);
    let first = net.pairing_port.unwrap_or_else(|| next(port));
    let mut out = vec![first];
    for p in fallbacks.iter().map(|p| next(*p)) {
        if p != port && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

impl Ctx {
    /// This machine's side relative to whoever is on the other side of `conn`: while
    /// pairing there is no peer yet, so it comes from the connection's two keys.
    fn side_on(&self, conn: &Connection) -> Result<Machine, PairError> {
        Machine::of_keys(
            self.identity.public_key().as_bytes(),
            conn.peer().as_bytes(),
        )
        .ok_or(PairError::SameKey)
    }

    fn publish(&self, status: PairingStatus) {
        self.agent.set_pairing_status(status);
    }

    /// The name this machine introduces itself with to the other (and the one the other saves it under).
    fn name(&self) -> String {
        self.agent.local_info().name
    }
}

/// A complete pairing: look, compare, save. Publishes each step to the
/// agent; the final result is published by the caller.
pub async fn run(ctx: &Ctx, answers: &mut mpsc::UnboundedReceiver<Answer>) -> Outcome {
    ctx.publish(PairingStatus::Searching);
    let found = tokio::select! {
        found = tokio::time::timeout(ctx.timeouts.search, search(ctx)) => found,
        cancelled = cancelled(answers) => {
            // Without a connection there is nobody to notify.
            let () = cancelled;
            return Outcome::Cancelled;
        }
    };
    let (conn, stream, pairing) = match found {
        Ok(Ok(found)) => found,
        Ok(Err(e)) => return Outcome::Failed(e),
        Err(_) => {
            return Outcome::Failed(
                "the other machine did not appear (is pairing open over there?)".into(),
            );
        }
    };
    let outcome = compare(ctx, stream, pairing, answers).await;
    drop(conn);
    outcome
}

/// Waits for the user to cancel; confirming while searching means nothing.
async fn cancelled(answers: &mut mpsc::UnboundedReceiver<Answer>) {
    loop {
        match answers.recv().await {
            Some(Answer::Confirm) => continue,
            Some(Answer::Cancel) | None => return,
        }
    }
}

type Found = (Connection, ControlStream, Pairing);

/// Waits for the other and looks for it at the same time, with a single endpoint (the same port serves and
/// calls). Ends with the first connection opened by `A` on which the protocol goes well.
async fn search(ctx: &Ctx) -> Result<Found, String> {
    let ports = pairing_ports(&ctx.agent.config().network, ctx.port, &ctx.fallback_ports);
    let (endpoint, addr) = crate::network::bind_first(ctx.bind_ip, &ports, |addr| {
        Endpoint::bind_pairing(addr, &ctx.identity)
    })
    .map_err(|e| format!("could not open the pairing: {e}"))?;
    let port = endpoint
        .local_addr()
        .map(|a| a.port())
        .unwrap_or(addr.port());
    tracing::info!(%addr, "pairing: waiting for and looking for the other machine");
    let discovery = if ctx.mdns {
        MdnsDiscovery::pairing(&ctx.identity.public_key(), Some(port))
            .map_err(|e| tracing::warn!("no mDNS, only searching over the fixed links: {e}"))
            .ok()
    } else {
        None
    };
    let lan = discovery.as_ref().map(MdnsDiscovery::peer_addrs);
    let endpoint = Arc::new(endpoint);
    tokio::select! {
        found = wait(ctx, &endpoint) => found,
        found = look(ctx, &endpoint, lan, port) => found,
    }
}

/// Serves the connections that arrive: only the one opened by `A` counts, so this machine
/// answers if it is `B` relative to the caller.
async fn wait(ctx: &Ctx, endpoint: &Endpoint) -> Result<Found, String> {
    loop {
        let conn = match endpoint.accept().await {
            Some(Ok(conn)) => conn,
            Some(Err(e)) => {
                tracing::debug!("pairing connection failed: {e}");
                continue;
            }
            None => return Err("the pairing port closed".into()),
        };
        match ctx.side_on(&conn) {
            Ok(Machine::B) => {}
            // This machine is A: the connection that counts is the one it opens.
            Ok(Machine::A) | Err(_) => {
                conn.close("pairing: the other calls");
                continue;
            }
        }
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, respond(ctx, &conn)).await {
            Ok(Ok((stream, pairing))) => return Ok((conn, stream, pairing)),
            Ok(Err(e)) => {
                tracing::warn!(from = %conn.remote_address(), "pairing rejected: {e}")
            }
            Err(_) => tracing::warn!(from = %conn.remote_address(), "pairing without a reply"),
        }
    }
}

/// Tries the other's addresses until one answers. If this machine turns out to be
/// `B`, it stops looking: the other (A) calls it and [`wait`] serves it.
///
/// Each attempt runs apart and is not cut by starting another: with the machine loaded (or on
/// Windows, where a port with nobody on it does not fail right away) a handshake can take longer than
/// the interval between attempts, and cutting it every time left both searching for each other forever.
async fn look(
    ctx: &Ctx,
    endpoint: &Arc<Endpoint>,
    lan: Option<tokio::sync::watch::Receiver<Vec<SocketAddr>>>,
    own: u16,
) -> Result<Found, String> {
    let mut attempts = tokio::task::JoinSet::new();
    let mut trying = BTreeSet::new();
    loop {
        let discovered = lan.as_ref().map(|l| l.borrow().clone()).unwrap_or_default();
        for addr in candidates(ctx, &discovered, own) {
            if trying.insert(addr) {
                let endpoint = endpoint.clone();
                attempts.spawn(async move {
                    let r = tokio::time::timeout(CONNECT_ATTEMPT, endpoint.connect(addr)).await;
                    (addr, r)
                });
            }
        }
        let done = tokio::select! {
            Some(done) = attempts.join_next() => done,
            () = tokio::time::sleep(RETRY_EVERY) => continue,
        };
        let Ok((addr, result)) = done else { continue };
        trying.remove(&addr);
        let conn = match result {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => {
                tracing::debug!(%addr, "the other machine does not answer the pairing: {e}");
                continue;
            }
            Err(_) => continue,
        };
        match ctx.side_on(&conn) {
            Ok(Machine::A) => {}
            Ok(Machine::B) => {
                conn.close("pairing: the other calls");
                tracing::debug!(%addr, "this machine is B: waiting for the other to call");
                return std::future::pending().await;
            }
            // This very machine.
            Err(_) => {
                conn.close("pairing: it is this very machine");
                continue;
            }
        }
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, initiate(ctx, &conn)).await {
            Ok(Ok((stream, pairing))) => return Ok((conn, stream, pairing)),
            Ok(Err(e)) => tracing::warn!(%addr, "pairing rejected: {e}"),
            Err(_) => tracing::warn!(%addr, "pairing without a reply"),
        }
    }
}

/// Where to look for the other: the IP of each fixed link with its pairing port (the
/// one after its agent's, and the one this one's config says) and what it announces over mDNS,
/// without repeats.
fn candidates(ctx: &Ctx, discovered: &[SocketAddr], own: u16) -> Vec<SocketAddr> {
    let net = &ctx.agent.config().network;
    let mine = net.pairing_port();
    let mut seen = BTreeSet::new();
    // The own pairing port on loopback would be this very machine: calling itself
    // is useless and on Windows it tangles the endpoint.
    let itself = |a: &SocketAddr| a.ip().is_loopback() && a.port() == own;
    net.peers
        .iter()
        .flat_map(|l| {
            let next = l.addr.port().checked_add(1).unwrap_or(l.addr.port() - 1);
            [
                SocketAddr::new(l.addr.ip(), next),
                SocketAddr::new(l.addr.ip(), mine),
            ]
        })
        .chain(discovered.iter().copied())
        .filter(|a| !itself(a) && seen.insert(*a))
        .collect()
}

fn check_peer(me: Machine, got: Peer) -> Result<(), PairError> {
    if got == Peer::Agent(me.other()) {
        Ok(())
    } else {
        Err(PairError::UnexpectedPeer {
            expected: me.other(),
            got,
        })
    }
}

/// The key the other claims to have must be the one that signed the handshake: this way the
/// code the users compare also authenticates this connection.
fn their_key(conn: &Connection, bytes: [u8; 32]) -> Result<PublicKey, PairError> {
    let key = PublicKey::from_bytes(bytes)?;
    if key == conn.peer() {
        Ok(key)
    } else {
        Err(PairError::KeyMismatch)
    }
}

fn clean_name(name: String) -> String {
    let name: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_CHARS)
        .collect();
    let name = name.trim();
    if name.is_empty() {
        "other machine".into()
    } else {
        name.to_owned()
    }
}

async fn next(stream: &mut ControlStream) -> Result<PairingMsg, PairError> {
    stream.recv().await?.ok_or(PairError::Closed)
}

/// The initiator (side A): offer with commitment, answer, reveal.
pub async fn initiate(ctx: &Ctx, conn: &Connection) -> Result<(ControlStream, Pairing), PairError> {
    let me = ctx.side_on(conn)?;
    let (mut stream, theirs) = conn.open_control(&Hello::new(Peer::Agent(me))).await?;
    check_peer(me, theirs.peer)?;
    let (initiator, offer) = PairingInitiator::start(
        ctx.identity.public_key(),
        ctx.name(),
        PairingNonce::generate()?,
    );
    stream
        .send(&PairingMsg::Offer {
            public_key: *offer.public_key.as_bytes(),
            name: offer.name,
            commitment: *offer.commitment.as_bytes(),
        })
        .await?;
    let PairingMsg::Answer {
        public_key,
        name,
        nonce,
    } = next(&mut stream).await?
    else {
        return Err(PairError::Unexpected);
    };
    let answer = PairingAnswer {
        public_key: their_key(conn, public_key)?,
        name: clean_name(name),
        nonce: PairingNonce::from_bytes(nonce),
    };
    let (pairing, reveal) = initiator.receive_answer(answer)?;
    stream
        .send(&PairingMsg::Reveal {
            nonce: *reveal.nonce.as_bytes(),
        })
        .await?;
    Ok((stream, pairing))
}

/// The responder (side B).
pub async fn respond(ctx: &Ctx, conn: &Connection) -> Result<(ControlStream, Pairing), PairError> {
    let me = ctx.side_on(conn)?;
    let (mut stream, theirs) = conn.accept_control(&Hello::new(Peer::Agent(me))).await?;
    check_peer(me, theirs.peer)?;
    let PairingMsg::Offer {
        public_key,
        name,
        commitment,
    } = next(&mut stream).await?
    else {
        return Err(PairError::Unexpected);
    };
    let offer = PairingOffer {
        public_key: their_key(conn, public_key)?,
        name: clean_name(name),
        commitment: PairingCommitment::from_bytes(commitment),
    };
    let (responder, answer) = PairingResponder::receive_offer(
        ctx.identity.public_key(),
        ctx.name(),
        offer,
        PairingNonce::generate()?,
    )?;
    stream
        .send(&PairingMsg::Answer {
            public_key: *answer.public_key.as_bytes(),
            name: answer.name,
            nonce: *answer.nonce.as_bytes(),
        })
        .await?;
    let PairingMsg::Reveal { nonce } = next(&mut stream).await? else {
        return Err(PairError::Unexpected);
    };
    let pairing = responder.receive_reveal(PairingReveal {
        nonce: PairingNonce::from_bytes(nonce),
    })?;
    Ok((stream, pairing))
}

/// The two users compare the code. Ends on having both confirmations (and
/// then saves trust), or on the first rejection, cut or timeout.
pub async fn compare(
    ctx: &Ctx,
    stream: ControlStream,
    mut pairing: Pairing,
    answers: &mut mpsc::UnboundedReceiver<Answer>,
) -> Outcome {
    let code = pairing.code();
    let peer = pairing.remote().name.clone();
    let comparing = |confirmed| PairingStatus::Comparing {
        code: code.to_string(),
        peer: peer.clone(),
        confirmed,
    };
    ctx.publish(comparing(false));
    tracing::info!(peer = %peer, "comparing the pairing code");
    let (mut tx, mut rx) = stream.split();
    let deadline = tokio::time::sleep(ctx.timeouts.confirm);
    tokio::pin!(deadline);
    let mut local = false;
    while !pairing.is_confirmed() {
        tokio::select! {
            answer = answers.recv() => match answer {
                Some(Answer::Confirm) if !local => {
                    local = true;
                    pairing.confirm_local();
                    if let Err(e) = tx.send(&PairingMsg::Confirm { code: code.value() }).await {
                        return lost(&e.into());
                    }
                    ctx.publish(comparing(true));
                }
                Some(Answer::Confirm) => {}
                Some(Answer::Cancel) | None => {
                    reject(&mut tx, &mut rx).await;
                    return Outcome::Cancelled;
                }
            },
            msg = rx.recv::<PairingMsg>() => match msg {
                Ok(Some(PairingMsg::Confirm { code: theirs })) => {
                    let ok = PairingCode::from_value(theirs)
                        .is_some_and(|c| pairing.confirm_remote(c).is_ok());
                    if !ok {
                        reject(&mut tx, &mut rx).await;
                        return Outcome::Failed(
                            "the other machine confirmed another code: not paired".into(),
                        );
                    }
                }
                Ok(Some(PairingMsg::Reject)) => {
                    return Outcome::Failed(format!("rejected at {peer}: not paired"));
                }
                Ok(Some(_)) => {
                    reject(&mut tx, &mut rx).await;
                    return Outcome::Failed(PairError::Unexpected.to_string());
                }
                Ok(None) => return lost(&PairError::Closed),
                Err(e) => return lost(&e.into()),
            },
            () = &mut deadline => {
                reject(&mut tx, &mut rx).await;
                return Outcome::Failed("time ran out to confirm the code".into());
            }
        }
    }
    // The side, before trusting the other: until trust is saved the normal endpoint
    // does not accept it, so nothing of its (its desk, with the sides already right) arrives
    // before the side switch and is flipped twice.
    if let Some(side) = Machine::of_keys(
        ctx.identity.public_key().as_bytes(),
        pairing.remote().public_key.as_bytes(),
    ) {
        ctx.agent.adopt_side(side);
    }
    let saved = {
        let mut trust = ctx.trust.write().unwrap_or_else(|e| e.into_inner());
        pairing.complete(&mut trust)
    };
    // Let the other side receive the confirmation before cutting.
    let _ = tx.finish();
    drain(&mut rx).await;
    match saved {
        Ok(peer) => {
            tracing::info!(peer = %peer.name, key = %peer.public_key.to_hex(), "paired");
            Outcome::Paired(peer)
        }
        Err(e) => Outcome::Failed(format!("could not save trust: {e}")),
    }
}

fn lost(e: &PairError) -> Outcome {
    tracing::warn!("pairing cut: {e}");
    Outcome::Failed("the connection to the other machine was lost: not paired".into())
}

/// Notifies the other side and gives it a moment to read it.
async fn reject(tx: &mut ControlSender, rx: &mut ControlReceiver) {
    if tx.send(&PairingMsg::Reject).await.is_ok() {
        let _ = tx.finish();
        drain(rx).await;
    }
}

/// Reads until the other side closes (or cuts, or [`CLOSE_WAIT`] passes).
async fn drain(rx: &mut ControlReceiver) {
    let _ = tokio::time::timeout(CLOSE_WAIT, async {
        while let Ok(Some(_)) = rx.recv::<PairingMsg>().await {}
    })
    .await;
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;
    use crate::StateStore;
    use crate::config::AgentConfig;

    const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

    fn ctx(machine: Machine, seed: u8, dir: &std::path::Path) -> Ctx {
        let cfg =
            AgentConfig::from_toml(&format!("machine = \"{machine}\"\nmonitors = []\n")).unwrap();
        let agent = Arc::new(Agent::new(cfg, StateStore::new(dir.join("state.json"))));
        let trust = TrustStore::open(dir.join(format!("trust-{seed}.toml"))).unwrap();
        Ctx {
            agent,
            identity: Identity::from_seed([seed; 32]),
            trust: Arc::new(RwLock::new(trust)),
            bind_ip: LOOPBACK,
            port: 0,
            fallback_ports: Vec::new(),
            mdns: false,
            timeouts: PairingTimeouts::default(),
        }
    }

    async fn connect(a: &Ctx, b: &Ctx) -> (Connection, Connection, Endpoint, Endpoint) {
        let ea = Endpoint::bind_pairing(SocketAddr::new(LOOPBACK, 0), &a.identity).unwrap();
        let eb = Endpoint::bind_pairing(SocketAddr::new(LOOPBACK, 0), &b.identity).unwrap();
        let (ca, cb) = tokio::join!(ea.connect(eb.local_addr().unwrap()), eb.accept());
        (ca.unwrap(), cb.unwrap().unwrap(), ea, eb)
    }

    #[test]
    fn names_are_cleaned_for_the_ui() {
        assert_eq!(clean_name("  PC\n".into()), "PC");
        assert_eq!(clean_name("\u{7}\t".into()), "other machine");
        assert_eq!(clean_name("x".repeat(500)).chars().count(), MAX_NAME_CHARS);
    }

    /// Each machine introduces itself with its `MachineInfo` name, never with fixed "Mac" or "PC"
    /// per side.
    #[test]
    fn each_machine_introduces_itself_by_its_own_name() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(Machine::A, 1, dir.path());
        assert_eq!(c.name(), c.agent.local_info().name);
        assert!(!c.name().is_empty());
    }

    #[test]
    fn each_machine_looks_for_the_other_on_every_static_link_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ctx(Machine::A, 1, dir.path());
        let cfg = AgentConfig::from_toml(
            r#"
            machine = "a"
            monitors = []
            [network]
            port = 47770
            peers = [
                { kind = "usb4", addr = "10.77.1.2:47770" },
                { kind = "ethernet", addr = "10.77.2.2:47770" },
            ]
            "#,
        )
        .unwrap();
        c.agent = Arc::new(Agent::new(cfg, StateStore::new(dir.path().join("s.json"))));
        let lan: SocketAddr = "192.168.1.20:47771".parse().unwrap();
        let got = candidates(&c, &[lan, "10.77.1.2:47771".parse().unwrap()], 47771);
        assert_eq!(
            got,
            [
                "10.77.1.2:47771".parse().unwrap(),
                "10.77.2.2:47771".parse().unwrap(),
                lan,
            ]
        );
    }

    #[test]
    fn each_machine_waits_next_to_its_real_port() {
        let net = |toml: &str| {
            AgentConfig::from_toml(&format!(
                "machine = \"b\"\nmonitors = []\n[network]\n{toml}"
            ))
            .unwrap()
            .network
        };
        // The agent ended up on 52100 because 52000 was busy: it waits on 52101.
        let fallbacks = [52000, 52100, 53100];
        assert_eq!(
            pairing_ports(&net("port = 52000"), 52100, &fallbacks),
            [52101, 52001, 53101]
        );
        // A fixed `pairing_port` goes first; none clashes with the agent's port.
        assert_eq!(
            pairing_ports(&net("pairing_port = 50000"), 52001, &fallbacks),
            [50000, 52101, 53101]
        );
        assert_eq!(pairing_ports(&net(""), 65535, &[]), [65534]);
    }

    #[tokio::test]
    async fn both_sides_get_the_same_code() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, pc) = (
            ctx(Machine::A, 1, dir.path()),
            ctx(Machine::B, 2, dir.path()),
        );
        let (to_pc, to_mac, _a, _b) = connect(&mac, &pc).await;
        let (m, p) = tokio::join!(initiate(&mac, &to_pc), respond(&pc, &to_mac));
        let ((_, m), (_, p)) = (m.unwrap(), p.unwrap());
        assert_eq!(m.code(), p.code());
        assert_eq!(m.remote().public_key, pc.identity.public_key());
        assert_eq!(m.remote().name, pc.name());
        assert_eq!(p.remote().public_key, mac.identity.public_key());
        assert_eq!(p.remote().name, mac.name());
    }

    /// A middleman that connects with its own key but offers the Mac's: the
    /// PC cuts it before showing any code.
    #[tokio::test]
    async fn an_offer_with_someone_elses_key_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (intruder, pc) = (
            ctx(Machine::A, 3, dir.path()),
            ctx(Machine::B, 2, dir.path()),
        );
        let mac_key = Identity::from_seed([1; 32]).public_key();
        let (to_pc, to_intruder, _a, _b) = connect(&intruder, &pc).await;
        let attack = async {
            let me = intruder.side_on(&to_pc).unwrap();
            let (mut s, _) = to_pc
                .open_control(&Hello::new(Peer::Agent(me)))
                .await
                .unwrap();
            let (_, offer) =
                PairingInitiator::start(mac_key, "other", PairingNonce::generate().unwrap());
            s.send(&PairingMsg::Offer {
                public_key: *offer.public_key.as_bytes(),
                name: offer.name,
                commitment: *offer.commitment.as_bytes(),
            })
            .await
            .unwrap();
            s
        };
        let (_s, refused) = tokio::join!(attack, respond(&pc, &to_intruder));
        assert!(
            matches!(refused, Err(PairError::KeyMismatch)),
            "{:?}",
            refused.err()
        );
        assert_eq!(pc.agent.pairing_status(), PairingStatus::Idle);
    }

    /// With the same key on both ends there are no two sides: it is cut before anything.
    #[tokio::test]
    async fn the_same_key_on_both_ends_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (
            ctx(Machine::A, 1, dir.path()),
            ctx(Machine::A, 1, dir.path()),
        );
        let (to_b, to_a, _x, _y) = connect(&a, &b).await;
        let (i, r) = tokio::join!(initiate(&a, &to_b), respond(&b, &to_a));
        assert!(matches!(i, Err(PairError::SameKey)), "{:?}", i.err());
        assert!(matches!(r, Err(PairError::SameKey)), "{:?}", r.err());
    }

    /// What each side kept in its config does not matter when pairing: the sides come from
    /// the connection's keys, so two agents that thought they were `A` pair all the same.
    #[tokio::test]
    async fn two_agents_that_think_they_are_a_still_pair() {
        let dir = tempfile::tempdir().unwrap();
        let (x, y) = (
            ctx(Machine::A, 5, dir.path()),
            ctx(Machine::A, 6, dir.path()),
        );
        let (to_y, to_x, _a, _b) = connect(&x, &y).await;
        let (i, r) = tokio::join!(initiate(&x, &to_y), respond(&y, &to_x));
        let ((_, i), (_, r)) = (i.unwrap(), r.unwrap());
        assert_eq!(i.code(), r.code());
    }

    /// Both confirmations save trust on both.
    #[tokio::test]
    async fn confirming_on_both_sides_saves_both() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, pc) = (
            ctx(Machine::A, 1, dir.path()),
            ctx(Machine::B, 2, dir.path()),
        );
        let (to_pc, to_mac, _a, _b) = connect(&mac, &pc).await;
        let (m, p) = tokio::join!(initiate(&mac, &to_pc), respond(&pc, &to_mac));
        let ((ms, mp), (ps, pp)) = (m.unwrap(), p.unwrap());
        let (mac_tx, mut mac_rx) = mpsc::unbounded_channel();
        let (pc_tx, mut pc_rx) = mpsc::unbounded_channel();
        mac_tx.send(Answer::Confirm).unwrap();
        pc_tx.send(Answer::Confirm).unwrap();
        let (mo, po) = tokio::join!(
            compare(&mac, ms, mp, &mut mac_rx),
            compare(&pc, ps, pp, &mut pc_rx)
        );
        assert!(
            matches!(&mo, Outcome::Paired(p) if p.name == pc.name()),
            "{mo:?}"
        );
        assert!(
            matches!(&po, Outcome::Paired(p) if p.name == mac.name()),
            "{po:?}"
        );
        assert!(
            mac.trust
                .read()
                .unwrap()
                .is_trusted(&pc.identity.public_key())
        );
        assert!(
            pc.trust
                .read()
                .unwrap()
                .is_trusted(&mac.identity.public_key())
        );
    }

    /// Rejecting on one side, even if the other already confirmed, saves nothing on
    /// either.
    #[tokio::test]
    async fn rejecting_on_one_side_saves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, pc) = (
            ctx(Machine::A, 1, dir.path()),
            ctx(Machine::B, 2, dir.path()),
        );
        let (to_pc, to_mac, _a, _b) = connect(&mac, &pc).await;
        let (m, p) = tokio::join!(initiate(&mac, &to_pc), respond(&pc, &to_mac));
        let ((ms, mp), (ps, pp)) = (m.unwrap(), p.unwrap());
        let (mac_tx, mut mac_rx) = mpsc::unbounded_channel();
        let (pc_tx, mut pc_rx) = mpsc::unbounded_channel();
        mac_tx.send(Answer::Confirm).unwrap();
        pc_tx.send(Answer::Cancel).unwrap();
        let (mo, po) = tokio::join!(
            compare(&mac, ms, mp, &mut mac_rx),
            compare(&pc, ps, pp, &mut pc_rx)
        );
        assert_eq!(po, Outcome::Cancelled);
        assert!(
            matches!(&mo, Outcome::Failed(why) if why.contains("rejected")),
            "{mo:?}"
        );
        assert!(
            !mac.trust
                .read()
                .unwrap()
                .is_trusted(&pc.identity.public_key())
        );
        assert!(
            !pc.trust
                .read()
                .unwrap()
                .is_trusted(&mac.identity.public_key())
        );
    }

    // Real clock: with `start_paused` and QUIC over loopback, tokio jumps to the next timer
    // as soon as there are no ready tasks and the handshake expired before the packet arrived
    // (it happened on WSL outside the sandbox).
    #[tokio::test]
    async fn nobody_confirming_times_out_without_saving() {
        let dir = tempfile::tempdir().unwrap();
        let (mut mac, mut pc) = (
            ctx(Machine::A, 1, dir.path()),
            ctx(Machine::B, 2, dir.path()),
        );
        mac.timeouts.confirm = Duration::from_millis(300);
        pc.timeouts.confirm = Duration::from_secs(60);
        let (to_pc, to_mac, _a, _b) = connect(&mac, &pc).await;
        let (m, p) = tokio::join!(initiate(&mac, &to_pc), respond(&pc, &to_mac));
        let ((ms, mp), (ps, pp)) = (m.unwrap(), p.unwrap());
        let (_mac_tx, mut mac_rx) = mpsc::unbounded_channel();
        let (_pc_tx, mut pc_rx) = mpsc::unbounded_channel();
        let (mo, po) = tokio::join!(
            compare(&mac, ms, mp, &mut mac_rx),
            compare(&pc, ps, pp, &mut pc_rx)
        );
        assert!(
            matches!(&mo, Outcome::Failed(why) if why.contains("time")),
            "{mo:?}"
        );
        assert!(matches!(&po, Outcome::Failed(_)), "{po:?}");
        assert!(
            !pc.trust
                .read()
                .unwrap()
                .is_trusted(&mac.identity.public_key())
        );
    }

    /// On loopback (tests, two agents on the same machine) the own pairing port
    /// is not a candidate: it would be calling itself.
    #[test]
    fn this_machine_never_dials_itself() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ctx(Machine::A, 1, dir.path());
        let cfg = AgentConfig::from_toml(
            "machine = \"a\"\n[network]\nport = 40000\npairing_port = 40001\n\
             peers = [{ kind = \"usb4\", addr = \"127.0.0.1:41000\" }]\n",
        )
        .unwrap();
        c.agent = Arc::new(Agent::new(cfg, StateStore::new(dir.path().join("s.json"))));
        let got = candidates(&c, &["127.0.0.1:40001".parse().unwrap()], 40001);
        assert_eq!(got, ["127.0.0.1:41001".parse::<SocketAddr>().unwrap()]);
    }
}
