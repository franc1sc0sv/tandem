//! In-memory IPC, to test the agent without a socket or pipe.

use std::io;

use tokio::io::{DuplexStream, duplex};
use tokio::sync::mpsc;

use super::Listener;

const BUF: usize = 64 * 1024;

pub struct MemListener(mpsc::UnboundedReceiver<DuplexStream>);

/// With it connections are opened toward the corresponding [`MemListener`].
#[derive(Clone)]
pub struct MemConnector(mpsc::UnboundedSender<DuplexStream>);

pub fn pair() -> (MemListener, MemConnector) {
    let (tx, rx) = mpsc::unbounded_channel();
    (MemListener(rx), MemConnector(tx))
}

impl MemConnector {
    pub fn connect(&self) -> io::Result<DuplexStream> {
        let (client, server) = duplex(BUF);
        self.0
            .send(server)
            .map_err(|_| io::Error::from(io::ErrorKind::ConnectionRefused))?;
        Ok(client)
    }
}

impl Listener for MemListener {
    type Stream = DuplexStream;

    async fn accept(&mut self) -> io::Result<DuplexStream> {
        match self.0.recv().await {
            Some(s) => Ok(s),
            // Without connectors nothing more will arrive: like a socket with no clients.
            None => std::future::pending().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_after_listener_drop_is_refused() {
        let (l, c) = pair();
        drop(l);
        assert_eq!(
            c.connect().unwrap_err().kind(),
            io::ErrorKind::ConnectionRefused
        );
    }
}
