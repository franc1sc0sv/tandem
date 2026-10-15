//! Transporte entre agentes: QUIC (quinn) con TLS 1.3 mutuo y pinning de la
//! llave Ed25519 del par (DESIGN §4.1, §9).
//!
//! Cada agente tiene un [`Endpoint`] que escucha y conecta. Los dos lados
//! presentan el certificado autofirmado de su [`Identity`](crate::identity::Identity)
//! y solo se acepta a un par cuya llave esté en la confianza local
//! ([`PeerTrust`], normalmente el [`TrustStore`](crate::identity::TrustStore)).
//!
//! Sobre una [`Connection`] hay dos canales:
//! - el stream de control ([`ControlStream`]), confiable y ordenado, que empieza
//!   con un intercambio de [`Hello`](tandem_proto::Hello);
//! - datagramas no confiables, para lo que pierde valor si llega tarde (el
//!   movimiento del mouse, DESIGN §7).

mod control;
mod endpoint;
mod error;
mod verify;

pub use control::ControlStream;
pub use endpoint::{Connection, Endpoint};
pub use error::TransportError;
pub use verify::{PeerTrust, peer_public_key};

/// Protocolo de aplicación anunciado en ALPN; otra aplicación QUIC en el
/// mismo puerto no llega a hablar con el agente.
pub const ALPN: &[u8] = b"tandem/1";
