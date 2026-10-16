//! La red simulada entre los dos agentes: un enlace por tipo (USB4, Ethernet, Wi-Fi)
//! con flujos en memoria que se pueden cortar.
//!
//! Sobre cada enlace corre el protocolo de control real de `tandemd` (`PeerClient` y
//! `peer::serve`) y el gestor de enlaces real de `tandem-link` elige por cuál van los
//! pasos. Un corte imita un cable desenchufado: nada falla, solo hay silencio, y los
//! flujos abiertos por ese enlace quedan muertos aunque el cable vuelva.

use std::collections::BTreeMap;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use tandem_core::{Machine, State, Step};
use tandem_link::manager::{LinkCandidate, LinkKind, LinkManager, Prober};
use tandem_proto::{CalibrationProbe, ControlMsg};
use tandemd::ipc::Framed;
use tandemd::orchestrator::{LinkHold, PEER_NOT_CONNECTED, PeerLink, ProbeFuture, StepFuture};
use tandemd::{Agent, PeerClient, peer};
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};

use crate::world::{Power, World};

/// Tamaño del búfer de cada flujo en memoria.
const WIRE_BUFFER: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
struct LinkWire {
    addr: SocketAddr,
    up: bool,
    /// Cambia con cada corte: los flujos de una generación anterior quedan muertos.
    generation: u64,
}

#[derive(Debug)]
struct NetInner {
    links: BTreeMap<LinkKind, LinkWire>,
}

/// Los enlaces físicos entre las dos máquinas.
#[derive(Debug, Clone)]
pub struct SimNet {
    inner: Arc<Mutex<NetInner>>,
    world: World,
}

/// Dirección simulada del otro agente por cada enlace (las de DESIGN §6).
pub fn link_addr(kind: LinkKind) -> SocketAddr {
    let net = match kind {
        LinkKind::Usb4 => 1,
        LinkKind::Ethernet => 2,
        LinkKind::Wifi => 3,
    };
    SocketAddr::from(([10, 77, net, 2], tandemd::config::DEFAULT_PORT))
}

impl SimNet {
    /// Los enlaces indicados, todos con el cable puesto.
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

    /// Desenchufa un enlace: los flujos abiertos por él no vuelven a funcionar.
    pub fn cut(&self, kind: LinkKind) {
        if let Some(l) = self.lock().links.get_mut(&kind) {
            l.up = false;
            l.generation += 1;
        }
        tracing::info!(link = %kind, "enlace cortado (simulado)");
    }

    /// Vuelve a enchufar un enlace: sirve para conexiones nuevas.
    pub fn restore(&self, kind: LinkKind) {
        if let Some(l) = self.lock().links.get_mut(&kind) {
            l.up = true;
        }
    }

    /// Una máquina se apagó: todas las conexiones con su agente mueren.
    pub(crate) fn drop_all(&self) {
        for l in self.lock().links.values_mut() {
            l.generation += 1;
        }
    }

    pub fn is_up(&self, kind: LinkKind) -> bool {
        self.lock().links.get(&kind).is_some_and(|l| l.up)
    }

    /// Si una conexión nueva por `kind` llegaría al otro agente, y con qué generación.
    fn reachable(&self, kind: LinkKind) -> Option<u64> {
        let link = *self.lock().links.get(&kind)?;
        let both = [Machine::Mac, Machine::Pc]
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

    /// Un par de flujos unidos por `kind`.
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

/// Un extremo de un flujo por un enlace simulado.
///
/// Con el enlace muerto todo queda en espera para siempre, sin despertar a nadie:
/// así se ve un cable desenchufado desde el software.
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

/// Lo que el gestor de enlaces entrega del enlace activo: por dónde pedir pasos.
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

/// Abre conexiones con el agente del otro lado por la red simulada.
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
            .ok_or_else(|| NetError(format!("{addr} no es un enlace simulado")))?;
        let Some(generation) = self.net.reachable(kind) else {
            // Sin cable o con la otra máquina apagada nadie contesta: lo corta el gestor.
            return std::future::pending().await;
        };
        // Un flujo para los pings del gestor y otro para los pedidos de pasos, como
        // los dos streams de control de QUIC.
        let (ping, ping_far) = self.net.wire_pair(kind, generation);
        let (steps, steps_far) = self.net.wire_pair(kind, generation);
        for far in [ping_far, steps_far] {
            let agent = self.far.clone();
            tokio::spawn(async move {
                if let Err(e) = peer::serve(Framed::new(far), agent).await {
                    tracing::debug!("flujo simulado terminado: {e}");
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
                None => return Err(NetError("el otro lado cerró".into())),
            }
        }
    }

    fn handle(s: &SimSession) -> SimHandle {
        s.handle.clone()
    }
}

/// El otro agente visto por el orquestador: cada pedido va por el enlace activo en
/// ese momento, el que eligió el gestor de enlaces, y falla con `PEER_LOST` si ese
/// enlace cae antes de la respuesta.
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
