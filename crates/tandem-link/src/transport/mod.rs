//! Transport between agents: QUIC (quinn) with mutual TLS 1.3 and pinning of the
//! peer's Ed25519 key (DESIGN §4.1, §9).
//!
//! Each agent has an [`Endpoint`] that listens and connects. Both sides
//! present the self-signed certificate of their [`Identity`](crate::identity::Identity)
//! and only a peer whose key is in the local trust is accepted
//! ([`PeerTrust`], normally the [`TrustStore`](crate::identity::TrustStore)).
//!
//! For pairing there is a separate endpoint ([`Endpoint::bind_pairing`]), with another ALPN,
//! that accepts unknown keys: it only lives while the user pairs.
//!
//! Over a [`Connection`] there are two channels:
//! - the control stream ([`ControlStream`]), reliable and ordered, which starts
//!   with an exchange of [`Hello`](tandem_proto::Hello);
//! - unreliable datagrams, for what loses value if it arrives late (the
//!   mouse movement, DESIGN §7).

mod control;
mod endpoint;
mod error;
mod verify;

pub use control::{ControlReceiver, ControlSender, ControlStream};

/// The write side of a data stream ([`ControlStream::into_data`]), to
/// name it without depending on quinn.
pub type DataSender = crate::qos::DataStream<quinn::SendStream>;
pub use endpoint::{Connection, Endpoint};
pub use error::TransportError;
pub use verify::{PeerTrust, peer_public_key};

/// Application protocol announced in ALPN; another QUIC application on the
/// same port does not get to talk to the agent.
pub const ALPN: &[u8] = b"tandem/1";

/// ALPN of the pairing endpoint ([`Endpoint::bind_pairing`]).
pub const PAIRING_ALPN: &[u8] = b"tandem-pair/1";
