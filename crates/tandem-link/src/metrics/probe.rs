//! The bandwidth test, over any pair of byte streams: QUIC's data
//! stream ([`ControlStream::measure`](crate::transport::ControlStream::measure)) or,
//! in tests, the simulated Wi-Fi.
//!
//! The receiver measures: what the sender writes stays for a while in its queues (QUIC's,
//! the system's), so counting on the writing side gives too much.
//!
//! It runs without the data cap (measuring through the cap would give the cap), but respects the
//! pause of T-101: if input exceeds its target, the test is cut off (DECISIONS).

use std::io;
use std::time::Duration;

use tandem_proto::MeasureReport;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{self, Instant};

use crate::qos::Qos;

const CHUNK: usize = 64 * 1024;

/// How the sending part ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeEnd {
    /// Bytes written.
    pub sent: u64,
    /// Cut short because input exceeded its target (`qos` paused).
    pub paused: bool,
}

/// Writes without stopping for `duration` and closes the write side; earlier, if `qos` (the
/// one of the path being measured) pauses the data.
pub async fn send_probe<W: AsyncWrite + Unpin>(
    w: &mut W,
    duration: Duration,
    qos: &Qos,
) -> io::Result<ProbeEnd> {
    let chunk = vec![0u8; CHUNK];
    let end = Instant::now() + duration;
    let mut gate = qos.watch_gate();
    let mut sent = 0u64;
    let mut paused = !gate.borrow_and_update().is_open();
    // A write held back by flow control neither lengthens the test nor delays the
    // cut: it is cancelled (and then it wrote nothing).
    while !paused {
        tokio::select! {
            written = time::timeout_at(end, w.write(&chunk)) => match written {
                Ok(n) => {
                    let n = n?;
                    if n == 0 {
                        return Err(io::ErrorKind::WriteZero.into());
                    }
                    sent += n as u64;
                }
                Err(_) => break,
            },
            changed = gate.changed() => {
                if changed.is_err() {
                    break;
                }
                paused = !gate.borrow_and_update().is_open();
            }
        }
    }
    if paused {
        tracing::info!(sent, "bandwidth test cut off: input exceeded its target");
    }
    w.shutdown().await?;
    Ok(ProbeEnd { sent, paused })
}

/// Reads until the end and counts what arrived after the first read, over the time that
/// goes from that first read to the last: that way it does not count the time taken to start.
/// `already` are bytes of the test that were already read (along with the channel opening):
/// they count as the first read.
pub async fn receive_probe<R: AsyncRead + Unpin>(
    r: &mut R,
    already: usize,
) -> io::Result<MeasureReport> {
    let mut buf = vec![0u8; CHUNK];
    let mut first = (already > 0).then(Instant::now);
    let mut last = first;
    let mut bytes = 0u64;
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let now = Instant::now();
        if first.is_none() {
            first = Some(now);
        } else {
            bytes += n as u64;
        }
        last = Some(now);
    }
    let micros = match (first, last) {
        (Some(f), Some(l)) => u64::try_from(l.duration_since(f).as_micros()).unwrap_or(u64::MAX),
        _ => 0,
    };
    Ok(MeasureReport { bytes, micros })
}

#[cfg(test)]
mod tests {
    use tokio::io::duplex;

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn without_data_there_is_no_measurement() {
        let (mut a, mut b) = duplex(1024);
        a.shutdown().await.unwrap();
        let r = receive_probe(&mut b, 0).await.unwrap();
        assert_eq!(r.bytes_per_sec(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn the_sender_stops_in_time_even_if_nobody_reads() {
        let (mut a, _b) = duplex(1024);
        let t0 = Instant::now();
        let end = send_probe(&mut a, Duration::from_secs(3), &Qos::default())
            .await
            .unwrap();
        assert_eq!(
            end,
            ProbeEnd {
                sent: 1024,
                paused: false
            }
        );
        assert_eq!(Instant::now() - t0, Duration::from_secs(3));
    }

    #[tokio::test(start_paused = true)]
    async fn what_was_already_read_with_the_opening_marks_the_start() {
        let (mut a, mut b) = duplex(1024);
        let recv = tokio::spawn(async move { receive_probe(&mut b, 10).await.unwrap() });
        time::sleep(Duration::from_secs(1)).await;
        a.write_all(&[0; 500]).await.unwrap();
        a.shutdown().await.unwrap();
        let r = recv.await.unwrap();
        assert_eq!((r.bytes, r.micros), (500, 1_000_000));
        assert_eq!(r.bytes_per_sec(), Some(500));
    }
}
