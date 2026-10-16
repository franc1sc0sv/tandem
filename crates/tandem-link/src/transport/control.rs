use serde::Serialize;
use serde::de::DeserializeOwned;
use tandem_proto::{FrameDecoder, Hello, decode, decode_hello, encode_frame};

use super::TransportError;

/// Stream bidireccional confiable con mensajes de `tandem-proto` en tramas.
pub struct ControlStream {
    tx: ControlSender,
    rx: ControlReceiver,
}

/// Mitad de escritura de un [`ControlStream`].
pub struct ControlSender {
    send: quinn::SendStream,
}

/// Mitad de lectura de un [`ControlStream`].
pub struct ControlReceiver {
    recv: quinn::RecvStream,
    decoder: FrameDecoder,
}

/// Lo que se pide al stream en cada lectura; las tramas de control son
/// chicas, así que alcanza para varias.
const READ_CHUNK: usize = 16 * 1024;

impl ControlStream {
    pub(crate) fn new(send: quinn::SendStream, recv: quinn::RecvStream) -> Self {
        Self {
            tx: ControlSender { send },
            rx: ControlReceiver {
                recv,
                decoder: FrameDecoder::new(),
            },
        }
    }

    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<(), TransportError> {
        self.tx.send(msg).await
    }

    /// Siguiente mensaje; `None` si el par cerró el stream limpiamente.
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<Option<T>, TransportError> {
        self.rx.recv().await
    }

    pub(crate) async fn recv_hello(&mut self) -> Result<Hello, TransportError> {
        let payload = self
            .rx
            .next_frame()
            .await?
            .ok_or(TransportError::MissingHello)?;
        Ok(decode_hello(&payload)?)
    }

    /// Avisa al par de que no se enviará nada más por este stream.
    pub fn finish(&mut self) -> Result<(), TransportError> {
        self.tx.finish()
    }

    /// Separa las dos mitades, para leer y escribir a la vez: quien manda
    /// mucho (el portapapeles) no deja de leer mientras el control de flujo
    /// lo frena, y dos lados escribiendo a la vez no se bloquean entre sí.
    pub fn split(self) -> (ControlSender, ControlReceiver) {
        (self.tx, self.rx)
    }
}

impl ControlSender {
    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<(), TransportError> {
        let frame = encode_frame(msg)?;
        self.send.write_all(&frame).await?;
        Ok(())
    }

    /// Avisa al par de que no se enviará nada más por este stream.
    pub fn finish(&mut self) -> Result<(), TransportError> {
        Ok(self.send.finish()?)
    }
}

impl ControlReceiver {
    /// Siguiente mensaje; `None` si el par cerró el stream limpiamente.
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<Option<T>, TransportError> {
        match self.next_frame().await? {
            Some(payload) => Ok(Some(decode(&payload)?)),
            None => Ok(None),
        }
    }

    async fn next_frame(&mut self) -> Result<Option<Vec<u8>>, TransportError> {
        loop {
            if let Some(frame) = self.decoder.next_frame()? {
                return Ok(Some(frame));
            }
            match self.recv.read_chunk(READ_CHUNK, true).await? {
                Some(chunk) => self.decoder.push(&chunk.bytes),
                None if self.decoder.buffered() == 0 => return Ok(None),
                None => return Err(TransportError::TruncatedFrame),
            }
        }
    }
}
