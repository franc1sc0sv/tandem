//! The live cursor (T-128): position and edge crossings for the UI, at no cost when
//! nobody is watching.
//!
//! - [`CursorGate`]: an atomic switch that the agent turns on while there is any
//!   subscription (this machine's UI or the other's). The drivers read it on the hot
//!   path: off, the cost per event is one relaxed read and nothing more.
//! - [`Limiter`]: lets through at most [`MAX_HZ`] samples per second. What is held back
//!   is not lost: it stays pending and goes out as soon as it is due, so that the UI dot does not
//!   stay a few pixels short of where the mouse stopped.
//! - [`CursorFeed`]: what each driver carries inside (the machine, the limit and where it goes out).
//!
//! Samples are published by the machine that has the focus: the server while the cursor
//! is on its screen, the client while it is on its own. Crossings are always published
//! by the server, which is the one that decides them.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tandem_core::Machine;
use tandem_proto::CursorSample;
use tandem_proto::cursor::MAX_HZ;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::geometry::{Point, Side};

/// Minimum separation between two samples. Rounded up (not down) so that
/// in any second 30 fit and not 31.
pub const MIN_INTERVAL: Duration = Duration::from_micros(1_000_000_u64.div_ceil(MAX_HZ as u64));

/// What is published: a sample or a crossing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorEvent {
    Sample(CursorSample),
    Crossed {
        from: Machine,
        to: Machine,
        /// The edge of `from` it left through.
        edge: Side,
    },
}

/// An edge crossing, as the server's session records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crossing {
    pub from: Machine,
    pub to: Machine,
    pub edge: Side,
}

/// Live-cursor switch, shared between the agent and the drivers.
#[derive(Debug, Default)]
pub struct CursorGate(AtomicBool);

impl CursorGate {
    pub fn set(&self, on: bool) {
        self.0.store(on, Ordering::Relaxed);
    }

    /// Whether anyone is watching. It is the only thing the hot path pays without subscriptions.
    #[inline]
    pub fn is_on(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Where samples and crossings go out, and the switch that says whether anyone is watching.
#[derive(Debug, Clone)]
pub struct CursorTap {
    gate: Arc<CursorGate>,
    tx: mpsc::UnboundedSender<CursorEvent>,
}

impl CursorTap {
    /// The tap and the receiver of what the drivers publish. The channel has no cap because
    /// the [`MAX_HZ`] limit already bounds it; whoever reads it is an agent task.
    pub fn new(gate: Arc<CursorGate>) -> (Self, mpsc::UnboundedReceiver<CursorEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (CursorTap { gate, tx }, rx)
    }
}

/// Where the cursor is, without the screen: what is cheap to compare and to store while
/// waiting for the turn to publish it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spot {
    pub machine: Machine,
    pub x: i32,
    pub y: i32,
}

impl Spot {
    pub fn new(machine: Machine, p: Point) -> Self {
        // `as` saturates to the i32 range, which is plenty for a screen point.
        Spot {
            machine,
            x: p.x.round() as i32,
            y: p.y.round() as i32,
        }
    }
}

/// Limits the frequency of what is published. It works with the time it is given (in
/// microseconds from any origin), to test it without a clock.
///
/// It guarantees that two outputs are at least `min_us` apart, that what is repeated does not go out and that what
/// was held back goes out later with the newest value.
#[derive(Debug, Clone)]
pub struct Limiter<T> {
    min_us: u64,
    last_at: Option<u64>,
    sent: Option<T>,
    pending: Option<T>,
}

impl<T: Copy + PartialEq> Limiter<T> {
    pub fn new(min_us: u64) -> Self {
        Limiter {
            min_us,
            last_at: None,
            sent: None,
            pending: None,
        }
    }

    fn ready(&self, now_us: u64) -> bool {
        self.last_at
            .is_none_or(|last| now_us.saturating_sub(last) >= self.min_us)
    }

    fn emit(&mut self, now_us: u64, value: T) -> Option<T> {
        self.last_at = Some(now_us);
        self.sent = Some(value);
        self.pending = None;
        Some(value)
    }

    /// A new value: it goes out now if it is due and is not the one that went out; otherwise, it stays pending.
    pub fn offer(&mut self, now_us: u64, value: T) -> Option<T> {
        if self.sent == Some(value) {
            self.pending = None;
            return None;
        }
        if self.ready(now_us) {
            return self.emit(now_us, value);
        }
        self.pending = Some(value);
        None
    }

    /// When what is pending can go out; `None` if nothing is pending.
    pub fn due_at(&self) -> Option<u64> {
        let last = self.last_at?;
        self.pending.map(|_| last.saturating_add(self.min_us))
    }

    /// Takes out what is pending if it is due.
    pub fn flush(&mut self, now_us: u64) -> Option<T> {
        let value = self.pending?;
        if !self.ready(now_us) {
            return None;
        }
        self.emit(now_us, value)
    }

    /// Forgets only what is pending.
    pub fn drop_pending(&mut self) {
        self.pending = None;
    }

