//! Injection on its own thread, where the newest position wins (T-358).
//!
//! Injecting is not free: on Windows each `SendInput` waits for every low-level hook of
//! the system (ours among them) and costs 0.3–1.5 ms. A Mac mouse sends up to ~1000
//! movements per second; injected one by one from the network loop, they piled up there
//! and the cursor followed the hand hundreds of milliseconds late.
//!
//! [`ThreadedInjector`] puts what has to be injected in a mailbox and returns at once; a
//! thread of its own injects it. While that thread is busy, a new absolute position
//! replaces the one still waiting (and relative deltas add up): only the newest position
//! goes out, so the wait is never longer than one injection. Keys, buttons, scroll and
//! everything else keep their order, and a position is never moved past them: a click
//! still lands where the cursor was when it was pressed.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::Injector;
use super::latency::Histogram;
use crate::event::InputEvent;

/// How often the injection thread reports its numbers.
const REPORT_EVERY: Duration = Duration::from_secs(5);

/// What the thread has to do, in order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Job {
    Inject(InputEvent),
    Refresh,
    Focus(bool),
}

/// The mailbox: what is waiting, already coalesced.
#[derive(Debug, Default)]
pub struct Mailbox {
    jobs: VecDeque<(Job, Instant)>,
    /// Movements that never went out because a newer one replaced them.
    coalesced: u64,
}

impl Mailbox {
    /// Adds a job. A movement right behind another of the same kind replaces it (absolute)
    /// or adds to it (relative), keeping the stamp of the oldest: what it waited counts.
    pub fn push(&mut self, job: Job, now: Instant) {
        if let Job::Inject(new) = job
            && let Some((Job::Inject(last), _)) = self.jobs.back_mut()
        {
            match (*last, new) {
                (InputEvent::MotionAbsolute { .. }, InputEvent::MotionAbsolute { .. }) => {
                    *last = new;
                    self.coalesced += 1;
                    return;
                }
                (
                    InputEvent::MotionRelative { dx, dy },
                    InputEvent::MotionRelative { dx: ndx, dy: ndy },
                ) => {
                    *last = InputEvent::MotionRelative {
                        dx: dx + ndx,
                        dy: dy + ndy,
                    };
                    self.coalesced += 1;
                    return;
                }
                _ => {}
            }
        }
        self.jobs.push_back((job, now));
    }

