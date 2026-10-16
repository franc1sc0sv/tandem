//! Emparejamiento desde la UI (DESIGN §9): encontrar al otro agente, mostrar el mismo
//! código de 6 dígitos en las dos pantallas y, si el usuario lo confirma en las dos,
//! guardar la llave del otro en la confianza.
//!
//! - Va por un endpoint aparte ([`Endpoint::bind_pairing`]), que acepta llaves
//!   desconocidas y solo existe mientras se empareja. El endpoint normal sigue sin
//!   aceptar a nadie que no esté en la confianza.
//! - Los papeles son fijos para que las dos máquinas no se busquen a la vez: la PC
//!   espera en el puerto de emparejamiento (y lo anuncia por mDNS) y la Mac la busca
//!   por los enlaces fijos de la config y por mDNS. El usuario abre el emparejamiento
//!   en las dos, en cualquier orden.
//! - El protocolo es el de compromiso de [`tandem_link::identity`]; las llaves que se
//!   intercambian tienen que ser las mismas que autenticó la conexión.
//! - Cada lado manda su [`PairingMsg::Confirm`] cuando su usuario confirma y guarda la
//!   confianza cuando tiene las dos confirmaciones. Rechazar en cualquiera de los dos
//!   lados (o que venza el tiempo) termina sin guardar nada en ninguno.

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

/// Lo que se espera en cada fase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairingTimeouts {
    /// Buscar a la otra máquina (que también tiene que estar emparejando).
    pub search: Duration,
    /// Que los dos usuarios confirmen el código.
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

/// Los tres mensajes del protocolo, sin contar la espera del usuario.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Cada intento de conexión de la Mac; si la PC todavía no espera, se reintenta.
const CONNECT_ATTEMPT: Duration = Duration::from_secs(2);
const RETRY_EVERY: Duration = Duration::from_millis(300);
/// Lo que se espera a que el otro lado reciba el último mensaje antes de cortar.
const CLOSE_WAIT: Duration = Duration::from_secs(2);
/// El nombre del otro se muestra en la UI: se recorta uno absurdo.
const MAX_NAME_CHARS: usize = 64;

/// Lo que contesta el usuario de esta máquina.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Confirm,
    Cancel,
}

/// Cómo terminó un emparejamiento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Guardado en la confianza.
    Paired(TrustedPeer),
    /// Lo canceló o rechazó el usuario de esta máquina.
    Cancelled,
    /// No se emparejó; el motivo es para la UI.
    Failed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error("{0}")]
    Transport(#[from] TransportError),
    #[error("{0}")]
    Identity(#[from] IdentityError),
    #[error("se esperaba el agente de la {expected}, contestó {got:?}")]
    UnexpectedPeer { expected: Machine, got: Peer },
    #[error("la llave del emparejamiento no es la de la conexión")]
    KeyMismatch,
    #[error("mensaje de emparejamiento inesperado")]
    Unexpected,
    #[error("la otra máquina cortó el emparejamiento")]
    Closed,
    #[error("la otra máquina dejó de responder")]
    Timeout,
}

/// Lo que necesita un emparejamiento.
pub struct Ctx {
    pub agent: Arc<Agent>,
    pub identity: Identity,
    pub trust: Arc<RwLock<TrustStore>>,
    /// La IP del endpoint normal: el de emparejamiento usa la misma familia.
    pub bind_ip: IpAddr,
    /// El puerto real del endpoint normal (puede no ser el de la config, si estaba ocupado).
    pub port: u16,
    /// Los de reserva de la red ([`crate::network::NetOptions::fallback_ports`]).
    pub fallback_ports: Vec<u16>,
    pub mdns: bool,
    pub timeouts: PairingTimeouts,
}

/// Dónde puede esperar la PC al emparejar, en orden: el `pairing_port` de la config si se
/// fijó, si no el siguiente al puerto real del agente; después el siguiente a cada puerto
/// de reserva. La Mac lo encuentra por el anuncio de mDNS, que lleva el puerto real.
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
    fn me(&self) -> Machine {
        self.agent.machine()
    }

    fn hello(&self) -> Hello {
        Hello::new(Peer::Agent(self.me()))
    }

    fn publish(&self, status: PairingStatus) {
        self.agent.set_pairing_status(status);
    }
}

/// El nombre con que esta máquina se presenta al otro (y con el que él la guarda).
pub fn display_name(m: Machine) -> &'static str {
    match m {
        Machine::Mac => "Mac",
        Machine::Pc => "PC",
    }
}

