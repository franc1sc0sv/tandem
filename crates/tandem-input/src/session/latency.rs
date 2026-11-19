//! Measurement of input latency (T-093, DESIGN §7.1).
//!
//! The server stamps each motion datagram and each key or click with its monotonic
//! clock ([`now_us`]). The client converts that stamp to its own clock with the offset
//! estimated by the heartbeats ([`Beats`], like NTP: round trip / 2, keeping the
//! sample with the smallest round trip) and stores the difference with the moment of injecting in
//! a [`Histogram`]. Every 5 s it sends p50/p99/max to the server and they are published.

use std::collections::VecDeque;
use std::sync::OnceLock;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tandem_proto::InputLatency;

/// Monotonic clock of the process, in microseconds since the first reading. Each
/// machine has its own: between the two it is only valid with the [`ClockSync`] offset.
pub fn now_us() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    let start = *START.get_or_init(Instant::now);
    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// An event as it comes out of the capture, with the [`now_us`] stamp set on the thread
/// that captured it (the hook's or the tap's): the published latency also counts what it
/// waits in the channel until the driver processes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Captured {
    pub t_us: u64,
    pub event: crate::event::InputEvent,
}

impl Captured {
    /// `event`, captured now.
    pub fn now(event: crate::event::InputEvent) -> Self {
        Self {
            t_us: now_us(),
            event,
        }
    }
}

/// Mantissa bits: 32 buckets per power of two, relative error ≤ 1/32.
const SUB_BITS: u32 = 5;
const SUB: u64 = 1 << SUB_BITS;
/// Up to `2 * SUB` each value has its own bucket.
const LINEAR: u64 = 2 * SUB;
/// What fits in `u32` microseconds (more than an hour) is plenty.
const BUCKETS: usize = (LINEAR + (32 - SUB_BITS as u64 - 1) * SUB) as usize;

/// Log-linear histogram of microseconds: fixed memory and recording is O(1), so it
/// does not weigh on the hot path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histogram {
    buckets: Vec<u32>,
    count: u32,
    max: u32,
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

impl Histogram {
    pub fn new() -> Self {
        Self {
            buckets: vec![0; BUCKETS],
            count: 0,
            max: 0,
        }
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn record(&mut self, us: u64) {
        let v = u32::try_from(us).unwrap_or(u32::MAX);
        let i = index(v);
        self.buckets[i] = self.buckets[i].saturating_add(1);
        self.count = self.count.saturating_add(1);
        self.max = self.max.max(v);
    }

    /// The value below which the fraction `q` of what was recorded lies (from above: the
    /// upper bound of its bucket, without exceeding the maximum). `None` without data.
    pub fn quantile(&self, q: f64) -> Option<u32> {
        if self.count == 0 {
            return None;
        }
        let q = q.clamp(0.0, 1.0);
        // 1-based rank of the sought value.
        let rank = ((q * f64::from(self.count)).ceil() as u64).max(1);
        let mut seen = 0u64;
        for (i, n) in self.buckets.iter().enumerate() {
            seen += u64::from(*n);
            if seen >= rank {
                return Some(upper(i).min(self.max));
            }
        }
        Some(self.max)
    }

    /// p50/p99/max of what was recorded; `None` without data.
    pub fn stats(&self) -> Option<InputLatency> {
        Some(InputLatency {
            link: None,
            count: self.count,
            p50_us: self.quantile(0.50)?,
            p99_us: self.quantile(0.99)?,
            max_us: self.max,
        })
    }

    /// The statistics of the window and a new window.
    pub fn take(&mut self) -> Option<InputLatency> {
        let s = self.stats();
        *self = Self::new();
        s
    }
}

fn index(v: u32) -> usize {
    let v = u64::from(v);
    if v < LINEAR {
        return v as usize;
    }
    let exp = 63 - u64::from(v.leading_zeros());
    let top = v >> (exp - u64::from(SUB_BITS));
    (LINEAR + (exp - u64::from(SUB_BITS) - 1) * SUB + (top - SUB)) as usize
}

/// Greatest value that falls in bucket `i`.
fn upper(i: usize) -> u32 {
    let i = i as u64;
    if i < LINEAR {
        return i as u32;
    }
    let k = i - LINEAR;
    let shift = k / SUB + 1;
    let top = SUB + k % SUB;
    let v = ((top + 1) << shift) - 1;
    u32::try_from(v).unwrap_or(u32::MAX)
}

/// The echo of the last heartbeat received: when the other sent it (on its clock) and how long
/// the answering side held it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Echo {
    pub sent_us: u64,
    pub held_us: u64,
}

/// Heartbeat with a timestamp; `echo` returns the last one from the other side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Beat {
    pub sent_us: u64,
    pub echo: Option<Echo>,
}

/// A round-trip sample: the round trip without what was held and the offset
/// (the other's clock − own clock) assuming symmetric round trips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sample {
    rtt_us: u64,
    offset_us: i64,
}