    /// Forgets what is pending and the last one sent: what came before is no use to whoever looks again
    /// (nor to whoever is looking now, if the switch was turned off).
    pub fn reset(&mut self) {
        self.sent = None;
        self.pending = None;
    }
}

/// What a driver carries to publish the cursor.
#[derive(Debug)]
pub struct CursorFeed {
    tap: CursorTap,
    /// This driver's machine: the samples it publishes are of its cursor.
    machine: Machine,
    limiter: Limiter<Spot>,
    epoch: Instant,
    on: bool,
}

impl CursorFeed {
    pub fn new(tap: CursorTap, machine: Machine) -> Self {
        CursorFeed {
            tap,
            machine,
            limiter: Limiter::new(MIN_INTERVAL.as_micros() as u64),
            epoch: Instant::now(),
            on: false,
        }
    }

    pub fn machine(&self) -> Machine {
        self.machine
    }

    fn now_us(&self) -> u64 {
        Instant::now()
            .saturating_duration_since(self.epoch)
            .as_micros() as u64
    }

    /// Is anyone watching? It is the hot-path check. When it turns off, what is
    /// pending is discarded.
    #[inline]
    pub fn active(&mut self) -> bool {
        let on = self.tap.gate.is_on();
        if on != self.on {
            self.on = on;
            if !on {
                self.limiter.reset();
            }
        }
        on
    }

    /// The cursor is at `p` (of this driver's machine). `screen` says which screen
    /// it falls on, and is only called if the sample goes out.
    pub fn observe(&mut self, p: Point, screen: impl FnOnce(Point) -> String) {
        let spot = Spot::new(self.machine, p);
        let now = self.now_us();
        if let Some(spot) = self.limiter.offer(now, spot) {
            self.send_sample(spot, screen);
        }
    }

    /// Discards the sample that was held back: the focus left and it is no longer this machine's.
    pub fn discard(&mut self) {
        self.limiter.drop_pending();
    }

    /// When the sample that was held back can go out.
    pub fn next_due(&self) -> Option<Instant> {
        self.limiter
            .due_at()
            .map(|us| self.epoch + Duration::from_micros(us))
    }

    /// Takes out the held-back sample if it is due.
    pub fn flush(&mut self, screen: impl FnOnce(Point) -> String) {
        let now = self.now_us();
        if let Some(spot) = self.limiter.flush(now) {
            self.send_sample(spot, screen);
        }
    }

    fn send_sample(&self, spot: Spot, screen: impl FnOnce(Point) -> String) {
        let screen = screen(Point::new(f64::from(spot.x), f64::from(spot.y)));
        // Without a receiver the agent is already gone: there is nobody to notify.
        let _ = self.tap.tx.send(CursorEvent::Sample(CursorSample {
            machine: spot.machine,
            screen,
            x: spot.x,
            y: spot.y,
        }));
    }

