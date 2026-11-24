use std::io;

use serde::Serialize;
use tandem_proto::encode_frame;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::Qos;
use crate::transport::TransportError;

/// Data writing that respects the pause and the cap of its connection ([`Qos`]).
///
/// The usual one is the write side of a QUIC stream of class
/// [`StreamClass::Data`](super::StreamClass::Data)
/// ([`ControlStream::into_data`](crate::transport::ControlStream::into_data)); in
/// tests, any [`AsyncWrite`].
#[derive(Debug)]
pub struct DataStream<W> {
    qos: Qos,
    inner: W,
    /// The input pause holds it back (bulk data); otherwise only the cap ([`Qos::live_stream`]).
    pausable: bool,
}

impl<W> DataStream<W> {
    pub fn new(qos: Qos, inner: W) -> Self {
        DataStream {
            qos,
            inner,
            pausable: true,
        }
    }

    /// Respects the cap but not the pause ([`Qos::acquire_capped`]).
    pub fn live(qos: Qos, inner: W) -> Self {
        DataStream {
            qos,
            inner,
            pausable: false,
        }
    }

    pub fn qos(&self) -> &Qos {
        &self.qos
    }

    pub fn get_ref(&self) -> &W {
        &self.inner
    }

    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: AsyncWrite + Unpin> DataStream<W> {
    /// Writes all of `buf`, in pieces as the cap lets out; when paused, waits.
    pub async fn write_all(&mut self, mut buf: &[u8]) -> io::Result<()> {
        while !buf.is_empty() {
            let n = if self.pausable {
                self.qos.acquire(buf.len()).await
            } else {
                self.qos.acquire_capped(buf.len()).await
            };
            let (now, rest) = buf.split_at(n.min(buf.len()));
            self.inner.write_all(now).await?;
            buf = rest;
        }
        Ok(())
    }

    /// A message in a `tandem-proto` frame.
    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<(), TransportError> {
        let frame = encode_frame(msg)?;
        self.write_all(&frame).await.map_err(TransportError::Data)
    }

    pub async fn flush(&mut self) -> io::Result<()> {
        self.inner.flush().await
    }

    /// Tells the other side that no more data is coming.
    pub async fn shutdown(&mut self) -> io::Result<()> {
        self.inner.shutdown().await
    }
}
