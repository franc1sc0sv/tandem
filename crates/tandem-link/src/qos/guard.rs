//! Pausing data when input falls behind (DESIGN §6).
//!
//! Pure logic: the input latency samples and the time arrive as arguments.
//! The p99 is computed over a sliding window; if it stays clearly over the target
//! ([`GuardConfig::trip_percent`]) for [`GuardConfig::trip`] in a row, data is paused.
//! It resumes when the p99 has been back within [`GuardConfig::resume_percent`] of the
//! target for [`GuardConfig::hold`], and in any case after [`GuardConfig::max_pause`];
//! after a pause data runs for at least [`GuardConfig::cooldown`] before another.
//!
//! The pause is a short emergency brake, not a state: the input latency it measures runs
//! from capture to injection, so it also carries what the two systems take, which no
//! pause of the tunnels can lower. Before the bounds, a cable whose p99 sat at 4.25 ms
//! against a 4 ms target kept the tunnels stopped for over an hour (2026-11-24). Without
//! input there is nothing to protect: an empty window counts as low latency.

use std::collections::VecDeque;
use std::time::Duration;

use tokio::time::Instant;

/// Timings and thresholds of the pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuardConfig {
    /// The p99 window.
    pub window: Duration,
    /// How long the p99 has to be above the target to pause (T-101: 2 s).
    pub trip: Duration,
    /// How long it has to be below `resume_percent` of the target to resume.
    pub hold: Duration,
    /// Below this % of the target the latency is considered to have dropped.
    pub resume_percent: u32,
    /// Above this % of the target the p99 counts as over it.
    pub trip_percent: u32,
    /// The longest a pause lasts.
    pub max_pause: Duration,
    /// How long data runs after a pause before another one can start.
    pub cooldown: Duration,
}

impl Default for GuardConfig {
    fn default() -> Self {
        GuardConfig {
            window: Duration::from_secs(1),
            trip: Duration::from_secs(2),
            hold: Duration::from_secs(1),
            resume_percent: 100,
            trip_percent: 150,
            max_pause: Duration::from_secs(2),
            cooldown: Duration::from_secs(10),
        }
    }
}

/// A change of state of the pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// The p99 exceeded the target for `trip`.
    Pause { p99: Duration },
    /// The p99 went down again. `p99` is `None` if there was no input in the window.
    Resume { p99: Option<Duration> },
}

#[derive(Debug, Clone)]
pub struct LatencyGuard {
    config: GuardConfig,
    target: Duration,
    samples: VecDeque<(Instant, Duration)>,
    paused: bool,
    /// Since when the p99 has been on the side that leads to changing state.
    since: Option<Instant>,
    /// The p99 of the last evaluation.
    last_p99: Option<Duration>,
    /// When the current pause started.
    paused_at: Option<Instant>,
    /// When the last pause ended: no new one before the cooldown.
    resumed_at: Option<Instant>,
}

impl LatencyGuard {
    pub fn new(config: GuardConfig, target: Duration) -> Self {
        LatencyGuard {
            config,
            target,
            samples: VecDeque::new(),
            paused: false,
            since: None,
            last_p99: None,
            paused_at: None,
            resumed_at: None,
        }
    }

    /// The p99 seen by the last evaluation (`None` with no input in the window).
    pub fn last_p99(&self) -> Option<Duration> {
        self.last_p99
    }

    pub fn target(&self) -> Duration {
        self.target
    }

