//! La red del agente (DESIGN §6 y §9): endpoint QUIC con pinning, descubrimiento del
//! par, gestor de enlaces y el [`PeerLink`] del orquestador sobre el enlace activo.
//!
//! Es lo mismo que arma `tandem-sim` con su red en memoria, sobre el transporte real:
//!
//! - El endpoint atiende cada conexión del par con [`peer::accept_streams`]: los pings
//!   de su gestor de enlaces y sus pedidos de pasos.
//! - Con un par emparejado, el gestor de enlaces prueba los enlaces fijos de la config
//!   y la dirección que el par anuncie en la LAN por mDNS ([`NetworkConfig::link_candidates`]).
//!   Por cada enlace abre una conexión con un stream de pings y otro de pedidos
//!   ([`AgentProber`]).
//! - Mientras algún enlace ve al otro agente, el orquestador tiene un [`ManagedPeer`]
//!   que manda cada pedido por el enlace activo; si ninguno lo ve, vuelve a no tener
//!   par, y encender la PC espera a que aparezca. Lo mismo alimenta a
//!   [`pc_watch`](crate::pc_watch) en la Mac y lo que la UI ve como [`PeerStatus`].
//! - Sin par emparejado no hay gestor ni descubrimiento: el agente trabaja solo y la
//!   UI recibe [`PeerStatus::Unpaired`].
//! - La UI empareja y desempareja en caliente ([`PairingControl`]): al emparejar se
//!   empieza a seguir al par nuevo; al desemparejar se lo borra de la confianza, se
//!   deja de seguirlo y se cortan sus conexiones.
//!
//! [`NetworkConfig::link_candidates`]: crate::NetworkConfig::link_candidates

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, Weak};
use std::time::Duration;

use tandem_core::{Machine, State, Step};
use tandem_link::discovery::MdnsDiscovery;
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_link::manager::{
    HealthConfig, LinkCandidate, LinkKind, LinkManager, ManagerError, Prober, QuicProber,
    QuicSession,
};
use tandem_link::transport::{Connection, Endpoint, TransportError};
use tandem_proto::{CalibrationProbe, Hello, Link, PairingStatus, Peer, PeerStatus};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::agent::{Agent, NO_NETWORK, PairingControl};
use crate::orchestrator::{LinkHold, PEER_NOT_CONNECTED, PeerLink, ProbeFuture, StepFuture};
use crate::pairing::{self, PairingTimeouts};
use crate::peer::{self, PeerClient, PeerError};

/// Lo que se espera a que el par se entere del cierre de las conexiones al apagar.
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// Cómo arrancar la red. Lo normal es [`NetOptions::default`]; las pruebas eligen la
/// dirección, apagan mDNS y aceleran la revisión de salud.
#[derive(Debug, Clone)]
pub struct NetOptions {
    /// Dirección UDP donde escucha el endpoint. Por omisión, todas las interfaces en el
    /// puerto de la config ([`default_bind`]).
    pub bind: Option<SocketAddr>,
    /// Anunciar y buscar al par en la LAN por mDNS.
    pub mdns: bool,
    pub health: HealthConfig,
    pub pairing: PairingTimeouts,
}

impl Default for NetOptions {
    fn default() -> Self {
        NetOptions {
            bind: None,
            mdns: true,
            health: HealthConfig::default(),
            pairing: PairingTimeouts::default(),
        }
    }
}

/// Los errores del transporte son grandes: van en caja para no agrandar cada `Result`.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("no se pudo abrir el endpoint QUIC en {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: Box<TransportError>,
    },
}

