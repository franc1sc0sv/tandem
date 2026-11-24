use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tandem_proto::{FrameDecoder, Hello, MeasureReport, decode, decode_hello, encode_frame};

use super::TransportError;
use crate::metrics::probe::{receive_probe, send_probe};
use crate::qos::{DataStream, Qos, StreamClass};

/// Reliable bidirectional stream with `tandem-proto` messages in frames.
pub struct ControlStream {
    tx: ControlSender,
    rx: ControlReceiver,
}

/// Write half of a [`ControlStream`].
pub struct ControlSender {
    send: quinn::SendStream,
}

/// Read half of a [`ControlStream`].
pub struct ControlReceiver {
    recv: quinn::RecvStream,
    decoder: FrameDecoder,
}

/// What is asked of the stream on each read; control frames are
/// small, so it is enough for several.
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

    /// Next message; `None` if the peer closed the stream cleanly.
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

    /// Tells the peer that nothing more will be sent over this stream.
    pub fn finish(&mut self) -> Result<(), TransportError> {
        self.tx.finish()
    }

    /// The priority of the write side (see [`StreamClass`]). Each side sets that of what
    /// it sends: that of an accepted stream does not come from the one that opened it.
    pub fn set_class(&self, class: StreamClass) -> Result<(), TransportError> {
        self.tx.set_class(class)
    }

    /// Turns the stream into data (tunnels, files): minimum priority and the pause and cap
    /// of `qos`, that of its connection ([`Connection::qos`](super::Connection::qos)). Reading
    /// continues with the receiver.
    pub fn into_data(
        self,
        qos: &Qos,
    ) -> Result<(DataStream<quinn::SendStream>, ControlReceiver), TransportError> {
        self.set_class(StreamClass::Data)?;
        Ok((qos.data_stream(self.tx.send), self.rx))
    }

    /// Like [`Self::into_data`] but held back only by the cap, not by the input pause: the
    /// output of a tunnel's processes, which a person may be watching live.
    pub fn into_live_data(
        self,
        qos: &Qos,
    ) -> Result<(DataStream<quinn::SendStream>, ControlReceiver), TransportError> {
        self.set_class(StreamClass::Data)?;
        Ok((qos.live_stream(self.tx.send), self.rx))
    }

    /// Sends the bandwidth test over this stream (already opened as
    /// [`Channel::Measure`](tandem_proto::Channel::Measure)) and waits for what the other
    /// side measured. It goes with the data priority but without the cap (measuring through the cap
    /// would give the cap), and it is cut off if `qos`, that of the path, pauses data: input
    /// comes first ([`TransportError::InputLate`]).
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
        // The other side answers the same; it is read to close the stream cleanly.
        let report = rx.recv().await?.ok_or(TransportError::ControlClosed)?;
        if end.paused {
            return Err(TransportError::InputLate);
        }
        Ok(report)
    }

    /// The other side of [`Self::measure`]: reads the test to the end and answers what was measured.
    pub async fn answer_measure(self) -> Result<MeasureReport, TransportError> {
        let ControlStream { mut tx, mut rx } = self;
        // What arrived together with the channel opening already belongs to the test.
        let already = rx.decoder.buffered();
        let report = receive_probe(&mut rx.recv, already)
            .await
            .map_err(TransportError::Data)?;
        tx.send(&report).await?;
        tx.finish()?;
        Ok(report)
    }

    /// Splits the two halves, to read and write at the same time: whoever sends
    /// a lot (the clipboard) does not stop reading while flow control
    /// holds it back, and two sides writing at once do not block each other.
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

    /// Tells the peer that nothing more will be sent over this stream.
    pub fn finish(&mut self) -> Result<(), TransportError> {
        Ok(self.send.finish()?)
    }

    /// See [`ControlStream::set_class`].
    pub fn set_class(&self, class: StreamClass) -> Result<(), TransportError> {
        Ok(self.send.set_priority(class.priority())?)
    }

    /// The priority the write side has now.
    pub fn priority(&self) -> Result<i32, TransportError> {
        Ok(self.send.priority()?)
    }
}

impl ControlReceiver {
    /// Next message; `None` if the peer closed the stream cleanly.
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
