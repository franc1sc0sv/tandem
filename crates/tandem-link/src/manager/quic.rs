use std::net::SocketAddr;
use std::sync::Arc;

use tandem_proto::{ControlMsg, Hello};

use super::Prober;
use crate::transport::{Connection, ControlStream, Endpoint, TransportError};

/// [`Prober`] over the QUIC transport: one connection per link and
/// [`ControlMsg::Ping`]/[`ControlMsg::Pong`] pings over its control stream.
///
/// The other agent answers with [`answer_pings`].
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

impl QuicProber {
    /// A session over a connection that already exists, opened by either side: an agent
    /// the other cannot dial still pings over the connection the other opened (T-237).
    pub async fn session_over(&self, conn: Connection) -> Result<QuicSession, TransportError> {
        let (control, _) = conn.open_control(&self.hello).await?;
        Ok(QuicSession {
            conn,
            control,
            nonce: 0,
        })
    }
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
                // A late Pong or another message: it does not count as an answer.
                Some(_) => {}
                None => return Err(TransportError::ControlClosed),
            }
        }
    }

    fn handle(s: &QuicSession) -> Connection {
        s.conn.clone()
    }
}

/// Accepting side: takes the control stream that the peer's [`QuicProber`]
/// opened and answers each Ping until the peer closes it.
pub async fn answer_pings(conn: Connection, hello: &Hello) -> Result<(), TransportError> {
    let (mut control, _) = conn.accept_control(hello).await?;
    while let Some(msg) = control.recv::<ControlMsg>().await? {
        if let ControlMsg::Ping { nonce } = msg {
            control.send(&ControlMsg::Pong { nonce }).await?;
        }
    }
    Ok(())
}
