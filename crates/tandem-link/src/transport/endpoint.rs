use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tandem_proto::Hello;

use super::control::ControlStream;
use super::verify::{
    AnyKey, PeerTrust, PinnedClientVerifier, PinnedServerVerifier, peer_public_key,
};
use super::{ALPN, PAIRING_ALPN, TransportError};
use crate::identity::{Identity, PublicKey, TLS_SERVER_NAME};

/// Endpoint QUIC de un agente: escucha y también abre conexiones, con el
/// mismo certificado y la misma confianza en los dos papeles.
pub struct Endpoint {
    inner: quinn::Endpoint,
}

impl Endpoint {
    /// Abre el socket UDP en `addr` (puerto 0 = cualquiera libre).
    ///
    /// Tiene que llamarse dentro de un runtime de tokio.
    pub fn bind(
        addr: SocketAddr,
        identity: &Identity,
        trust: Arc<dyn PeerTrust>,
    ) -> Result<Self, TransportError> {
        Self::bind_with(addr, identity, trust, ALPN)
    }

    /// Endpoint solo para emparejarse: acepta cualquier llave Ed25519 (el par todavía
    /// no es de confianza) pero exige que la otra punta firme el handshake con ella,
    /// así [`Connection::peer`] es la llave real del otro lado. Usa otro ALPN
    /// ([`PAIRING_ALPN`]): no puede hablar con un endpoint normal ni al revés, y lo
    /// único que pasa por él es el emparejamiento.
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
        let mut inner = quinn::Endpoint::server(server, addr).map_err(TransportError::Bind)?;
        inner.set_default_client_config(client);
        Ok(Self { inner })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        self.inner.local_addr().map_err(TransportError::Bind)
    }

    /// Conecta con el agente en `addr`. Falla si su llave no es de confianza.
    ///
    /// Si es el otro lado el que no confía en nosotros, TLS 1.3 puede dar el
    /// handshake por terminado aquí antes de que el servidor lo rechace: el
    /// rechazo llega como error en la primera operación de la conexión.
    pub async fn connect(&self, addr: SocketAddr) -> Result<Connection, TransportError> {
        let conn = self.inner.connect(addr, TLS_SERVER_NAME)?.await?;
        Connection::new(conn)
    }

    /// Espera la siguiente conexión entrante; `None` si el endpoint se cerró.
    /// Un par no emparejado sale como `Some(Err(..))`.
    pub async fn accept(&self) -> Option<Result<Connection, TransportError>> {
        let incoming = self.inner.accept().await?;
        Some(match incoming.await {
            Ok(conn) => Connection::new(conn),
            Err(e) => Err(e.into()),
        })
    }

    /// Cierra todas las conexiones y deja de aceptar.
    pub fn close(&self) {
        self.inner.close(0u32.into(), b"cierre");
    }

    /// Espera a que los pares se enteren del cierre.
    pub async fn wait_idle(&self) {
        self.inner.wait_idle().await;
    }
}

fn configs(
    identity: &Identity,
    trust: Arc<dyn PeerTrust>,
    alpn: &[u8],
) -> Result<(quinn::ServerConfig, quinn::ClientConfig), TransportError> {
    // ring y no aws-lc-rs: ya lo usa rcgen y compila sin cmake en Windows.
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

/// Conexión autenticada con un par de confianza.
#[derive(Clone)]
pub struct Connection {
    inner: quinn::Connection,
    peer: PublicKey,
}

impl Connection {
    fn new(inner: quinn::Connection) -> Result<Self, TransportError> {
        let peer = inner
            .peer_identity()
            .and_then(|id| id.downcast::<Vec<CertificateDer<'static>>>().ok())
            .and_then(|certs| certs.first().and_then(peer_public_key))
            .ok_or(TransportError::NoPeerIdentity)?;
        Ok(Self { inner, peer })
    }

    /// Llave del par, ya verificada contra la confianza local.
    pub fn peer(&self) -> PublicKey {
        self.peer
    }

    pub fn remote_address(&self) -> SocketAddr {
        self.inner.remote_address()
    }

    /// Estimación de ida y vuelta de QUIC.
    pub fn rtt(&self) -> Duration {
        self.inner.rtt()
    }

    /// Abre el stream de control (lo hace quien conectó) e intercambia los
    /// [`Hello`]: devuelve el stream y el Hello del par.
    pub async fn open_control(
        &self,
        hello: &Hello,
    ) -> Result<(ControlStream, Hello), TransportError> {
        let (send, recv) = self.inner.open_bi().await?;
        let mut stream = ControlStream::new(send, recv);
        // QUIC no anuncia el stream hasta que lleva datos: el Hello va primero.
        stream.send(hello).await?;
        let theirs = stream.recv_hello().await?;
        Ok((stream, theirs))
    }

    /// Acepta el stream de control que abrió el par e intercambia los
    /// [`Hello`].
    pub async fn accept_control(
        &self,
        hello: &Hello,
    ) -> Result<(ControlStream, Hello), TransportError> {
        let (send, recv) = self.inner.accept_bi().await?;
        let mut stream = ControlStream::new(send, recv);
        let theirs = stream.recv_hello().await;
        // El Hello propio sale aunque el del par sea de otra versión, para
        // que el otro lado también vea el error tipado y no un corte mudo.
        let sent = stream.send(hello).await;
        let theirs = theirs?;
        sent?;
        Ok((stream, theirs))
    }

    /// Envía un datagrama no confiable (movimiento del mouse, por ejemplo).
    pub fn send_datagram(&self, data: Bytes) -> Result<(), TransportError> {
        Ok(self.inner.send_datagram(data)?)
    }

    pub async fn read_datagram(&self) -> Result<Bytes, TransportError> {
        Ok(self.inner.read_datagram().await?)
    }

    /// Tamaño máximo de datagrama que admite ahora la ruta; `None` si el par
    /// no acepta datagramas.
    pub fn max_datagram_size(&self) -> Option<usize> {
        self.inner.max_datagram_size()
    }

    pub fn close(&self, reason: &str) {
        self.inner.close(0u32.into(), reason.as_bytes());
    }

    /// Espera al cierre de la conexión y devuelve el motivo.
    pub async fn closed(&self) -> TransportError {
        self.inner.closed().await.into()
    }
}