    pub fn pop(&mut self) -> Option<(Job, Instant)> {
        self.jobs.pop_front()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn take_coalesced(&mut self) -> u64 {
        std::mem::take(&mut self.coalesced)
    }
}

#[derive(Debug, Default)]
struct State {
    mailbox: Mailbox,
    closing: bool,
}

#[derive(Debug, Default)]
struct Shared {
    state: Mutex<State>,
    ready: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic while injecting must not leave the keyboard dead: the state is still usable.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// An [`Injector`] that injects on its own thread (see the module).
pub struct ThreadedInjector {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl ThreadedInjector {
    /// Starts the thread with `inner`. `name` is the thread's name.
    pub fn spawn<I: Injector + 'static>(inner: I, name: &str) -> std::io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let thread = std::thread::Builder::new().name(name.to_owned()).spawn({
            let shared = Arc::clone(&shared);
            move || run(inner, &shared)
        })?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    fn push(&self, job: Job) {
        self.shared.lock().mailbox.push(job, Instant::now());
        self.shared.ready.notify_one();
    }
}

impl Drop for ThreadedInjector {
    /// Injects what is still waiting (the releases of the end of a session) and stops.
    fn drop(&mut self) {
        self.shared.lock().closing = true;
        self.shared.ready.notify_one();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Injector for ThreadedInjector {
    type Error = Infallible;

    fn inject(&mut self, event: &InputEvent) -> Result<(), Infallible> {
        self.push(Job::Inject(*event));
        Ok(())
    }

    fn refresh(&mut self) -> Result<(), Infallible> {
        self.push(Job::Refresh);
        Ok(())
    }

    fn set_focus(&mut self, here: bool) -> Result<(), Infallible> {
        self.push(Job::Focus(here));
        Ok(())
    }
}

fn run<I: Injector>(mut inner: I, shared: &Shared) {
    raise_priority();
    let mut wait = Histogram::new();
    let mut report_at = Instant::now() + REPORT_EVERY;
    loop {
        let (job, queued) = {
            let mut st = shared.lock();
            loop {
                if let Some(job) = st.mailbox.pop() {
                    break job;
                }
                if st.closing {
                    return;
                }
                st = shared
                    .ready
                    .wait_timeout(st, REPORT_EVERY)
                    .map(|(g, _)| g)
                    .unwrap_or_else(|e| e.into_inner().0);
                if Instant::now() >= report_at {
                    report(&mut wait, st.mailbox.take_coalesced());
                    report_at = Instant::now() + REPORT_EVERY;
                }
            }
        };
        let result = match job {
            Job::Inject(event) => inner
                .inject(&event)
                .map_err(|e| (Some(event), e.to_string())),
            Job::Refresh => inner.refresh().map_err(|e| (None, e.to_string())),
            Job::Focus(here) => inner.set_focus(here).map_err(|e| (None, e.to_string())),
        };
        let now = Instant::now();
        if let Job::Inject(_) = job {
            wait.record(now.saturating_duration_since(queued).as_micros() as u64);
        }
        match result {
            Err((Some(event), e)) => tracing::error!(?event, "injection: {e}"),
            Err((None, e)) => tracing::warn!("injection: {e}"),
            Ok(()) => {}
        }
        if now >= report_at {
            let coalesced = shared.lock().mailbox.take_coalesced();
            report(&mut wait, coalesced);
            report_at = now + REPORT_EVERY;
        }
    }
}

fn report(wait: &mut Histogram, coalesced: u64) {
    if let Some(l) = wait.take() {
        tracing::info!(
            n = l.count,
            p50_us = l.p50_us,
            p99_us = l.p99_us,
            max_us = l.max_us,
            coalesced,
            "injection thread (queued → injected)"
        );
    }
}

/// The injection thread goes before ordinary work: a late cursor is what the user notices.
#[cfg(windows)]
fn raise_priority() {
    crate::windows::raise_thread_priority();
}

#[cfg(not(windows))]
fn raise_priority() {}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::event::{HidUsage, KeyState};

    fn abs(x: f64) -> Job {
        Job::Inject(InputEvent::MotionAbsolute { x, y: 0.0 })
    }

    fn rel(dx: f64) -> Job {
        Job::Inject(InputEvent::MotionRelative { dx, dy: 1.0 })
    }

    fn key(state: KeyState) -> Job {
        Job::Inject(InputEvent::Key {
            usage: HidUsage::key(0x04),
            state,
        })
    }

    fn drain(m: &mut Mailbox) -> Vec<Job> {
        std::iter::from_fn(|| m.pop().map(|(j, _)| j)).collect()
    }

    #[test]
    fn only_the_newest_position_waits() {
        let mut m = Mailbox::default();
        let t = Instant::now();
        for x in 0..100 {
            m.push(abs(f64::from(x)), t);
        }
        assert_eq!(drain(&mut m), vec![abs(99.0)]);
        assert_eq!(m.take_coalesced(), 99);
        for _ in 0..3 {
            m.push(rel(2.0), t);
        }
        assert_eq!(
            drain(&mut m),
            vec![Job::Inject(InputEvent::MotionRelative { dx: 6.0, dy: 3.0 })]
        );
    }

    /// A click lands where the cursor was when it was pressed: positions are never merged
    /// across a key or a button.
    #[test]
    fn keys_and_buttons_keep_their_order_and_their_position() {
        let mut m = Mailbox::default();
        let t = Instant::now();
        m.push(abs(1.0), t);
        m.push(abs(2.0), t);
        m.push(key(KeyState::Pressed), t);
        m.push(abs(3.0), t);
        m.push(Job::Focus(false), t);
        m.push(abs(4.0), t);
        m.push(abs(5.0), t);
        m.push(rel(1.0), t);
        assert_eq!(
            drain(&mut m),
            vec![
                abs(2.0),
                key(KeyState::Pressed),
                abs(3.0),
                Job::Focus(false),
                abs(5.0),
                rel(1.0),
            ]
        );
    }

    #[test]
    fn the_oldest_stamp_is_kept() {
        let mut m = Mailbox::default();
        let t0 = Instant::now();
        m.push(abs(1.0), t0);
        m.push(abs(2.0), t0 + Duration::from_millis(5));
        assert_eq!(m.pop(), Some((abs(2.0), t0)));
    }

    /// Records every call; it can be made slow to see the coalescing happen.
    struct Slow {
        tx: mpsc::Sender<Job>,
        delay: Duration,
    }

    impl Injector for Slow {
        type Error = Infallible;
        fn inject(&mut self, event: &InputEvent) -> Result<(), Infallible> {
            std::thread::sleep(self.delay);
            let _ = self.tx.send(Job::Inject(*event));
            Ok(())
        }
        fn refresh(&mut self) -> Result<(), Infallible> {
            let _ = self.tx.send(Job::Refresh);
            Ok(())
        }
        fn set_focus(&mut self, here: bool) -> Result<(), Infallible> {
            let _ = self.tx.send(Job::Focus(here));
            Ok(())
        }
    }

    /// The bug: with a slow injection (1 ms per `SendInput`) and 1000 movements a second, a
    /// queue grew and the cursor arrived seconds late. Now the network side never waits and
    /// what reaches the system ends at the last position, with far fewer injections.
    #[test]
    fn a_slow_injection_never_builds_a_queue() {
        let (tx, rx) = mpsc::channel();
        let mut inj = ThreadedInjector::spawn(
            Slow {
                tx,
                delay: Duration::from_millis(1),
            },
            "test-inject",
        )
        .unwrap();
        let start = Instant::now();
        inj.set_focus(true).unwrap();
        for x in 0..2000 {
            inj.inject(&InputEvent::MotionAbsolute {
                x: f64::from(x),
                y: 0.0,
            })
            .unwrap();
        }
        inj.inject(&InputEvent::Key {
            usage: HidUsage::key(0x04),
            state: KeyState::Pressed,
        })
        .unwrap();
        // Pushing 2000 movements does not wait for any of them.
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "{:?}",
            start.elapsed()
        );
        drop(inj);
        let got: Vec<Job> = rx.try_iter().collect();
        assert_eq!(got.first(), Some(&Job::Focus(true)));
        assert_eq!(got.last(), Some(&key(KeyState::Pressed)));
        assert_eq!(
            got[got.len() - 2],
            abs(1999.0),
            "the newest position went out"
        );
        assert!(
            got.len() < 1000,
            "{} injections for 2000 movements",
            got.len()
        );
    }

    #[test]
    fn dropping_it_injects_what_was_waiting() {
        let (tx, rx) = mpsc::channel();
        let mut inj = ThreadedInjector::spawn(
            Slow {
                tx,
                delay: Duration::ZERO,
            },
            "test-inject",
        )
        .unwrap();
        inj.inject(&InputEvent::Key {
            usage: HidUsage::key(0x04),
            state: KeyState::Released,
        })
        .unwrap();
        inj.set_focus(false).unwrap();
        drop(inj);
        let got: Vec<Job> = rx.try_iter().collect();
        assert_eq!(got, vec![key(KeyState::Released), Job::Focus(false)]);
    }
}
