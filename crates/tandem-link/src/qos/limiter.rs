//! Rate limiter for a connection's data (token bucket).
//!
//! Guarantee: in any 1 s window no more bytes than the cap are delivered. That is why the
//! bucket fills at `cap − burst` per second: what can go out at once (the burst)
//! plus what is replenished in a second never exceeds the cap. The count is integer, in
//! nanobytes, so that rounding does not break the guarantee.

use std::time::Duration;

use tokio::time::Instant;

/// The lowest cap accepted: with less, a frame would take seconds to go out.
pub const MIN_CAP: u64 = 64 * 1024;

/// The largest burst. A burst enters the access point's queue whole, ahead of
/// the next key: 16 KiB is 8 ms on a Wi-Fi that gives 2 MB/s.
pub const MAX_BURST: u64 = 16 * 1024;

/// The least that is delivered if there is not enough for everything asked: a QUIC packet.
pub const MIN_GRANT: u64 = 1200;

const NANOS: u128 = 1_000_000_000;

#[derive(Debug, Clone)]
pub struct RateLimiter {
    cap: u64,
    burst: u64,
    /// Bytes per second that are replenished: `cap - burst`.
    refill: u64,
    /// Tokens in nanobytes (bytes × 10⁹).
    tokens: u128,
    last: Instant,
}

impl RateLimiter {
    /// A limiter with a full bucket. `cap` in bytes per second; below
    /// [`MIN_CAP`] it is [`MIN_CAP`].
    pub fn new(cap: u64, now: Instant) -> Self {
        let mut l = RateLimiter {
            cap: 0,
            burst: 0,
            refill: 0,
            tokens: 0,
            last: now,
        };
        l.set_cap(cap, now);
        l.tokens = u128::from(l.burst) * NANOS;
        l
    }

    /// The cap in bytes per second.
    pub fn cap(&self) -> u64 {
        self.cap
    }

    /// The most that [`Self::take`] delivers at once.
    pub fn burst(&self) -> u64 {
        self.burst
    }

    /// Changes the cap without giving away tokens: those left over from the previous cap are lost.
    pub fn set_cap(&mut self, cap: u64, now: Instant) {
        self.refill_to(now);
        let cap = cap.max(MIN_CAP);
        let burst = (cap / 100).clamp(1, MAX_BURST);
        self.cap = cap;
        self.burst = burst;
        self.refill = cap - burst;
        self.tokens = self.tokens.min(u128::from(burst) * NANOS);
    }

    fn refill_to(&mut self, now: Instant) {
        if now <= self.last {
            return;
        }
        let dt = now.duration_since(self.last).as_nanos();
        let full = u128::from(self.burst) * NANOS;
        self.tokens = self
            .tokens
            .saturating_add(dt.saturating_mul(u128::from(self.refill)))
            .min(full);
        self.last = now;
    }