/// Todas las interfaces en `port`. IPv6 solo si algún enlace fijo es IPv6: un socket
/// IPv6 no recibe IPv4 en todos los sistemas (en Windows es solo IPv6 por omisión), y
/// los enlaces directos de DESIGN §6 son IPv4.
pub fn default_bind(port: u16, statics: &[LinkCandidate]) -> SocketAddr {
    let ip = if statics.iter().any(|l| l.addr.is_ipv6()) {
        IpAddr::V6(Ipv6Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    };
    SocketAddr::new(ip, port)
}

/// El enlace del gestor, como lo ve la UI.
pub fn link_of(kind: LinkKind) -> Link {
    match kind {
        LinkKind::Usb4 => Link::Usb4,
        LinkKind::Ethernet => Link::Ethernet,
        LinkKind::Wifi => Link::Wifi,
    }
}

/// La red en marcha. Al soltarla se cierran las conexiones y se detienen el gestor de
/// enlaces, el descubrimiento y el emparejamiento en curso.
pub struct Network {
    shared: Arc<Shared>,
}

/// Lo que comparten la red, el emparejamiento y quien atiende sus pedidos.
struct Shared {
    agent: Arc<Agent>,
    identity: Identity,
    endpoint: Arc<Endpoint>,
    local_addr: SocketAddr,
    /// Con candado: el emparejamiento desde la UI agrega y quita pares en caliente, y
    /// el pinning del endpoint lo ve en las conexiones nuevas.
    trust: Arc<RwLock<TrustStore>>,
    opts: NetOptions,
    conns: Arc<Conns>,
    accept: JoinHandle<()>,
    /// El par que se sigue (gestor de enlaces, mDNS y `pc_watch`), si hay.
    peer: Mutex<Option<Following>>,
    pairing: Mutex<Option<PairingRun>>,
    /// El enlace activo con el par que se sigue, para el motor de entrada. Vive aquí y
    /// no en cada `supervise`: el par puede emparejarse o desemparejarse en caliente, y
    /// quien lo escucha desde el arranque tiene que ver cada enlace nuevo.
    active: LinkFeed,
}

/// El enlace activo y quién puede escribirlo: solo el `supervise` del par que se
/// sigue ahora. Uno que ya se abortó puede estar a mitad de un poll; sin la
/// generación podría reponer el enlace de un par olvidado.
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

    /// Invalida al escritor anterior y deja el enlace en `None`. Devuelve la
    /// generación del escritor nuevo.
    fn reset(&self) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.tx.send_replace(None);
        generation
    }

    /// Publica `link` si `generation` sigue siendo la vigente y cambió algo. La
    /// comparación va dentro del candado del canal: `reset` escribe con ese mismo
    /// candado, así que lo que se publique antes lo pisa el `None`.
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

/// Lo que `supervise` usa para publicar el enlace activo.
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

/// Las conexiones que abrió el otro agente, para cortarlas al desemparejar: el
/// pinning solo mira la confianza en el handshake.
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
            conn.close("desemparejado");
        }
    }
}

/// Si otro hilo entró en pánico con el candado, lo de adentro sigue sirviendo.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Lo que la UI recibe al pedir emparejar con un par ya emparejado.
pub fn already_paired(name: &str) -> String {
    format!("ya está emparejado con {name}: desemparéjalo primero")
}
pub const PAIRING_IN_PROGRESS: &str = "ya hay un emparejamiento en curso";
pub const NOTHING_TO_CONFIRM: &str = "no hay ningún código que confirmar";
pub const NOT_PAIRED: &str = "no hay ningún par emparejado";

/// El enlace por el que va todo en este momento: el motor de entrada abre ahí sus
/// streams y manda sus avisos.
#[derive(Clone)]
pub struct ActiveLink {
    pub kind: LinkKind,
    pub handle: PeerHandle,
}

impl ActiveLink {
    /// La misma sesión: mismo enlace y misma conexión.
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
    /// Abre el endpoint y, si hay un par emparejado en `trust`, empieza a buscarlo.
    /// Atiende los pedidos de emparejamiento del agente. Tiene que llamarse dentro de
    /// un runtime de tokio.
    pub fn start(
        agent: Arc<Agent>,
        identity: &Identity,
        trust: TrustStore,
        opts: NetOptions,
    ) -> Result<Self, NetError> {
        let net_cfg = &agent.config().network;
        let addr = opts
            .bind
            .unwrap_or_else(|| default_bind(net_cfg.port, &net_cfg.peers));
        // El primer par de la confianza: por ahora un agente se empareja con uno solo.
        let paired = trust.peers().next().cloned();
        let trust = Arc::new(RwLock::new(trust));
        let endpoint = Arc::new(Endpoint::bind(addr, identity, trust.clone()).map_err(
            |source| NetError::Bind {
                addr,
                source: Box::new(source),
            },
        )?);
        let local_addr = endpoint.local_addr().map_err(|source| NetError::Bind {
            addr,
            source: Box::new(source),
        })?;
        tracing::info!(addr = %local_addr, "endpoint QUIC escuchando");

        let conns = Arc::new(Conns::default());
        let accept = tokio::spawn(accept_loop(endpoint.clone(), agent.clone(), conns.clone()));
        let shared = Arc::new(Shared {
            agent: agent.clone(),
            identity: identity.clone(),
            endpoint,
            local_addr,
            trust,
            opts,
            conns,
            accept,
            peer: Mutex::new(None),
            pairing: Mutex::new(None),
            active: LinkFeed::new(),
        });
        agent.set_pairing(Arc::new(Pairer(Arc::downgrade(&shared))));
        match paired {
            Some(peer) => shared.follow(peer),
            None => {
                tracing::info!("sin par emparejado: el agente trabaja solo");
                agent.set_peer_status(PeerStatus::Unpaired);
            }
        }
        Ok(Network { shared })
    }