/// Un emparejamiento completo: buscar, comparar, guardar. Publica cada paso en el
/// agente; el resultado final lo publica quien llama.
pub async fn run(ctx: &Ctx, answers: &mut mpsc::UnboundedReceiver<Answer>) -> Outcome {
    ctx.publish(PairingStatus::Searching);
    let found = tokio::select! {
        found = tokio::time::timeout(ctx.timeouts.search, search(ctx)) => found,
        cancelled = cancelled(answers) => {
            // Sin conexión no hay a quién avisar.
            let () = cancelled;
            return Outcome::Cancelled;
        }
    };
    let (conn, stream, pairing) = match found {
        Ok(Ok(found)) => found,
        Ok(Err(e)) => return Outcome::Failed(e),
        Err(_) => {
            return Outcome::Failed(format!(
                "no apareció la {} (¿está abierto el emparejamiento allá?)",
                display_name(ctx.me().other())
            ));
        }
    };
    let outcome = compare(ctx, stream, pairing, answers).await;
    drop(conn);
    outcome
}

/// Espera a que el usuario cancele; confirmar mientras se busca no significa nada.
async fn cancelled(answers: &mut mpsc::UnboundedReceiver<Answer>) {
    loop {
        match answers.recv().await {
            Some(Answer::Confirm) => continue,
            Some(Answer::Cancel) | None => return,
        }
    }
}

type Found = (Connection, ControlStream, Pairing);

async fn search(ctx: &Ctx) -> Result<Found, String> {
    match ctx.me() {
        Machine::Pc => wait_for_mac(ctx).await,
        Machine::Mac => look_for_pc(ctx).await,
    }
}

/// La PC: espera en el puerto de emparejamiento a que la Mac conecte.
async fn wait_for_mac(ctx: &Ctx) -> Result<Found, String> {
    let ports = pairing_ports(&ctx.agent.config().network, ctx.port, &ctx.fallback_ports);
    let (endpoint, addr) = crate::network::bind_first(ctx.bind_ip, &ports, |addr| {
        Endpoint::bind_pairing(addr, &ctx.identity)
    })
    .map_err(|e| format!("no se pudo esperar a la Mac: {e}"))?;
    let port = endpoint
        .local_addr()
        .map(|a| a.port())
        .unwrap_or(addr.port());
    tracing::info!(%addr, "esperando a la Mac para emparejar");
    let _announce = ctx.mdns.then(|| {
        MdnsDiscovery::pairing(&ctx.identity.public_key(), Some(port))
            .map_err(|e| tracing::warn!("no se pudo anunciar el emparejamiento por mDNS: {e}"))
            .ok()
    });
    loop {
        let conn = match endpoint.accept().await {
            Some(Ok(conn)) => conn,
            Some(Err(e)) => {
                tracing::debug!("conexión de emparejamiento fallida: {e}");
                continue;
            }
            None => return Err("se cerró el puerto de emparejamiento".into()),
        };
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, respond(ctx, &conn)).await {
            Ok(Ok((stream, pairing))) => return Ok((conn, stream, pairing)),
            Ok(Err(e)) => {
                tracing::warn!(from = %conn.remote_address(), "emparejamiento rechazado: {e}")
            }
            Err(_) => tracing::warn!(from = %conn.remote_address(), "emparejamiento sin respuesta"),
        }
    }
}

/// La Mac: prueba las direcciones de la PC hasta que una conteste.
async fn look_for_pc(ctx: &Ctx) -> Result<Found, String> {
    let endpoint = Endpoint::bind_pairing(SocketAddr::new(ctx.bind_ip, 0), &ctx.identity)
        .map_err(|e| format!("no se pudo abrir el emparejamiento: {e}"))?;
    let discovery = if ctx.mdns {
        MdnsDiscovery::pairing(&ctx.identity.public_key(), None)
            .map_err(|e| tracing::warn!("sin mDNS, solo se busca por los enlaces fijos: {e}"))
            .ok()
    } else {
        None
    };
    let lan = discovery.as_ref().map(MdnsDiscovery::peer_addrs);
    loop {
        let discovered = lan.as_ref().map(|l| l.borrow().clone()).unwrap_or_default();
        for addr in candidates(ctx, &discovered) {
            let conn = match tokio::time::timeout(CONNECT_ATTEMPT, endpoint.connect(addr)).await {
                Ok(Ok(conn)) => conn,
                Ok(Err(e)) => {
                    tracing::debug!(%addr, "la PC no contesta al emparejamiento: {e}");
                    continue;
                }
                Err(_) => continue,
            };
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, initiate(ctx, &conn)).await {
                Ok(Ok((stream, pairing))) => return Ok((conn, stream, pairing)),
                Ok(Err(e)) => tracing::warn!(%addr, "emparejamiento rechazado: {e}"),
                Err(_) => tracing::warn!(%addr, "emparejamiento sin respuesta"),
            }
        }
        tokio::time::sleep(RETRY_EVERY).await;
    }
}

