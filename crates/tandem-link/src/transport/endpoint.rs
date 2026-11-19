use std::net::{Ipv4Addr, SocketAddr, SocketAddrV6};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use socket2::{Domain, Protocol, Socket, Type};
use tandem_proto::Hello;

use super::control::ControlStream;
use super::verify::{
    AnyKey, PeerTrust, PinnedClientVerifier, PinnedServerVerifier, peer_public_key,
};
use super::{ALPN, PAIRING_ALPN, TransportError};
use crate::identity::{Identity, PublicKey, TLS_SERVER_NAME};
use crate::qos::Qos;

/// QUIC endpoint of an agent: it listens and also opens connections, with the
/// same certificate and the same trust in both roles.
pub struct Endpoint {
    inner: quinn::Endpoint,
}

impl Endpoint {
    /// Opens the UDP socket on `addr` (port 0 = any free one).
    ///
    /// It has to be called inside a tokio runtime.
    pub fn bind(
        addr: SocketAddr,
        identity: &Identity,
        trust: Arc<dyn PeerTrust>,
    ) -> Result<Self, TransportError> {
        Self::bind_with(addr, identity, trust, ALPN)
    }

    /// Endpoint only for pairing: accepts any Ed25519 key (the peer is not yet
    /// trusted) but requires the other end to sign the handshake with it,
    /// so [`Connection::peer`] is the real key of the other side. It uses another ALPN
    /// ([`PAIRING_ALPN`]): it cannot talk to a normal endpoint or vice versa, and the
    /// only thing that passes through it is pairing.
    pub fn bind_pairing(addr: SocketAddr, identity: &Identity) -> Result<Self, TransportError> {
        Self::bind_with(addr, identity, Arc::new(AnyKey), PAIRING_ALPN)
    }

    fn bind_with(
        addr: SocketAddr,
        identity: &Identity,
        trust: Arc<dyn PeerTrust>,
        alpn: &[u8],
    ) -> Result<Self, TransportError> {
        let (server, client) = configs(identity, trust, alpn)?;
        let socket = udp_socket(addr).map_err(TransportError::Bind)?;
        let runtime = quinn::default_runtime()
            .ok_or_else(|| TransportError::Bind(std::io::Error::other("no async runtime")))?;
        let mut inner = quinn::Endpoint::new(
            quinn::EndpointConfig::default(),
            Some(server),
            socket,
            runtime,
        )
        .map_err(TransportError::Bind)?;
        inner.set_default_client_config(client);
        Ok(Self { inner })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        self.inner.local_addr().map_err(TransportError::Bind)
    }

    /// Connects with the agent at `addr`. Fails if its key is not trusted.
    ///
    /// If it is the other side that does not trust us, TLS 1.3 may consider the
    /// handshake finished here before the server rejects it: the
    /// rejection arrives as an error on the first operation of the connection.
    pub async fn connect(&self, addr: SocketAddr) -> Result<Connection, TransportError> {
        let conn = self.inner.connect(addr, TLS_SERVER_NAME)?.await?;
        Connection::new(conn)
    }

    /// Waits for the next incoming connection; `None` if the endpoint closed.
    /// An unpaired peer comes out as `Some(Err(..))`.
    pub async fn accept(&self) -> Option<Result<Connection, TransportError>> {
        let incoming = self.inner.accept().await?;
        Some(match incoming.await {
            Ok(conn) => Connection::new(conn),
            Err(e) => Err(e.into()),
        })
    }

    /// Closes all the connections and stops accepting.
    pub fn close(&self) {
        self.inner.close(0u32.into(), b"closed");
    }

    /// Waits for the peers to find out about the close.
    pub async fn wait_idle(&self) {
        self.inner.wait_idle().await;
    }
}

/// The UDP socket for `addr`. On `[::]` it is dual-stack: the configured peer is usually an
/// IPv4 address while the cable's link-local the agent learns by mDNS is IPv6, and both go
/// through the same endpoint (an IPv4-only one refuses IPv6 with `InvalidRemoteAddress`).
/// Windows makes IPv6 sockets IPv6-only by default, hence `set_only_v6(false)`. A host
/// without IPv6 falls back to `0.0.0.0` on the same port.
fn udp_socket(addr: SocketAddr) -> std::io::Result<std::net::UdpSocket> {
    let dual = match addr {
        SocketAddr::V6(v6) if v6.ip().is_unspecified() => v6,
        _ => return std::net::UdpSocket::bind(addr),
    };
    match dual_stack(dual) {
        Ok(socket) => Ok(socket),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => Err(e),
        Err(e) => {
            tracing::warn!(%addr, "no dual-stack socket ({e}): IPv4 only");
            std::net::UdpSocket::bind(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), dual.port()))
        }
    }
}