    /// El enlace activo con el otro agente; `None` si ninguno lo ve (o no hay par).
    pub fn active_link(&self) -> watch::Receiver<Option<ActiveLink>> {
        self.shared.active.tx.subscribe()
    }

    /// Dirección real del endpoint (con el puerto elegido si se pidió el 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.shared.local_addr
    }

    /// Cierra las conexiones y espera un poco a que el par se entere, para que no
    /// tenga que esperar a que se le venzan los pings.
    pub async fn shutdown(self) {
        self.shared.stop();
        if tokio::time::timeout(CLOSE_TIMEOUT, self.shared.endpoint.wait_idle())
            .await
            .is_err()
        {
            tracing::debug!("el par no confirmó el cierre de las conexiones a tiempo");
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

    /// Empieza a seguir a `peer`: mDNS, gestor de enlaces y, en la Mac, `pc_watch`.
    fn follow(self: &Arc<Self>, peer: TrustedPeer) {
        let TrustedPeer { public_key, name } = peer;
        tracing::info!(par = %name, llave = %public_key.to_hex(), "par emparejado");
        let agent = &self.agent;
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
                // Sin multicast quedan los enlaces fijos: no es motivo para no arrancar.
                Err(e) => {
                    tracing::warn!("mDNS no disponible, solo se usan los enlaces fijos: {e}");
                    None
                }
            }
        } else {
            None
        };
        let (reachable_tx, reachable) = watch::channel(false);
        let mut following = lock(&self.peer);
        // Primero se calla al anterior: su enlace no es el del par nuevo.
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
            )),
            // En la PC no hace nada: que la Mac no conteste no dice nada de la corriente.
            tokio::spawn(crate::pc_watch::watch(agent.clone(), reachable)),
        ];
        *following = Some(Following {
            key: public_key,
            tasks,
        });
    }

    /// Olvida al par: lo borra de la confianza, deja de buscarlo y corta sus
    /// conexiones. Si no se puede guardar la confianza, no cambia nada.
    fn forget(&self) -> Result<(), String> {
        let mut following = lock(&self.peer);
        let Some(key) = following.as_ref().map(|f| f.key) else {
            return Err(NOT_PAIRED.into());
        };
        {
            let mut trust = self.trust.write().unwrap_or_else(|e| e.into_inner());
            trust
                .remove_and_save(&key)
                .map_err(|e| format!("no se pudo guardar la confianza: {e}"))?;
        }
        if let Some(f) = following.take() {
            for t in f.tasks {
                t.abort();
            }
        }
        // Sin par no hay enlace: el motor de entrada cierra su sesión.
        self.active.reset();
        drop(following);
        // `supervise` no llega a soltar el par al abortarse.
        self.agent.orchestrator().clear_peer();
        self.conns.close_from(&key);
        tracing::info!(llave = %key.to_hex(), "desemparejado");
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
            mdns: self.opts.mdns,
            timeouts: self.opts.pairing,
        };
        // Antes de devolver: la UI que pidió empezar ya ve que se está buscando.
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
                    tracing::warn!("no se emparejó: {reason}");
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

/// Lo que el agente usa para emparejar. Débil: la red es de quien la arrancó, y sin
/// ella los pedidos se rechazan.
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
            // Nada en curso: la UI cierra un resultado ya mostrado.
            shared.agent.set_pairing_status(PairingStatus::Idle);
        }
        Ok(())
    }

    fn unpair(&self) -> Result<(), String> {
        self.shared()?.forget()
    }
}

/// Atiende cada conexión entrante. El pinning ya rechazó a quien no esté en la confianza.
async fn accept_loop(endpoint: Arc<Endpoint>, agent: Arc<Agent>, conns: Arc<Conns>) {
    while let Some(conn) = endpoint.accept().await {
        match conn {
            Ok(conn) => {
                tracing::debug!(from = %conn.remote_address(), "conexión del otro agente");
                let (agent, conns) = (agent.clone(), conns.clone());
                let id = conns.add(conn.clone());
                tokio::spawn(async move {
                    peer::accept_streams(conn, agent).await;
                    conns.remove(id);
                });
            }
            Err(e) => tracing::debug!("conexión entrante rechazada: {e}"),
        }
    }
}

