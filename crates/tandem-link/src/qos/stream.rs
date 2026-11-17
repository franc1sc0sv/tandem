use std::io;

use serde::Serialize;
use tandem_proto::encode_frame;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::Qos;
use crate::transport::TransportError;

/// Escritura de datos que respeta la pausa y el tope de su conexión ([`Qos`]).
///
/// Lo normal es el lado de escritura de un stream QUIC de clase
/// [`StreamClass::Data`](super::StreamClass::Data)
/// ([`ControlStream::into_data`](crate::transport::ControlStream::into_data)); en las
/// pruebas, cualquier [`AsyncWrite`].
#[derive(Debug)]
pub struct DataStream<W> {
    qos: Qos,
    inner: W,
}

impl<W> DataStream<W> {
    pub fn new(qos: Qos, inner: W) -> Self {
        DataStream { qos, inner }
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
    /// Escribe todo `buf`, de a lo que deje salir el tope; en pausa, espera.
    pub async fn write_all(&mut self, mut buf: &[u8]) -> io::Result<()> {
        while !buf.is_empty() {
            let n = self.qos.acquire(buf.len()).await;
            let (now, rest) = buf.split_at(n.min(buf.len()));
            self.inner.write_all(now).await?;
            buf = rest;
        }
        Ok(())
    }

    /// Un mensaje en una trama de `tandem-proto`.
    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<(), TransportError> {
        let frame = encode_frame(msg)?;
        self.write_all(&frame).await.map_err(TransportError::Data)
    }

    pub async fn flush(&mut self) -> io::Result<()> {
        self.inner.flush().await
    }

    /// Avisa al otro lado de que no hay más datos.
    pub async fn shutdown(&mut self) -> io::Result<()> {
        self.inner.shutdown().await
    }
}