fn dual_stack(addr: SocketAddrV6) -> std::io::Result<std::net::UdpSocket> {
    let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_only_v6(false)?;
    socket.bind(&SocketAddr::V6(addr).into())?;
    Ok(socket.into())
}

fn configs(
    identity: &Identity,
    trust: Arc<dyn PeerTrust>,
    alpn: &[u8],
) -> Result<(quinn::ServerConfig, quinn::ClientConfig), TransportError> {
    // ring and not aws-lc-rs: rcgen already uses it and it builds without cmake on Windows.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls = identity.tls_certificate()?;
    let cert = CertificateDer::from(tls.cert_der);
    let key = || PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(tls.key_pkcs8_der.to_vec()));

    let mut server = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(PinnedClientVerifier::new(
            trust.clone(),
            &provider,
        )))
        .with_single_cert(vec![cert.clone()], key())?;
    server.alpn_protocols = vec![alpn.to_vec()];

    let mut client = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedServerVerifier::new(trust, &provider)))
        .with_client_auth_cert(vec![cert], key())?;
    client.alpn_protocols = vec![alpn.to_vec()];

    let server = quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(server)?));
    let client = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(client)?));
    Ok((server, client))
}

/// Connection authenticated with a trusted peer. Copies share the connection and its
/// [`Qos`].
#[derive(Clone)]
pub struct Connection {
    inner: quinn::Connection,
    peer: PublicKey,
    qos: Qos,
}

impl Connection {
    fn new(inner: quinn::Connection) -> Result<Self, TransportError> {
        let peer = inner
            .peer_identity()
            .and_then(|id| id.downcast::<Vec<CertificateDer<'static>>>().ok())
            .and_then(|certs| certs.first().and_then(peer_public_key))
            .ok_or(TransportError::NoPeerIdentity)?;
        Ok(Self {
            inner,
            peer,
            qos: Qos::default(),
        })
    }

    /// Priority, cap and pause of the data over this connection. Without [`Self::with_qos`] it is
    /// one of its own, with no known path (and no cap).
    /// Identifies this connection while it lives (quinn's `stable_id`).
    pub fn id(&self) -> usize {
        self.inner.stable_id()
    }

    pub fn qos(&self) -> &Qos {
        &self.qos
    }

    /// The same connection with the [`Qos`] of its path, shared with the other connections
    /// over that path ([`LinkQos`](crate::qos::LinkQos)): that way the cap holds for all
    /// together. It is set on opening or accepting it, before cloning it.
    #[must_use]
    pub fn with_qos(mut self, qos: Qos) -> Self {
        self.qos = qos;
        self
    }

    /// The peer's key, already verified against the local trust.
    pub fn peer(&self) -> PublicKey {
        self.peer
    }

    pub fn remote_address(&self) -> SocketAddr {
        self.inner.remote_address()
    }

    /// QUIC's round-trip estimate.
    pub fn rtt(&self) -> Duration {
        self.inner.rtt()
    }

    /// Opens the control stream (done by whoever connected) and exchanges the
    /// [`Hello`]s: returns the stream and the peer's Hello.
    pub async fn open_control(
        &self,
        hello: &Hello,
    ) -> Result<(ControlStream, Hello), TransportError> {
        let (send, recv) = self.inner.open_bi().await?;
        let mut stream = ControlStream::new(send, recv);
        // QUIC does not announce the stream until it carries data: the Hello goes first.
        stream.send(hello).await?;
        let theirs = stream.recv_hello().await?;
        Ok((stream, theirs))
    }

    /// Accepts the control stream the peer opened and exchanges the
    /// [`Hello`]s.
    pub async fn accept_control(
        &self,
        hello: &Hello,
    ) -> Result<(ControlStream, Hello), TransportError> {
        let (send, recv) = self.inner.accept_bi().await?;
        let mut stream = ControlStream::new(send, recv);
        let theirs = stream.recv_hello().await;
        // The own Hello goes out even if the peer's is of another version, so
        // that the other side also sees the typed error and not a silent cut.
        let sent = stream.send(hello).await;
        let theirs = theirs?;
        sent?;
        Ok((stream, theirs))
    }

    /// Sends an unreliable datagram (mouse movement, for example).
    pub fn send_datagram(&self, data: Bytes) -> Result<(), TransportError> {
        Ok(self.inner.send_datagram(data)?)
    }

    pub async fn read_datagram(&self) -> Result<Bytes, TransportError> {
        Ok(self.inner.read_datagram().await?)
    }

    /// Maximum datagram size the path admits now; `None` if the peer
    /// does not accept datagrams.
    pub fn max_datagram_size(&self) -> Option<usize> {
        self.inner.max_datagram_size()
    }

    pub fn close(&self, reason: &str) {
        self.inner.close(0u32.into(), reason.as_bytes());
    }

    /// Waits for the connection to close and returns the reason.
    pub async fn closed(&self) -> TransportError {
        self.inner.closed().await.into()
    }
}