/// Dónde buscar a la PC: la IP de cada enlace fijo con el puerto de emparejamiento, y
/// lo que anuncie por mDNS, sin repetir.
fn candidates(ctx: &Ctx, discovered: &[SocketAddr]) -> Vec<SocketAddr> {
    let net = &ctx.agent.config().network;
    let port = net.pairing_port();
    let mut seen = BTreeSet::new();
    net.peers
        .iter()
        .map(|l| SocketAddr::new(l.addr.ip(), port))
        .chain(discovered.iter().copied())
        .filter(|a| seen.insert(*a))
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

/// La llave que el otro dice tener tiene que ser la que firmó el handshake: así el
/// código que comparan los usuarios también autentica esta conexión.
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
        "otra máquina".into()
    } else {
        name.to_owned()
    }
}

async fn next(stream: &mut ControlStream) -> Result<PairingMsg, PairError> {
    stream.recv().await?.ok_or(PairError::Closed)
}

/// Quien inicia (la Mac): oferta con compromiso, respuesta, revelación.
pub async fn initiate(ctx: &Ctx, conn: &Connection) -> Result<(ControlStream, Pairing), PairError> {
    let (mut stream, theirs) = conn.open_control(&ctx.hello()).await?;
    check_peer(ctx.me(), theirs.peer)?;
    let (initiator, offer) = PairingInitiator::start(
        ctx.identity.public_key(),
        display_name(ctx.me()),
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

/// Quien responde (la PC).
pub async fn respond(ctx: &Ctx, conn: &Connection) -> Result<(ControlStream, Pairing), PairError> {
    let (mut stream, theirs) = conn.accept_control(&ctx.hello()).await?;
    check_peer(ctx.me(), theirs.peer)?;
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
        display_name(ctx.me()),
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

/// Los dos usuarios comparan el código. Termina al tener las dos confirmaciones (y
/// entonces guarda la confianza), o al primer rechazo, corte o vencimiento.
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
    tracing::info!(par = %peer, "comparando el código de emparejamiento");
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
                            "la otra máquina confirmó otro código: no se emparejó".into(),
                        );
                    }
                }
                Ok(Some(PairingMsg::Reject)) => {
                    return Outcome::Failed(format!(
                        "se rechazó en la {}: no se emparejó",
                        display_name(ctx.me().other())
                    ));
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
                return Outcome::Failed("se acabó el tiempo para confirmar el código".into());
            }
        }
    }
    let saved = {
        let mut trust = ctx.trust.write().unwrap_or_else(|e| e.into_inner());
        pairing.complete(&mut trust)
    };
    // Que el otro lado reciba la confirmación antes de cortar.
    let _ = tx.finish();
    drain(&mut rx).await;
    match saved {
        Ok(peer) => {
            tracing::info!(par = %peer.name, llave = %peer.public_key.to_hex(), "emparejado");
            Outcome::Paired(peer)
        }
        Err(e) => Outcome::Failed(format!("no se pudo guardar la confianza: {e}")),
    }
}

fn lost(e: &PairError) -> Outcome {
    tracing::warn!("emparejamiento cortado: {e}");
    Outcome::Failed("se perdió la conexión con la otra máquina: no se emparejó".into())
}

/// Avisa al otro lado y le da un momento para leerlo.
async fn reject(tx: &mut ControlSender, rx: &mut ControlReceiver) {
    if tx.send(&PairingMsg::Reject).await.is_ok() {
        let _ = tx.finish();
        drain(rx).await;
    }
}

