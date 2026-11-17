use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tandem_proto::{FrameDecoder, Hello, MeasureReport, decode, decode_hello, encode_frame};

use super::TransportError;
use crate::metrics::probe::{receive_probe, send_probe};
use crate::qos::{DataStream, Qos, StreamClass};

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

    /// La prioridad del lado de escritura (ver [`StreamClass`]). Cada lado pone la de lo
    /// que manda: la de un stream aceptado no viene del que lo abrió.
    pub fn set_class(&self, class: StreamClass) -> Result<(), TransportError> {
        self.tx.set_class(class)
    }

    /// Pasa el stream a datos (túneles, archivos): prioridad mínima y la pausa y el tope
    /// de `qos`, el de su conexión ([`Connection::qos`](super::Connection::qos)). Se sigue
    /// leyendo con el receptor.
    pub fn into_data(
        self,
        qos: &Qos,
    ) -> Result<(DataStream<quinn::SendStream>, ControlReceiver), TransportError> {
        self.set_class(StreamClass::Data)?;
        Ok((qos.data_stream(self.tx.send), self.rx))
    }

    /// Manda la prueba de ancho de banda por este stream (ya abierto como
    /// [`Channel::Measure`](tandem_proto::Channel::Measure)) y espera lo que midió el otro
    /// lado. Va con la prioridad de los datos pero sin el tope (medir a través del tope
    /// daría el tope), y se corta si `qos`, el del camino, pausa los datos: la entrada
    /// manda ([`TransportError::InputLate`]).
    pub async fn measure(
        self,
        duration: Duration,
        qos: &Qos,
    ) -> Result<MeasureReport, TransportError> {
        self.set_class(StreamClass::Data)?;
        let ControlStream { mut tx, mut rx } = self;
        let end = send_probe(&mut tx.send, duration, qos)
            .await
            .map_err(TransportError::Data)?;
        // El otro lado contesta igual; se lee para cerrar el stream limpio.
        let report = rx.recv().await?.ok_or(TransportError::ControlClosed)?;
        if end.paused {
            return Err(TransportError::InputLate);
        }
        Ok(report)
    }

    /// El otro lado de [`Self::measure`]: lee la prueba hasta el fin y contesta lo medido.
    pub async fn answer_measure(self) -> Result<MeasureReport, TransportError> {
        let ControlStream { mut tx, mut rx } = self;
        // Lo que llegó junto con la apertura del canal ya es de la prueba.
        let already = rx.decoder.buffered();
        let report = receive_probe(&mut rx.recv, already)
            .await
            .map_err(TransportError::Data)?;
        tx.send(&report).await?;
        tx.finish()?;
        Ok(report)
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

    /// Ver [`ControlStream::set_class`].
    pub fn set_class(&self, class: StreamClass) -> Result<(), TransportError> {
        Ok(self.send.set_priority(class.priority())?)
    }

    /// La prioridad que tiene ahora el lado de escritura.
    pub fn priority(&self) -> Result<i32, TransportError> {
        Ok(self.send.priority()?)
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