/// Estimation of the offset with the other side's clock. It keeps the sample with the smallest
/// round trip of the last [`ClockSync::WINDOW`]: the shortest one is the one that
/// can have the least asymmetry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClockSync {
    samples: VecDeque<Sample>,
}

impl ClockSync {
    pub const WINDOW: usize = 16;

    /// `t0`: when we sent the heartbeat (our clock); `t2`: when the other sent
    /// theirs with the echo (their clock); `held`: how long it held it; `t3`: when it arrived (ours).
    pub fn sample(&mut self, t0: u64, t2: u64, held: u64, t3: u64) {
        let Some(rtt) = t3.checked_sub(t0).and_then(|r| r.checked_sub(held)) else {
            return;
        };
        // t1 = t2 − held is when ours reached the other.
        let (t0, t2, held, t3) = (t0 as i128, t2 as i128, held as i128, t3 as i128);
        let offset = ((t2 - held - t0) + (t2 - t3)) / 2;
        let Ok(offset_us) = i64::try_from(offset) else {
            return;
        };
        if self.samples.len() == Self::WINDOW {
            self.samples.pop_front();
        }
        self.samples.push_back(Sample {
            rtt_us: rtt,
            offset_us,
        });
    }

    /// The other's clock − own clock, if there is already a sample.
    pub fn offset_us(&self) -> Option<i64> {
        self.best().map(|s| s.offset_us)
    }

    /// The smallest round trip in the window.
    pub fn rtt_us(&self) -> Option<u64> {
        self.best().map(|s| s.rtt_us)
    }

    fn best(&self) -> Option<&Sample> {
        self.samples.iter().min_by_key(|s| s.rtt_us)
    }

    /// How much time passed from the other's stamp `peer_us` until our `now_us`.
    pub fn elapsed_since(&self, peer_us: u64, now_us: u64) -> Option<u64> {
        let offset = self.offset_us()?;
        let local = i128::from(peer_us) - i128::from(offset);
        // An estimation error can give something negative: it is zero.
        let d = (i128::from(now_us) - local).max(0);
        Some(u64::try_from(d).unwrap_or(u64::MAX))
    }
}

/// The heartbeats of one side: builds the outgoing ones (with the echo of the last that arrived) and extracts
/// offset samples from the incoming ones.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Beats {
    /// The other's last heartbeat: when it sent it (its clock) and when it arrived (ours).
    last_rx: Option<(u64, u64)>,
    sync: ClockSync,
}