/// Arma el gestor de enlaces con los candidatos de cada momento y sigue su enlace
/// activo. Si lo descubierto en la LAN cambia los candidatos, se arma de nuevo: el
/// gestor no admite enlaces nuevos en marcha, y las conexiones de los enlaces fijos se
/// vuelven a abrir en un ping.
async fn supervise(
    agent: Arc<Agent>,
    endpoint: Arc<Endpoint>,
    name: String,
    discovery: Option<MdnsDiscovery>,
    health: HealthConfig,
    reachable: watch::Sender<bool>,
    active_link: LinkWriter,
) {
    let mut discovered = discovery.as_ref().map(MdnsDiscovery::peer_addrs);
    let lan = |d: &Option<watch::Receiver<Vec<SocketAddr>>>| {
        d.as_ref().map(|d| d.borrow().clone()).unwrap_or_default()
    };
    let mut candidates = agent.config().network.link_candidates(&lan(&discovered));
    loop {
        let managed = match spawn_manager(&agent, &endpoint, &candidates, health) {
            Ok(m) => Some(m),
            Err(ManagerError::NoCandidates) => {
                tracing::info!("sin enlaces con el par: se espera a descubrirlo en la LAN");
                None
            }
            Err(e) => {
                tracing::error!("no se pudo arrancar el gestor de enlaces: {e}");
                None
            }
        };
        let mut links = managed.as_ref().map(|m| m.links.watch());
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
                        // El gestor no termina solo; si pasara, no queda nada que seguir.
                        links = None;
                    }
                }
                changed = changed(&mut discovered) => {
                    if changed.is_err() {
                        discovered = None;
                        continue;
                    }
                    let next = agent.config().network.link_candidates(&lan(&discovered));
                    if next != candidates {
                        tracing::info!(?next, "cambiaron los enlaces con el par");
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
    }
}

/// Espera el próximo cambio; sin receptor, nunca termina.
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
) -> Result<Arc<ManagedPeer>, ManagerError> {
    let prober = AgentProber::new(endpoint.clone(), agent.machine());
    // Los cambios de enlace ya los registra el gestor; la foto basta para seguirlos.
    let (links, _events) = LinkManager::spawn(candidates, prober, health)?;
    tracing::info!(?candidates, "gestor de enlaces en marcha");
    Ok(Arc::new(ManagedPeer { links }))
}

/// Lo que el gestor de enlaces entrega del enlace activo: la conexión y el stream de
/// pedidos abierto sobre ella.
#[derive(Clone)]
pub struct PeerHandle {
    pub conn: Connection,
    pub client: Arc<PeerClient>,
}

pub struct AgentSession {
    quic: QuicSession,
    handle: PeerHandle,
}

/// [`QuicProber`] más el stream de pedidos: cada enlace sano ya tiene por dónde
/// pedirle pasos al otro agente. El otro lado atiende los dos streams con
/// [`peer::accept_streams`].
pub struct AgentProber {
    quic: QuicProber,
    me: Machine,
}

impl AgentProber {
    pub fn new(endpoint: Arc<Endpoint>, me: Machine) -> Self {
        AgentProber {
            quic: QuicProber::new(endpoint, Hello::new(Peer::Agent(me))),
            me,
        }
    }
}

impl Prober for AgentProber {
    type Session = AgentSession;
    type Handle = PeerHandle;
    type Error = PeerError;

    async fn connect(&self, addr: SocketAddr) -> Result<AgentSession, PeerError> {
        let quic = self.quic.connect(addr).await?;
        let conn = QuicProber::handle(&quic);
        let client = Arc::new(PeerClient::open(&conn, self.me).await?);
        Ok(AgentSession {
            quic,
            handle: PeerHandle { conn, client },
        })
    }

    async fn ping(&self, s: &mut AgentSession) -> Result<(), PeerError> {
        Ok(self.quic.ping(&mut s.quic).await?)
    }

    fn handle(s: &AgentSession) -> PeerHandle {
        s.handle.clone()
    }
}

/// El otro agente visto por el orquestador: cada pedido va por el enlace activo en
/// ese momento y falla con `PEER_LOST` si ese enlace cae antes de la respuesta.
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

    fn hold_link(&self) -> Option<LinkHold> {
        Some(Box::new(self.links.hold()))
    }

    fn show_probe<'a>(&'a self, probe: &'a CalibrationProbe) -> ProbeFuture<'a> {
        let active = self.links.active();
        let links = self.links.watch();
        Box::pin(async move {
            match active {
                Some(a) => peer::over_link(links, a.kind, a.handle.client.show_probe(probe)).await,
                None => Err(PEER_NOT_CONNECTED.to_owned()),
            }
        })
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
    fn every_link_kind_has_its_ui_name() {
        assert_eq!(
            LinkKind::ALL.map(link_of),
            [Link::Usb4, Link::Ethernet, Link::Wifi]
        );
    }
}