    /// A crossing: it is not limited, it is rare and the UI cannot lose it.
    pub fn crossed(&self, c: Crossing) {
        let _ = self.tap.tx.send(CursorEvent::Crossed {
            from: c.from,
            to: c.to,
            edge: c.edge,
        });
    }
}

/// Waits until `due`; with no date, never. For the drivers' `select!`.
pub(super) async fn sleep_until_opt(due: Option<Instant>) {
    match due {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const MIN: u64 = 33_334;

    fn limiter() -> Limiter<u32> {
        Limiter::new(MIN)
    }

    #[test]
    fn the_minimum_interval_fits_thirty_samples_a_second() {
        assert_eq!(MIN_INTERVAL, Duration::from_micros(MIN));
        // With 33,333 µs 31 would fit in a second.
        assert!(30 * MIN_INTERVAL.as_micros() >= 1_000_000);
    }

    #[test]
    fn the_first_value_goes_out_at_once_and_a_close_one_waits() {
        let mut l = limiter();
        assert_eq!(l.offer(0, 1), Some(1));
        assert_eq!(l.offer(10, 2), None);
        assert_eq!(l.due_at(), Some(MIN));
        assert_eq!(l.flush(MIN - 1), None);
        assert_eq!(l.flush(MIN), Some(2));
        assert_eq!(l.due_at(), None);
        assert_eq!(l.flush(10 * MIN), None);
    }

    #[test]
    fn a_waiting_value_is_replaced_by_the_newest() {
        let mut l = limiter();
        l.offer(0, 1);
        assert_eq!(l.offer(5, 2), None);
        assert_eq!(l.offer(9, 3), None);
        assert_eq!(l.flush(MIN), Some(3));
    }

    #[test]
    fn what_was_already_sent_is_not_repeated() {
        let mut l = limiter();
        assert_eq!(l.offer(0, 1), Some(1));
        assert_eq!(l.offer(MIN, 1), None);
        assert_eq!(l.offer(2 * MIN, 1), None);
        // Going back to the previous one while another waits cancels the wait.
        assert_eq!(l.offer(2 * MIN + 1, 2), Some(2));
        assert_eq!(l.offer(2 * MIN + 2, 3), None);
        assert_eq!(l.offer(2 * MIN + 3, 2), None);
        assert_eq!(l.flush(10 * MIN), None);
    }

    #[test]
    fn resetting_forgets_the_pending_value_and_the_last_one_sent() {
        let mut l = limiter();
        l.offer(0, 1);
        l.offer(1, 2);
        l.reset();
        assert_eq!(l.due_at(), None);
        // Whoever looks again wants to see where the cursor is even if it has not moved,
        // but without skipping the interval.
        assert_eq!(l.offer(2, 1), None);
        assert_eq!(l.flush(MIN), Some(1));
    }

    #[derive(Debug, Clone)]
    enum Step {
        Offer(u32),
        Flush,
    }

    fn steps() -> impl Strategy<Value = Vec<(u64, Step)>> {
        // Time jumps from 0 to 100 ms, with many more small ones than large: the
        // mouse at 1000 Hz and pauses.
        let gap = prop_oneof![8 => 0u64..2_000, 2 => 0u64..100_000];
        let step = prop_oneof![
            3 => (0u32..50).prop_map(Step::Offer),
            1 => Just(Step::Flush),
        ];
        prop::collection::vec((gap, step), 0..400).prop_map(|v| {
            let mut t = 0;
            v.into_iter()
                .map(|(g, s)| {
                    t += g;
                    (t, s)
                })
                .collect()
        })
    }

    proptest! {
        /// The T-128 invariant: never more than 30 outputs in any second, whatever
        /// happens with the offers and the clock notices.
        #[test]
        fn never_more_than_thirty_a_second(steps in steps()) {
            let mut l = limiter();
            let mut out = Vec::new();
            for (t, step) in steps {
                let got = match step {
                    Step::Offer(v) => l.offer(t, v),
                    Step::Flush => l.flush(t),
                };
                if got.is_some() {
                    out.push(t);
                }
            }
            for w in out.windows(31) {
                prop_assert!(w[30] - w[0] >= 1_000_000, "31 outputs in {} µs", w[30] - w[0]);
            }
            for w in out.windows(2) {
                prop_assert!(w[1] - w[0] >= MIN);
            }
        }

        /// What was offered last always goes out (at most one interval later): the UI
        /// dot ends up where the mouse stopped.
        #[test]
        fn the_last_value_always_gets_out(steps in steps(), last in 100u32..200) {
            let mut l = limiter();
            let mut now = 0;
            for (t, step) in steps {
                now = t;
                match step {
                    Step::Offer(v) => {
                        l.offer(t, v);
                    }
                    Step::Flush => {
                        l.flush(t);
                    }
                }
            }
            let got = l.offer(now, last);
            let got = got.or_else(|| l.due_at().and_then(|due| l.flush(due)));
            prop_assert_eq!(got, Some(last));
        }
    }

    #[test]
    fn a_spot_rounds_to_whole_pixels() {
        let s = Spot::new(Machine::A, Point::new(10.4, -0.6));
        assert_eq!((s.x, s.y), (10, -1));
        let s = Spot::new(Machine::B, Point::new(f64::MAX, f64::MIN));
        assert_eq!((s.x, s.y), (i32::MAX, i32::MIN));
    }

    #[tokio::test(start_paused = true)]
    async fn a_feed_publishes_only_while_the_gate_is_on_and_at_most_thirty_hertz() {
        let gate = Arc::new(CursorGate::default());
        let (tap, mut rx) = CursorTap::new(gate.clone());
        let mut feed = CursorFeed::new(tap, Machine::B);
        let screen = |_: Point| "b-1".to_owned();

        assert!(!feed.active());
        // The driver does not call `observe` with the switch off; if it did by
        // mistake, `active` would still return false and nothing is published.
        assert!(rx.try_recv().is_err());

        gate.set(true);
        assert!(feed.active());
        feed.observe(Point::new(1.0, 2.0), screen);
        feed.observe(Point::new(3.0, 4.0), screen);
        assert_eq!(
            rx.try_recv(),
            Ok(CursorEvent::Sample(CursorSample {
                machine: Machine::B,
                screen: "b-1".into(),
                x: 1,
                y: 2,
            }))
        );
        assert!(rx.try_recv().is_err(), "the second waits its turn");
        let due = feed.next_due().expect("there is a held-back sample");
        tokio::time::sleep_until(due).await;
        feed.flush(screen);
        assert!(matches!(
            rx.try_recv(),
            Ok(CursorEvent::Sample(CursorSample { x: 3, y: 4, .. }))
        ));

        // When it turns off, what is pending is discarded.
        feed.observe(Point::new(5.0, 6.0), screen);
        gate.set(false);
        assert!(!feed.active());
        assert_eq!(feed.next_due(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_crossing_is_never_held_back() {
        let gate = Arc::new(CursorGate::default());
        gate.set(true);
        let (tap, mut rx) = CursorTap::new(gate);
        let feed = CursorFeed::new(tap, Machine::A);
        let c = Crossing {
            from: Machine::A,
            to: Machine::B,
            edge: Side::Right,
        };
        feed.crossed(c);
        feed.crossed(c);
        for _ in 0..2 {
            assert_eq!(
                rx.try_recv(),
                Ok(CursorEvent::Crossed {
                    from: Machine::A,
                    to: Machine::B,
                    edge: Side::Right,
                })
            );
        }
    }
}
