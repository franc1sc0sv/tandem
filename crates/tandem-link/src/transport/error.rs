use std::io;

use tandem_proto::ProtoError;

use crate::identity::IdentityError;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("identity: {0}")]
    Identity(#[from] IdentityError),
    #[error("TLS configuration: {0}")]
    Tls(#[from] rustls::Error),
    #[error("the TLS configuration is not valid for QUIC: {0}")]
    QuicTls(#[from] quinn::crypto::rustls::NoInitialCipherSuite),
    #[error("could not open the socket: {0}")]
    Bind(#[source] io::Error),
    #[error("could not start the connection: {0}")]
    Connect(#[from] quinn::ConnectError),
    /// Includes the peer's rejection in the handshake (unpaired key).
    #[error("connection: {0}")]
    Connection(#[from] quinn::ConnectionError),
    #[error("write on the control stream: {0}")]
    Write(#[from] quinn::WriteError),
    #[error("read of the control stream: {0}")]
    Read(#[from] quinn::ReadError),
    #[error("the control stream was already closed")]
    ClosedStream(#[from] quinn::ClosedStream),
    #[error("datagram: {0}")]
    Datagram(#[from] quinn::SendDatagramError),
    #[error("protocol: {0}")]
    Proto(#[from] ProtoError),
    #[error("the peer closed the control stream in the middle of a frame")]
    TruncatedFrame,
    #[error("the peer closed the control stream")]
    ControlClosed,
    #[error("the peer closed the control stream before its Hello")]
    MissingHello,
    #[error("the peer did not present a valid Ed25519 certificate")]
    NoPeerIdentity,
    #[error("write on a data stream: {0}")]
    Data(#[source] io::Error),
    #[error("input exceeded its target: the bandwidth test was cut off")]
    InputLate,
}

impl TransportError {
    /// The peer cut the handshake with a TLS certificate alert (bad, unknown, unknown
    /// CA or access denied, which is what rustls sends on an application
    /// verifier): it does not trust our key. TLS errors go as `0x100 + alert`
    /// (RFC 9001 §4.8). With QUIC the client's rejection arrives with the connection already open,
    /// so it also shows up when reading the first stream.
    pub fn is_peer_rejection(&self) -> bool {
        let closed = match self {
            TransportError::Connection(e)
            | TransportError::Read(quinn::ReadError::ConnectionLost(e)) => e,
            _ => return false,
        };
        match closed {
            quinn::ConnectionError::ConnectionClosed(c) => {
                matches!(u64::from(c.error_code), 0x12a | 0x12e | 0x130 | 0x131)
            }
            _ => false,
        }
    }
}