    pub fn window(&self) -> Duration {
        self.config.window
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Another target (the path changed): what the previous one measured no longer says anything.
    /// Returns [`Transition::Resume`] if it was paused.
    pub fn reset(&mut self, target: Duration) -> Option<Transition> {
        self.target = target;
        self.samples.clear();
        self.since = None;
        self.last_p99 = None;
        self.paused_at = None;
        self.resumed_at = None;
        std::mem::take(&mut self.paused).then_some(Transition::Resume { p99: None })
    }

    /// A measured input latency.
    pub fn record(&mut self, now: Instant, latency: Duration) {
        self.samples.push_back((now, latency));
    }

    /// The p99 of the window ending at `now`; `None` with no samples. It is on the path of
    /// every evaluation (every 20 ms with input): O(n) selection, without sorting.
    pub fn p99(&mut self, now: Instant) -> Option<Duration> {
        self.forget_before(now);
        if self.samples.is_empty() {
            return None;
        }
        let mut lat: Vec<Duration> = self.samples.iter().map(|(_, l)| *l).collect();
        Some(select(&mut lat, 99))
    }

    /// The p50 and p99 of the window ending at `now`, for the Network screen (T-102):
    /// once per second, outside the evaluation.
    pub fn percentiles(&mut self, now: Instant) -> Option<(Duration, Duration)> {
        self.forget_before(now);
        if self.samples.is_empty() {
            return None;
        }
        let mut lat: Vec<Duration> = self.samples.iter().map(|(_, l)| *l).collect();
        Some((select(&mut lat, 50), select(&mut lat, 99)))
    }

    fn forget_before(&mut self, now: Instant) {
        while let Some((t, _)) = self.samples.front() {
            if now.saturating_duration_since(*t) > self.config.window {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// Checks the state with the window ending at `now`.
    pub fn evaluate(&mut self, now: Instant) -> Option<Transition> {
        let p99 = self.p99(now);
        self.last_p99 = p99;
        let elapsed = |t: Option<Instant>| t.map(|t| now.saturating_duration_since(t));
        if self.paused {
            if elapsed(self.paused_at).is_some_and(|d| d >= self.config.max_pause) {
                return Some(self.resume(now, p99));
            }
            let threshold = self.target * self.config.resume_percent / 100;
            if p99.is_some_and(|p| p > threshold) {
                self.since = None;
                return None;
            }
            let since = *self.since.get_or_insert(now);
            if now.saturating_duration_since(since) < self.config.hold {
                return None;
            }
            Some(self.resume(now, p99))
        } else {
            if elapsed(self.resumed_at).is_some_and(|d| d < self.config.cooldown) {
                self.since = None;
                return None;
            }
            let threshold = self.target * self.config.trip_percent / 100;
            let Some(p99) = p99.filter(|p| *p > threshold) else {
                self.since = None;
                return None;
            };
            let since = *self.since.get_or_insert(now);
            if now.saturating_duration_since(since) < self.config.trip {
                return None;
            }
            self.paused = true;
            self.paused_at = Some(now);
            self.since = None;
            Some(Transition::Pause { p99 })
        }
    }

    fn resume(&mut self, now: Instant, p99: Option<Duration>) -> Transition {
        self.paused = false;
        self.paused_at = None;
        self.resumed_at = Some(now);
        self.since = None;
        Transition::Resume { p99 }
    }
}

/// The smallest value that leaves `p` % of the (non-empty) samples at or below it.
fn select(lat: &mut [Duration], p: usize) -> Duration {
    if lat.is_empty() {
        return Duration::ZERO;
    }
    let rank = (lat.len() * p).div_ceil(100).max(1) - 1;
    let (_, v, _) = lat.select_nth_unstable(rank);
    *v
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const TARGET: Duration = Duration::from_millis(20);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// One sample per millisecond with the latency that `lat` says, evaluating every 10 ms.
    fn run(
        g: &mut LatencyGuard,
        t0: Instant,
        from_ms: u64,
        to_ms: u64,
        lat: impl Fn(u64) -> Duration,
    ) -> Vec<(u64, Transition)> {
        let mut out = Vec::new();
        for t in from_ms..to_ms {
            let now = t0 + ms(t);
            g.record(now, lat(t));
            if t % 10 == 0
                && let Some(tr) = g.evaluate(now)
            {
                out.push((t, tr));
            }
        }
        out
    }

    #[test]
    fn the_p99_is_that_of_the_slowest_one_percent() {
        let t0 = Instant::now();
        let mut g = LatencyGuard::new(GuardConfig::default(), TARGET);
        assert_eq!(g.p99(t0), None);
        for i in 0..100u64 {
            g.record(t0, ms(i + 1));
        }
        assert_eq!(g.p99(t0), Some(ms(99)));
        assert_eq!(g.percentiles(t0), Some((ms(50), ms(99))));
        g.record(t0, ms(1000));
        assert_eq!(g.p99(t0), Some(ms(100)));
        // Outside the window it does not count.
        assert_eq!(g.p99(t0 + ms(1001)), None);
    }

    #[test]
    fn pauses_after_2_s_well_above_and_resumes_with_hysteresis() {
        let t0 = Instant::now();
        let mut g = LatencyGuard::new(GuardConfig::default(), TARGET);
        assert!(run(&mut g, t0, 0, 3000, |_| ms(5)).is_empty());
        // Over the target but within the margin (30 ms is 150 % of 20): no pause.
        assert!(run(&mut g, t0, 3000, 8000, |_| ms(30)).is_empty());
        // From 8 s the latency is 40 ms. The p99 goes over the margin when the slow ones
        // are more than 1 % of the window (11 of 1001, at 8010 ms), and 2 s later, pause.
        let out = run(&mut g, t0, 8000, 10_500, |_| ms(40));
        assert_eq!(out, vec![(10_010, Transition::Pause { p99: ms(40) })]);
        assert!(g.is_paused());
        // At 10 ms: the p99 drops when 10 slow ones remain in the window (at 11490 ms), and
        // one more second is needed, which is past the longest pause: it resumes at 2 s.
        let out = run(&mut g, t0, 10_500, 13_000, |_| ms(10));
        assert_eq!(
            out,
            vec![(12_010, Transition::Resume { p99: Some(ms(10)) })]
        );
    }

    #[test]
    fn it_resumes_once_back_within_the_target_for_the_hold() {
        let t0 = Instant::now();
        let config = GuardConfig {
            max_pause: Duration::from_secs(60),
            ..GuardConfig::default()
        };
        let mut g = LatencyGuard::new(config, TARGET);
        let out = run(&mut g, t0, 0, 2100, |_| ms(50));
        assert_eq!(out, vec![(2000, Transition::Pause { p99: ms(50) })]);
        // At 25 ms it is still over the target: it stays paused.
        assert!(run(&mut g, t0, 2100, 6000, |_| ms(25)).is_empty());
        // At 18 ms it is within the target: the window clears at 6990 ms, plus the hold.
        let out = run(&mut g, t0, 6000, 9000, |_| ms(18));
        assert_eq!(out, vec![(7990, Transition::Resume { p99: Some(ms(18)) })]);
    }

    /// The log of 2026-11-24: a cable link (target 4 ms) whose input p99 sat a fraction of
    /// a millisecond over the target paused the tunnels for minutes, even for an hour.
    #[test]
    fn a_p99_a_fraction_over_the_target_does_not_pause() {
        let t0 = Instant::now();
        let mut g = LatencyGuard::new(GuardConfig::default(), ms(4));
        let out = run(&mut g, t0, 0, 30_000, |_| Duration::from_micros(4_250));
        assert!(out.is_empty(), "{out:?}");
        assert!(!g.is_paused());
    }

    /// Input that stays late does not keep data stopped: each pause is short, and after
    /// one data runs for a while before the next.
    #[test]
    fn a_pause_is_short_and_data_runs_between_pauses() {
        let t0 = Instant::now();
        let config = GuardConfig::default();
        let mut g = LatencyGuard::new(config, TARGET);
        let out = run(&mut g, t0, 0, 60_000, |_| ms(100));
        assert!(!out.is_empty());
        let mut paused_ms = 0;
        let mut paused_since = None;
        for (t, tr) in &out {
            match tr {
                Transition::Pause { .. } => {
                    if let Some(prev) = out
                        .iter()
                        .rev()
                        .find(|(at, x)| at < t && matches!(x, Transition::Resume { .. }))
                    {
                        let gap = ms(t - prev.0);
                        assert!(gap >= config.cooldown, "paused again after {gap:?}");
                    }
                    paused_since = Some(*t);
                }
                Transition::Resume { .. } => {
                    let since = paused_since.take().expect("a resume without a pause");
                    let lasted = ms(t - since);
                    assert!(lasted <= config.max_pause + ms(10), "paused {lasted:?}");
                    paused_ms += t - since;
                }
            }
        }
        if let Some(since) = paused_since {
            paused_ms += 60_000 - since;
        }
        assert!(paused_ms * 4 < 60_000, "data paused {paused_ms} ms of 60 s");
    }

    #[test]
    fn a_short_spike_does_not_pause() {
        let t0 = Instant::now();
        let mut g = LatencyGuard::new(GuardConfig::default(), TARGET);
        // 1.5 s bad among good: the p99 rises and falls before 2 s.
        let out = run(&mut g, t0, 0, 6000, |t| {
            if (2000..2500).contains(&t) {
                ms(80)
            } else {
                ms(3)
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn without_input_the_pause_ends_on_its_own() {
        let t0 = Instant::now();
        let mut g = LatencyGuard::new(GuardConfig::default(), TARGET);
        run(&mut g, t0, 0, 2100, |_| ms(50));
        assert!(g.is_paused());
        assert_eq!(g.evaluate(t0 + ms(2500)), None);
        assert_eq!(g.evaluate(t0 + ms(3200)), None);
        assert_eq!(
            g.evaluate(t0 + ms(4200)),
            Some(Transition::Resume { p99: None })
        );
    }

    #[test]
    fn changing_path_resumes_and_forgets_what_was_measured() {
        let t0 = Instant::now();
        let mut g = LatencyGuard::new(GuardConfig::default(), TARGET);
        run(&mut g, t0, 0, 2100, |_| ms(50));
        assert!(g.is_paused());
        assert_eq!(g.reset(ms(4)), Some(Transition::Resume { p99: None }));
        assert_eq!(g.target(), ms(4));
        assert_eq!(g.p99(t0 + ms(2100)), None);
        assert_eq!(g.reset(ms(4)), None);
    }

    proptest! {
        /// With input always within the target, there is never a pause.
        #[test]
        fn within_the_target_never_pauses(lats in prop::collection::vec(0u64..=20_000, 1..3000)) {
            let t0 = Instant::now();
            let mut g = LatencyGuard::new(GuardConfig::default(), TARGET);
            for (i, us) in lats.iter().enumerate() {
                let now = t0 + ms(i as u64 * 3);
                g.record(now, Duration::from_micros(*us));
                prop_assert_eq!(g.evaluate(now), None);
            }
        }

        /// If all input exceeds the target by the margin, the pause arrives between 2 s and
        /// 2 s plus the evaluation step, and only once before the longest pause ends it.
        #[test]
        fn above_it_pauses_at_2_s(
            over in 31u64..500,
            step in 1u64..100,
        ) {
            let t0 = Instant::now();
            let mut g = LatencyGuard::new(GuardConfig::default(), TARGET);
            let mut pauses = Vec::new();
            let mut t = 0;
            while t < 4000 {
                let now = t0 + ms(t);
                g.record(now, ms(over));
                if let Some(tr) = g.evaluate(now) {
                    pauses.push((t, tr));
                }
                t += step;
            }
            prop_assert_eq!(pauses.len(), 1);
            let (at, tr) = pauses[0];
            prop_assert!((2000..2000 + step).contains(&at), "pause at {} ms", at);
            prop_assert_eq!(tr, Transition::Pause { p99: ms(over) });
        }
    }
}
