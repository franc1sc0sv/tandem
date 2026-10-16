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
//!
//! [`NetworkConfig::link_candidates`]: crate::NetworkConfig::link_candidates

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tandem_core::{Machine, State, Step};
use tandem_link::discovery::MdnsDiscovery;
use tandem_link::identity::{Identity, PublicKey, TrustStore};
use tandem_link::manager::{
    HealthConfig, LinkCandidate, LinkKind, LinkManager, ManagerError, Prober, QuicProber,
    QuicSession,
};
use tandem_link::transport::{Connection, Endpoint, TransportError};
use tandem_proto::{Hello, Link, Peer, PeerStatus};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::agent::Agent;
use crate::orchestrator::{LinkHold, PEER_NOT_CONNECTED, PeerLink, StepFuture};
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
}

impl Default for NetOptions {
    fn default() -> Self {
        NetOptions {
            bind: None,
            mdns: true,
            health: HealthConfig::default(),
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
/// enlaces y el descubrimiento.
pub struct Network {
    endpoint: Arc<Endpoint>,
    local_addr: SocketAddr,
    tasks: Vec<JoinHandle<()>>,
}

impl Network {
    /// Abre el endpoint y, si hay un par emparejado en `trust`, empieza a buscarlo.
    /// Tiene que llamarse dentro de un runtime de tokio.
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
        let paired = trust.peers().next().map(|p| (p.public_key, p.name.clone()));
        let trusted: Vec<PublicKey> = trust.peers().map(|p| p.public_key).collect();
        // Con candado: el emparejamiento desde la UI va a agregar pares en caliente.
        let trust = Arc::new(RwLock::new(trust));
        let endpoint =
            Arc::new(
                Endpoint::bind(addr, identity, trust).map_err(|source| NetError::Bind {
                    addr,
                    source: Box::new(source),
                })?,
            );
        let local_addr = endpoint.local_addr().map_err(|source| NetError::Bind {
            addr,
            source: Box::new(source),
        })?;
        tracing::info!(addr = %local_addr, "endpoint QUIC escuchando");

        let mut tasks = vec![tokio::spawn(accept_loop(endpoint.clone(), agent.clone()))];
        let Some((key, name)) = paired else {
            tracing::info!("sin par emparejado: el agente trabaja solo");
            agent.set_peer_status(PeerStatus::Unpaired);
            return Ok(Network {
                endpoint,
                local_addr,
                tasks,
            });
        };
        tracing::info!(par = %name, llave = %key.to_hex(), "par emparejado");
        agent.set_peer_status(PeerStatus::Paired {
            name: name.clone(),
            link: None,
        });

        let discovery = if opts.mdns {
            match MdnsDiscovery::start(&identity.public_key(), local_addr.port(), trusted) {
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
        tasks.push(tokio::spawn(supervise(
            agent.clone(),
            endpoint.clone(),
            name,
            discovery,
            opts.health,
            reachable_tx,
        )));
        // En la PC no hace nada: que la Mac no conteste no dice nada de la corriente.
        tasks.push(tokio::spawn(crate::pc_watch::watch(agent, reachable)));
        Ok(Network {
            endpoint,
            local_addr,
            tasks,
        })
    }

    /// Dirección real del endpoint (con el puerto elegido si se pidió el 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Cierra las conexiones y espera un poco a que el par se entere, para que no
    /// tenga que esperar a que se le venzan los pings.
    pub async fn shutdown(self) {
        for t in &self.tasks {
            t.abort();
        }
        self.endpoint.close();
        if tokio::time::timeout(CLOSE_TIMEOUT, self.endpoint.wait_idle())
            .await
            .is_err()
        {
            tracing::debug!("el par no confirmó el cierre de las conexiones a tiempo");
        }
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
        self.endpoint.close();
    }
}

/// Atiende cada conexión entrante. El pinning ya rechazó a quien no esté en la confianza.
async fn accept_loop(endpoint: Arc<Endpoint>, agent: Arc<Agent>) {
    while let Some(conn) = endpoint.accept().await {
        match conn {
            Ok(conn) => {
                tracing::debug!(from = %conn.remote_address(), "conexión del otro agente");
                tokio::spawn(peer::accept_streams(conn, agent.clone()));
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
            let active = links
                .as_ref()
                .and_then(|l| l.borrow().active.as_ref().map(|a| a.kind));
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
