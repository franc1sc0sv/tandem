//! IPC en memoria, para probar el agente sin socket ni pipe.

use std::io;

use tokio::io::{DuplexStream, duplex};
use tokio::sync::mpsc;

use super::Listener;

const BUF: usize = 64 * 1024;

pub struct MemListener(mpsc::UnboundedReceiver<DuplexStream>);

/// Con él se abren conexiones hacia el [`MemListener`] correspondiente.
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
            // Sin conectores no llegará nada más: como un socket sin clientes.
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
