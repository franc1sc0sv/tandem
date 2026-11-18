use std::io;

use tandem_proto::ProtoError;

use crate::identity::IdentityError;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("identidad: {0}")]
    Identity(#[from] IdentityError),
    #[error("configuración TLS: {0}")]
    Tls(#[from] rustls::Error),
    #[error("la configuración TLS no sirve para QUIC: {0}")]
    QuicTls(#[from] quinn::crypto::rustls::NoInitialCipherSuite),
    #[error("no se pudo abrir el socket: {0}")]
    Bind(#[source] io::Error),
    #[error("no se pudo iniciar la conexión: {0}")]
    Connect(#[from] quinn::ConnectError),
    /// Incluye el rechazo del par en el handshake (llave no emparejada).
    #[error("conexión: {0}")]
    Connection(#[from] quinn::ConnectionError),
    #[error("escritura en el stream de control: {0}")]
    Write(#[from] quinn::WriteError),
    #[error("lectura del stream de control: {0}")]
    Read(#[from] quinn::ReadError),
    #[error("el stream de control ya estaba cerrado")]
    ClosedStream(#[from] quinn::ClosedStream),
    #[error("datagrama: {0}")]
    Datagram(#[from] quinn::SendDatagramError),
    #[error("protocolo: {0}")]
    Proto(#[from] ProtoError),
    #[error("el par cerró el stream de control a mitad de una trama")]
    TruncatedFrame,
    #[error("el par cerró el stream de control")]
    ControlClosed,
    #[error("el par cerró el stream de control antes de su Hello")]
    MissingHello,
    #[error("el par no presentó un certificado Ed25519 válido")]
    NoPeerIdentity,
    #[error("escritura en un stream de datos: {0}")]
    Data(#[source] io::Error),
    #[error("la entrada pasó su objetivo: se cortó la prueba de ancho de banda")]
    InputLate,
}

impl TransportError {
    /// El par cortó el handshake con una alerta TLS de certificado (malo, desconocido, CA
    /// desconocida o acceso denegado, que es lo que rustls manda ante un verificador de
    /// aplicación): no confía en nuestra llave. Los errores de TLS van en `0x100 + alerta`
    /// (RFC 9001 §4.8). Con QUIC el rechazo del cliente llega ya con la conexión abierta,
    /// así que también aparece al leer el primer stream.
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
