//! A simulated Wi-Fi with bandwidth, to test that data does not add latency to
//! input (T-101, DESIGN §6).
//!
//! Wi-Fi is a single medium: everything leaving a machine goes through the same queue (the
//! system's and the access point's) and that queue drains in arrival order at the
//! capacity of the moment. QUIC's priority orders what has not yet left, but it cannot
//! put a key ahead of the data already in that queue: if data fills it,
//! every key waits as long as it takes to drain. The model is just that: capacity, a
//! fixed propagation delay and a fixed-size queue.
//!
//! - Input ([`SimWifi::send_input`]) never waits to enter the queue: it is a few
//!   bytes and goes with the highest priority, so it enters in the first gap.
//! - Data ([`SimWifi::data_writer`]) waits until there is room in the queue, like a
//!   QUIC stream held back by the congestion window.
//!
//! - [`SimWifi::data_pipe`] also gives the other end ([`WifiReader`]): what is written arrives
//!   when it leaves the queue plus the propagation (T-102's bandwidth test).
//!   Only the sizes travel: it reads as zeros.
//!
//! It runs on the tokio clock (paused in tests): times are exact.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use tokio::time::{Instant, Sleep};

/// The smallest amount of room waited for before writing data: a packet.
const PACKET: u64 = 1500;

#[derive(Debug)]
struct Air {
    /// Bytes per second.
    capacity: u64,
    /// Propagation, the same for everything.
    base: Duration,
    /// What fits in the queue.
    buffer: u64,
    /// When the last thing that entered leaves the queue.
    busy_until: Instant,
    /// Data that entered the queue: when and how many bytes.
    data: Vec<(Instant, usize)>,
}

impl Air {
    fn transmit_time(&self, bytes: u64) -> Duration {
        let ns = u128::from(bytes) * 1_000_000_000 / u128::from(self.capacity.max(1));
        Duration::from_nanos(u64::try_from(ns).unwrap_or(u64::MAX))
    }

    /// Bytes waiting in the queue at `now`.
    fn backlog(&self, now: Instant) -> u64 {
        let left = self.busy_until.saturating_duration_since(now).as_nanos();
        u64::try_from(left * u128::from(self.capacity) / 1_000_000_000).unwrap_or(u64::MAX)
    }

    /// Puts `bytes` at the end of the queue; returns when they arrive at the other side.
    fn enqueue(&mut self, now: Instant, bytes: u64) -> Instant {
        let start = self.busy_until.max(now);
        self.busy_until = start + self.transmit_time(bytes);
        self.busy_until + self.base
    }
}

/// The Wi-Fi between the two machines, in one direction. Cheap to clone.
#[derive(Debug, Clone)]
pub struct SimWifi {
    air: Arc<Mutex<Air>>,
}

