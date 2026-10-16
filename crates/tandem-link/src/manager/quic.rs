use std::net::SocketAddr;
use std::sync::Arc;

use tandem_proto::{ControlMsg, Hello};

use super::Prober;
use crate::transport::{Connection, ControlStream, Endpoint, TransportError};

/// [`Prober`] sobre el transporte QUIC: una conexión por enlace y pings
/// [`ControlMsg::Ping`]/[`ControlMsg::Pong`] por su stream de control.
///
/// El otro agente contesta con [`answer_pings`].
pub struct QuicProber {
    endpoint: Arc<Endpoint>,
    hello: Hello,
}

impl QuicProber {
    pub fn new(endpoint: Arc<Endpoint>, hello: Hello) -> Self {
        Self { endpoint, hello }
    }
}

pub struct QuicSession {
    conn: Connection,
    control: ControlStream,
    nonce: u64,
}

impl Prober for QuicProber {
    type Session = QuicSession;
    type Handle = Connection;
    type Error = TransportError;

    async fn connect(&self, addr: SocketAddr) -> Result<QuicSession, TransportError> {
        let conn = self.endpoint.connect(addr).await?;
        let (control, _) = conn.open_control(&self.hello).await?;
        Ok(QuicSession {
            conn,
            control,
            nonce: 0,
        })
    }

    async fn ping(&self, s: &mut QuicSession) -> Result<(), TransportError> {
        s.nonce = s.nonce.wrapping_add(1);
        let nonce = s.nonce;
        s.control.send(&ControlMsg::Ping { nonce }).await?;
        loop {
            match s.control.recv::<ControlMsg>().await? {
                Some(ControlMsg::Pong { nonce: n }) if n == nonce => return Ok(()),
                // Un Pong atrasado u otro mensaje: no cuenta como respuesta.
                Some(_) => {}
                None => return Err(TransportError::ControlClosed),
            }
        }
    }

    fn handle(s: &QuicSession) -> Connection {
        s.conn.clone()
    }
}

/// Lado que acepta: toma el stream de control que abrió el [`QuicProber`] del
/// par y contesta cada Ping hasta que el par lo cierre.
pub async fn answer_pings(conn: Connection, hello: &Hello) -> Result<(), TransportError> {
    let (mut control, _) = conn.accept_control(hello).await?;
    while let Some(msg) = control.recv::<ControlMsg>().await? {
        if let ControlMsg::Ping { nonce } = msg {
            control.send(&ControlMsg::Pong { nonce }).await?;
        }
    }
    Ok(())
}