    /// Asks for up to `want` bytes. `Ok(n)`: `n` may go out now (at most the burst); if there is
    /// not enough for everything but there is for a packet, what there is goes out. `Err(wait)`: how long
    /// until a packet; whoever asks waits and asks again.
    ///
    /// That what there is goes out matters: whoever waits wakes up late (the tokio clock
    /// rounds to the millisecond), and if it waited for the whole burst, what was gathered extra
    /// would overflow the bucket and be lost.
    pub fn take(&mut self, now: Instant, want: usize) -> Result<usize, Duration> {
        self.refill_to(now);
        let n = u64::try_from(want).unwrap_or(u64::MAX).min(self.burst);
        if n == 0 {
            return Ok(0);
        }
        let have = u64::try_from(self.tokens / NANOS).unwrap_or(u64::MAX);
        let least = n.min(MIN_GRANT);
        if have >= least {
            let n = n.min(have);
            self.tokens -= u128::from(n) * NANOS;
            // `n` ≤ `want`, which was already usize.
            return Ok(usize::try_from(n).unwrap_or(want));
        }
        let missing = u128::from(least) * NANOS - self.tokens;
        let ns = missing.div_ceil(u128::from(self.refill));
        Err(Duration::from_nanos(u64::try_from(ns).unwrap_or(u64::MAX)))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const SEC: Duration = Duration::from_secs(1);

    /// The most delivered in a window [t, t + 1 s] that starts at a delivery (deliveries
    /// go in time order).
    fn worst_window(grants: &[(Duration, usize)]) -> u64 {
        let (mut worst, mut sum, mut end) = (0, 0u64, 0);
        for (start, n) in grants {
            while end < grants.len() && grants[end].0 <= *start + SEC {
                sum += grants[end].1 as u64;
                end += 1;
            }
            worst = worst.max(sum);
            sum -= *n as u64;
        }
        worst
    }

    #[test]
    fn the_cap_is_respected_and_used() {
        let t0 = Instant::now();
        let mut l = RateLimiter::new(1_000_000, t0);
        assert_eq!(l.burst(), 10_000);
        let mut now = t0;
        let mut grants = Vec::new();
        while now < t0 + Duration::from_secs(5) {
            match l.take(now, 16 * 1024) {
                Ok(n) => grants.push((now - t0, n)),
                Err(wait) => now += wait,
            }
        }
        let total: u64 = grants.iter().map(|(_, n)| *n as u64).sum();
        assert!(worst_window(&grants) <= 1_000_000);
        // What is replenished per second (99 % of the cap) plus the initial burst.
        assert!(total >= 4 * 990_000, "only {total} came out");
    }

    #[test]
    fn a_low_cap_rises_to_the_minimum() {
        let l = RateLimiter::new(10, Instant::now());
        assert_eq!(l.cap(), MIN_CAP);
        assert!(l.burst() > 0);
    }

    #[test]
    fn lowering_the_cap_leaves_no_extra_tokens() {
        let t0 = Instant::now();
        let mut l = RateLimiter::new(10_000_000, t0);
        l.set_cap(MIN_CAP, t0);
        assert_eq!(l.take(t0, usize::MAX), Ok(l.burst() as usize));
        assert!(l.take(t0, 1).is_err());
    }

    #[test]
    fn if_there_is_not_enough_for_everything_what_there_is_goes_out_from_one_packet() {
        let t0 = Instant::now();
        let mut l = RateLimiter::new(1_000_000, t0);
        assert_eq!(l.take(t0, 10_000), Ok(10_000));
        // 990 kB/s are replenished: in 1 ms, 990 bytes, less than a packet.
        let wait = l.take(t0 + Duration::from_millis(1), 10_000).unwrap_err();
        assert!(wait > Duration::ZERO);
        // In 5 ms there are 4950: those come out, not the 10,000.
        assert_eq!(l.take(t0 + Duration::from_millis(5), 10_000), Ok(4950));
    }

    #[test]
    fn the_wait_is_just_enough() {
        let t0 = Instant::now();
        let mut l = RateLimiter::new(1_000_000, t0);
        assert_eq!(l.take(t0, 10_000), Ok(10_000));
        let wait = l.take(t0, 1000).unwrap_err();
        assert!(l.take(t0 + wait - Duration::from_nanos(1), 1000).is_err());
        assert_eq!(l.take(t0 + wait, 1000), Ok(1000));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Requests of any size at any moment: never more than the cap in a
        /// 1 s window.
        #[test]
        fn never_exceeds_the_cap_in_a_1_s_window(
            cap in MIN_CAP..50_000_000u64,
            steps in prop::collection::vec((0u64..400_000, 1usize..300_000), 1..400),
        ) {
            let t0 = Instant::now();
            let mut l = RateLimiter::new(cap, t0);
            let mut at = Duration::ZERO;
            let mut grants = Vec::new();
            for (dt_us, want) in steps {
                at += Duration::from_micros(dt_us);
                if let Ok(n) = l.take(t0 + at, want) {
                    prop_assert!(n <= want && n as u64 <= l.burst());
                    grants.push((at, n));
                }
            }
            prop_assert!(worst_window(&grants) <= cap);
        }

        /// Whoever waits what the limiter says and asks again does not exceed it either, and
        /// makes use of what is replenished.
        #[test]
        fn whoever_waits_exactly_does_not_exceed_it_and_makes_use_of_it(
            cap in MIN_CAP..20_000_000u64,
            want in 4096usize..200_000,
        ) {
            let t0 = Instant::now();
            let mut l = RateLimiter::new(cap, t0);
            let mut now = t0;
            let mut grants = Vec::new();
            let end = t0 + Duration::from_secs(3);
            while now < end {
                match l.take(now, want) {
                    Ok(n) => grants.push((now - t0, n)),
                    Err(wait) => now += wait,
                }
            }
            prop_assert!(worst_window(&grants) <= cap);
            let total: u64 = grants.iter().map(|(_, n)| *n as u64).sum();
            let refill = cap - l.burst();
            // Three seconds of replenishment, minus what may be left half gathered.
            prop_assert!(total + l.burst() >= 3 * refill, "{} < {}", total, 3 * refill);
        }
    }
}