impl SimWifi {
    /// `capacity` in bytes per second, `base` propagation and a queue of `buffer` bytes.
    pub fn new(capacity: u64, base: Duration, buffer: u64) -> Self {
        SimWifi {
            air: Arc::new(Mutex::new(Air {
                capacity: capacity.max(1),
                base,
                buffer: buffer.max(PACKET),
                busy_until: Instant::now(),
                data: Vec::new(),
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Air> {
        self.air.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The Wi-Fi gets worse or better (interference, another machine downloading). What is already
    /// in the queue leaves at the new capacity.
    pub fn set_capacity(&self, capacity: u64) {
        let now = Instant::now();
        let mut air = self.lock();
        let backlog = air.backlog(now);
        air.capacity = capacity.max(1);
        air.busy_until = now + air.transmit_time(backlog);
        tracing::info!(capacity, "Wi-Fi capacity (simulated)");
    }

    pub fn capacity(&self) -> u64 {
        self.lock().capacity
    }

    /// An input message of `bytes`: enters the queue now and returns when it arrives.
    pub fn send_input(&self, bytes: usize) -> Instant {
        let now = Instant::now();
        self.lock().enqueue(now, bytes as u64)
    }

    /// For writing data: waits until there is room in the queue.
    pub fn data_writer(&self) -> WifiWriter {
        WifiWriter {
            wifi: self.clone(),
            wait: None,
            deliver: None,
        }
    }

    /// The two ends of a data stream over the Wi-Fi. The reader sees the end when the
    /// writer closes (or is dropped) and everything before it arrived.
    pub fn data_pipe(&self) -> (WifiWriter, WifiReader) {
        let (tx, rx) = mpsc::unbounded_channel();
        let writer = WifiWriter {
            wifi: self.clone(),
            wait: None,
            deliver: Some(tx),
        };
        let reader = WifiReader {
            arrivals: rx,
            current: None,
            wait: None,
        };
        (writer, reader)
    }

    /// The data that left (entered the queue): when and how many bytes.
    pub fn data_sent(&self) -> Vec<(Instant, usize)> {
        self.lock().data.clone()
    }
}

/// Data write through the [`SimWifi`].
pub struct WifiWriter {
    wifi: SimWifi,
    wait: Option<Pin<Box<Sleep>>>,
    /// At the other end, if any ([`SimWifi::data_pipe`]): when each stretch arrives.
    deliver: Option<mpsc::UnboundedSender<(Instant, usize)>>,
}

/// The end that receives from [`SimWifi::data_pipe`].
pub struct WifiReader {
    arrivals: mpsc::UnboundedReceiver<(Instant, usize)>,
    /// The stretch being read: when it arrives and how much is left.
    current: Option<(Instant, usize)>,
    wait: Option<Pin<Box<Sleep>>>,
}

impl AsyncRead for WifiReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let (at, left) = match self.current {
                Some(c) => c,
                None => match self.arrivals.poll_recv(cx) {
                    Poll::Ready(Some(c)) => {
                        self.current = Some(c);
                        c
                    }
                    // Without a writer: the end.
                    Poll::Ready(None) => return Poll::Ready(Ok(())),
                    Poll::Pending => return Poll::Pending,
                },
            };
            if Instant::now() < at {
                let wait = self
                    .wait
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep_until(at)));
                if wait.as_mut().poll(cx).is_pending() {
                    return Poll::Pending;
                }
            }
            self.wait = None;
            let n = left.min(buf.remaining());
            if n == 0 {
                self.current = None;
                continue;
            }
            buf.put_slice(&vec![0u8; n]);
            self.current = (left > n).then_some((at, left - n));
            return Poll::Ready(Ok(()));
        }
    }
}

impl AsyncWrite for WifiWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            if let Some(wait) = self.wait.as_mut() {
                if wait.as_mut().poll(cx).is_pending() {
                    return Poll::Pending;
                }
                self.wait = None;
            }
            let now = Instant::now();
            let mut air = self.wifi.lock();
            let backlog = air.backlog(now);
            let room = air.buffer.saturating_sub(backlog);
            let want = buf.len() as u64;
            let need = want.min(PACKET);
            if room >= need {
                let n = want.min(room);
                let arrives = air.enqueue(now, n);
                let n = usize::try_from(n).unwrap_or(buf.len());
                air.data.push((now, n));
                if let Some(d) = &self.deliver {
                    let _ = d.send((arrives, n));
                }
                return Poll::Ready(Ok(n));
            }
            // Until what is missing for a packet to fit leaves.
            let until = now + air.transmit_time(need - room);
            drop(air);
            self.wait = Some(Box::pin(tokio::time::sleep_until(until)));
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        // The reader sees the end after what is already on the way.
        self.deliver = None;
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;

    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_queue_input_takes_what_the_air_takes() {
        let wifi = SimWifi::new(1_000_000, ms(2), 100_000);
        let t0 = Instant::now();
        // 1000 bytes at 1 MB/s: 1 ms, plus 2 ms of propagation.
        assert_eq!(wifi.send_input(1000), t0 + ms(3));
    }

    #[tokio::test(start_paused = true)]
    async fn data_fills_the_queue_and_input_waits_behind() {
        let wifi = SimWifi::new(1_000_000, ms(2), 100_000);
        let mut w = wifi.data_writer();
        let t0 = Instant::now();
        w.write_all(&[0; 100_000]).await.unwrap();
        // Everything entered without waiting: the queue has room for 100 kB.
        assert_eq!(Instant::now(), t0);
        assert_eq!(wifi.send_input(1000), t0 + ms(103));
        // What does not fit waits until it drains.
        w.write_all(&[0; 50_000]).await.unwrap();
        assert!(Instant::now() >= t0 + ms(49), "{:?}", Instant::now() - t0);
        let sent: usize = wifi.data_sent().iter().map(|(_, n)| n).sum();
        assert_eq!(sent, 150_000);
    }

    #[tokio::test(start_paused = true)]
    async fn what_is_written_reaches_the_other_end_on_time() {
        use tokio::io::AsyncReadExt;

        let wifi = SimWifi::new(1_000_000, ms(2), 100_000);
        let (mut w, mut r) = wifi.data_pipe();
        let t0 = Instant::now();
        w.write_all(&[7; 10_000]).await.unwrap();
        w.shutdown().await.unwrap();
        let mut got = Vec::new();
        r.read_to_end(&mut got).await.unwrap();
        // 10 kB at 1 MB/s is 10 ms, plus 2 ms of propagation.
        assert_eq!(got.len(), 10_000);
        assert_eq!(Instant::now() - t0, ms(12));
    }

    #[tokio::test(start_paused = true)]
    async fn lowering_the_capacity_lengthens_the_queue() {
        let wifi = SimWifi::new(1_000_000, Duration::ZERO, 100_000);
        let mut w = wifi.data_writer();
        w.write_all(&[0; 10_000]).await.unwrap();
        let t0 = Instant::now();
        wifi.set_capacity(500_000);
        assert_eq!(wifi.capacity(), 500_000);
        // 10 kB at 500 kB/s is 20 ms, and the key (500 B) 1 ms more.
        assert_eq!(wifi.send_input(500), t0 + ms(21));
    }
}