impl Beats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn outgoing(&self, now: u64) -> Beat {
        Beat {
            sent_us: now,
            echo: self.last_rx.map(|(sent, at)| Echo {
                sent_us: sent,
                held_us: now.saturating_sub(at),
            }),
        }
    }

    pub fn incoming(&mut self, beat: Beat, now: u64) {
        if let Some(e) = beat.echo {
            self.sync.sample(e.sent_us, beat.sent_us, e.held_us, now);
        }
        self.last_rx = Some((beat.sent_us, now));
    }

    pub fn sync(&self) -> &ClockSync {
        &self.sync
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn without_data_there_are_no_statistics() {
        let mut h = Histogram::new();
        assert_eq!(h.stats(), None);
        assert_eq!(h.take(), None);
        assert_eq!(h.quantile(0.5), None);
    }

    #[test]
    fn percentiles_of_a_known_distribution() {
        let mut h = Histogram::new();
        for v in 1..=100u64 {
            h.record(v);
        }
        let s = h.take().unwrap();
        assert_eq!(s.count, 100);
        assert_eq!(s.max_us, 100);
        // Below 64 each value is exact; above, within 1/32.
        assert_eq!(s.p50_us, 50);
        assert!((99..=102).contains(&s.p99_us), "{}", s.p99_us);
        assert_eq!(h.count(), 0, "taking opens a new window");
    }

    #[test]
    fn valores_enormes_no_rompen_nada() {
        let mut h = Histogram::new();
        h.record(u64::MAX);
        h.record(0);
        let s = h.stats().unwrap();
        assert_eq!(s.max_us, u32::MAX);
        assert_eq!(h.quantile(1.0), Some(u32::MAX));
        assert_eq!(h.quantile(0.0), Some(0));
    }

    #[test]
    fn the_buckets_cover_all_of_u32_in_order() {
        let mut values: Vec<u32> = (0..32)
            .flat_map(|shift| [1u32 << shift, (1u32 << shift).saturating_add(7)])
            .collect();
        values.sort_unstable();
        let mut last = 0;
        for v in values {
            let i = index(v);
            assert!(i < BUCKETS, "{v} → {i}");
            assert!(i >= last, "{v} → {i} < {last}");
            assert!(upper(i) >= v);
            assert!(
                i == 0 || upper(i - 1) < v,
                "{v} fits in the previous bucket"
            );
            last = i;
        }
        assert_eq!(index(u32::MAX), BUCKETS - 1);
        assert_eq!(upper(BUCKETS - 1), u32::MAX);
    }

    #[test]
    fn offset_with_symmetric_round_trip() {
        // The other clock is 1 s ahead; 400 µs out, 400 back, 10 ms held.
        let mut c = ClockSync::default();
        assert_eq!(c.offset_us(), None);
        let (t0, t1) = (5_000u64, 1_005_400u64);
        let t2 = t1 + 10_000;
        let t3 = t0 + 400 + 10_000 + 400;
        c.sample(t0, t2, 10_000, t3);
        assert_eq!(c.offset_us(), Some(1_000_000));
        assert_eq!(c.rtt_us(), Some(800));
        // A stamp from the other from 2 ms ago, seen from here.
        assert_eq!(c.elapsed_since(1_100_000, 102_000), Some(2_000));
        // An estimate that gives negative is zero.
        assert_eq!(c.elapsed_since(1_100_000, 99_000), Some(0));
    }

    #[test]
    fn the_sample_with_the_smallest_round_trip_wins() {
        let mut c = ClockSync::default();
        // Slow outbound (queue): 5 ms out and 100 µs back, real offset 0.
        c.sample(0, 5_000, 0, 5_100);
        assert_eq!(c.offset_us(), Some(2_450));
        // A clean sample wins.
        c.sample(10_000, 10_050, 0, 10_100);
        assert_eq!(c.offset_us(), Some(0));
        // Impossible samples (clocks going backwards) are discarded.
        c.sample(10, 0, 50, 20);
        assert_eq!(c.rtt_us(), Some(100));
    }

    #[test]
    fn heartbeats_with_echo_estimate_the_offset() {
        // A and B with clocks 7 ms apart; 300 µs each way.
        let mut a = Beats::new();
        let mut b = Beats::new();
        let skew = 7_000u64;
        let ab = a.outgoing(1_000);
        assert_eq!(ab.echo, None);
        b.incoming(ab, 1_300 + skew);
        // B answers 1 s later, with the echo.
        let ba = b.outgoing(1_001_300 + skew);
        assert_eq!(
            ba.echo,
            Some(Echo {
                sent_us: 1_000,
                held_us: 1_000_000
            })
        );
        a.incoming(ba, 1_001_600);
        assert_eq!(a.sync().offset_us(), Some(7_000));
        assert_eq!(a.sync().rtt_us(), Some(600));
        assert_eq!(b.sync().offset_us(), None, "B has not yet received an echo");
    }

    proptest! {
        /// The reported percentile is never below the real one and exceeds it at most
        /// by 1/32 (plus one microsecond of rounding); the maximum is exact.
        #[test]
        fn the_percentile_bounds_the_real_one(
            mut values in prop::collection::vec(0u64..5_000_000, 1..400),
            q in 0.0f64..=1.0,
        ) {
            let mut h = Histogram::new();
            for v in &values {
                h.record(*v);
            }
            values.sort_unstable();
            let rank = ((q * values.len() as f64).ceil() as usize).max(1);
            let real = values[rank - 1] as f64;
            let got = f64::from(h.quantile(q).unwrap());
            prop_assert!(got >= real, "{got} < {real}");
            prop_assert!(got <= real * (1.0 + 1.0 / 32.0) + 1.0, "{got} ≫ {real}");
            prop_assert_eq!(h.stats().unwrap().max_us as u64, *values.last().unwrap());
        }

        /// The percentiles grow with `q`.
        #[test]
        fn the_percentiles_are_monotonic(
            values in prop::collection::vec(any::<u32>(), 1..200),
            a in 0.0f64..=1.0,
            b in 0.0f64..=1.0,
        ) {
            let mut h = Histogram::new();
            for v in values {
                h.record(u64::from(v));
            }
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            prop_assert!(h.quantile(lo) <= h.quantile(hi));
        }

        /// With symmetric round trips the offset is recovered exactly (within ±1 µs of
        /// rounding), whatever the real offset and what was held.
        #[test]
        fn desfase_simetrico_exacto(
            t0 in 0u64..1_000_000_000,
            skew in -1_000_000_000i64..1_000_000_000,
            one_way in 0u64..50_000,
            held in 0u64..2_000_000,
        ) {
            let t1 = (t0 as i64 + skew + one_way as i64).max(0) as u64;
            prop_assume!(t1 as i64 == t0 as i64 + skew + one_way as i64);
            let t2 = t1 + held;
            let t3 = t0 + one_way + held + one_way;
            let mut c = ClockSync::default();
            c.sample(t0, t2, held, t3);
            let off = c.offset_us().unwrap();
            prop_assert!((off - skew).abs() <= 1, "{off} vs {skew}");
            prop_assert_eq!(c.rtt_us(), Some(2 * one_way));
        }
    }
}