/// Lee hasta que el otro lado cierra (o corta, o pasa [`CLOSE_WAIT`]).
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
        assert_eq!(clean_name("\u{7}\t".into()), "otra máquina");
        assert_eq!(clean_name("x".repeat(500)).chars().count(), MAX_NAME_CHARS);
        assert_eq!(display_name(Machine::Mac), "Mac");
        assert_eq!(display_name(Machine::Pc), "PC");
    }

    #[test]
    fn the_mac_looks_for_the_pc_on_every_static_link_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ctx(Machine::Mac, 1, dir.path());
        let cfg = AgentConfig::from_toml(
            r#"
            machine = "mac"
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
        let got = candidates(&c, &[lan, "10.77.1.2:47771".parse().unwrap()]);
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
    fn the_pc_waits_next_to_its_real_port() {
        let net = |toml: &str| {
            AgentConfig::from_toml(&format!(
                "machine = \"pc\"\nmonitors = []\n[network]\n{toml}"
            ))
            .unwrap()
            .network
        };
        // El agente quedó en 52100 porque el 52000 estaba ocupado: espera en 52101.
        let fallbacks = [52000, 52100, 53100];
        assert_eq!(
            pairing_ports(&net("port = 52000"), 52100, &fallbacks),
            [52101, 52001, 53101]
        );
        // Un `pairing_port` fijo va primero; ninguno choca con el puerto del agente.
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
            ctx(Machine::Mac, 1, dir.path()),
            ctx(Machine::Pc, 2, dir.path()),
        );
        let (to_pc, to_mac, _a, _b) = connect(&mac, &pc).await;
        let (m, p) = tokio::join!(initiate(&mac, &to_pc), respond(&pc, &to_mac));
        let ((_, m), (_, p)) = (m.unwrap(), p.unwrap());
        assert_eq!(m.code(), p.code());
        assert_eq!(m.remote().public_key, pc.identity.public_key());
        assert_eq!(m.remote().name, "PC");
        assert_eq!(p.remote().public_key, mac.identity.public_key());
        assert_eq!(p.remote().name, "Mac");
    }

    /// Un intermediario que conecta con su propia llave pero ofrece la de la Mac: la
    /// PC lo corta antes de mostrar ningún código.
    #[tokio::test]
    async fn an_offer_with_someone_elses_key_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (intruder, pc) = (
            ctx(Machine::Mac, 3, dir.path()),
            ctx(Machine::Pc, 2, dir.path()),
        );
        let mac_key = Identity::from_seed([1; 32]).public_key();
        let (to_pc, to_intruder, _a, _b) = connect(&intruder, &pc).await;
        let attack = async {
            let (mut s, _) = to_pc.open_control(&intruder.hello()).await.unwrap();
            let (_, offer) =
                PairingInitiator::start(mac_key, "Mac", PairingNonce::generate().unwrap());
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

    #[tokio::test]
    async fn the_same_machine_on_both_ends_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (
            ctx(Machine::Pc, 1, dir.path()),
            ctx(Machine::Pc, 2, dir.path()),
        );
        let (to_b, to_a, _x, _y) = connect(&a, &b).await;
        let (i, r) = tokio::join!(initiate(&a, &to_b), respond(&b, &to_a));
        assert!(
            matches!(i, Err(PairError::UnexpectedPeer { .. })),
            "{:?}",
            i.err()
        );
        assert!(
            matches!(r, Err(PairError::UnexpectedPeer { .. })),
            "{:?}",
            r.err()
        );
    }

    /// Las dos confirmaciones guardan la confianza en los dos.
    #[tokio::test]
    async fn confirming_on_both_sides_saves_both() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, pc) = (
            ctx(Machine::Mac, 1, dir.path()),
            ctx(Machine::Pc, 2, dir.path()),
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
            matches!(&mo, Outcome::Paired(p) if p.name == "PC"),
            "{mo:?}"
        );
        assert!(
            matches!(&po, Outcome::Paired(p) if p.name == "Mac"),
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

    /// Rechazar en un lado, aunque el otro ya haya confirmado, no guarda nada en
    /// ninguno.
    #[tokio::test]
    async fn rejecting_on_one_side_saves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, pc) = (
            ctx(Machine::Mac, 1, dir.path()),
            ctx(Machine::Pc, 2, dir.path()),
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
            matches!(&mo, Outcome::Failed(why) if why.contains("rechazó")),
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

    #[tokio::test(start_paused = true)]
    async fn nobody_confirming_times_out_without_saving() {
        let dir = tempfile::tempdir().unwrap();
        let (mut mac, mut pc) = (
            ctx(Machine::Mac, 1, dir.path()),
            ctx(Machine::Pc, 2, dir.path()),
        );
        mac.timeouts.confirm = Duration::from_secs(5);
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
            matches!(&mo, Outcome::Failed(why) if why.contains("tiempo")),
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
}
