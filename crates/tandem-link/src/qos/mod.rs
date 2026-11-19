//! Priorities and cap of the data (DECISIONS A2, DESIGN §6).
//!
//! Data (tunnels, files) goes over any path, Wi-Fi too. So that it does not add
//! latency to the keyboard and mouse:
//!
//! - **QUIC stream priority** ([`StreamClass`]): input > control > clipboard >
//!   data. QUIC sends the pending data of the highest-priority stream first, and the
//!   motion datagrams go before any stream in each packet.
//! - **Cap on Wi-Fi** ([`data_cap`]): priority only orders what has not yet left;
//!   what is already in the access point's queue leaves in arrival order. With data
//!   below what Wi-Fi gives (by default, 50 % of what was measured) that queue does not
//!   grow and the next key does not wait behind a file.
//! - **Pause by latency** ([`LatencyGuard`]): if the input p99 still exceeds its
//!   target for 2 s (the Wi-Fi got worse), data is paused until it drops.
//!
//! All that lives in one [`Qos`] per path ([`LinkQos`]): between the two machines there is more than
//! one connection (the one each agent opens and the one it accepts), and all those of the same path
//! share the same air, so they share the cap and the pause
//! ([`Connection::with_qos`](crate::transport::Connection::with_qos)). The data modules
//! write through a [`DataStream`], which waits for the pause and the cap, and follow the pause with
//! [`Qos::gate`] or [`Qos::events`].
//!
//! **Whoever measures the input (T-093)** passes each latency to
//! `conn.qos().record_input_latency(..)` of the connection it arrived on, **on both
//! machines**: the data of both compete for the same air, so the side that injects
//! measures it and the one that captures receives it back (the T-093 histogram) and also
//! records it in its `Qos`.

mod cap;
mod guard;
mod limiter;
mod stream;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tandem_core::DataPrefs;
use tandem_proto::Channel;
use tokio::sync::{broadcast, watch};
use tokio::time::{self, Instant};

pub use cap::{UNMEASURED_WIFI, data_cap, input_p99_target};
pub use guard::{GuardConfig, LatencyGuard, Transition};
pub use limiter::{MAX_BURST, MIN_CAP, RateLimiter};
pub use stream::DataStream;

use crate::manager::LinkKind;

/// Class of a QUIC stream, from highest to lowest priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StreamClass {
    /// Keys and clicks (motion goes in datagrams).
    Input,
    /// Requests between agents, pings, state.
    Control,
    Clipboard,
    /// Tunnels and files.
    Data,
}

impl StreamClass {
    /// Priority for quinn (higher, earlier). Control stays at 0, the default: a
    /// stream without a class is a control one.
    pub fn priority(self) -> i32 {
        match self {
            StreamClass::Input => 10,
            StreamClass::Control => 0,
            StreamClass::Clipboard => -10,
            StreamClass::Data => -20,
        }
    }

    /// The class of a channel's own stream ([`tandem_proto::ControlMsg::Open`]).
    pub fn of(channel: Channel) -> Self {
        match channel {
            Channel::Input => StreamClass::Input,
            Channel::Clipboard => StreamClass::Clipboard,
            // Requests are control; the data channel becomes `Data` when `Attach` is requested.
            Channel::Tunnel => StreamClass::Control,
            Channel::Measure => StreamClass::Data,
        }
    }
}

/// Whether data may go out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataGate {
    Open,
    /// Input exceeded its target: `p99` measured against `target`.
    Paused {
        p99: Duration,
        target: Duration,
    },
}

impl DataGate {
    pub fn is_open(&self) -> bool {
        matches!(self, DataGate::Open)
    }
}

/// What happens to a connection's data, for the modules and the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QosEvent {
    Paused {
        link: Option<LinkKind>,
        p99: Duration,
        target: Duration,
    },
    Resumed {
        link: Option<LinkKind>,
        /// How long the pause lasted.
        after: Duration,
    },
    /// The cap changed (bytes per second; `None` = no cap).
    Cap {
        link: Option<LinkKind>,
        cap: Option<u64>,
    },
}

/// How often the p99 is recalculated at most, and how often whoever waits checks the pause.
const EVALUATE_EVERY: Duration = Duration::from_millis(20);
const RECHECK: Duration = Duration::from_millis(100);

/// Fraction of the configured cap that is used, in thousandths.
const FULL: u32 = 1000;
/// The least it is lowered (the real cap never goes under [`MIN_CAP`]).
const MIN_SCALE: u32 = 10;
/// How often the cap may rise at most.
const RAMP_EVERY: Duration = Duration::from_secs(1);

/// Priority, cap and pause of a connection's data. Cheap to clone: all the copies
/// share the same state.
#[derive(Clone)]
pub struct Qos {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Qos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self.lock();
        f.debug_struct("Qos")
            .field("link", &s.link)
            .field("cap", &s.limiter.as_ref().map(RateLimiter::cap))
            .field("paused", &s.guard.is_paused())
            .finish_non_exhaustive()
    }
}

struct Shared {
    inner: Mutex<Inner>,
    gate: watch::Sender<DataGate>,
    events: broadcast::Sender<QosEvent>,
}

struct Inner {
    /// The effective label: the worse of the own class and the one the other end announces.
    link: Option<LinkKind>,
    own: Option<LinkKind>,
    peer: Option<LinkKind>,
    /// The connection that announced `peer`: only it may erase it when closing.
    peer_from: Option<usize>,
    prefs: DataPrefs,
    measured: Option<u64>,
    limiter: Option<RateLimiter>,
    guard: LatencyGuard,
    last_eval: Option<Instant>,
    paused_at: Option<Instant>,
    /// What part of the configured cap is used now (thousandths). It goes down when input
    /// falls behind and rises little by little when it does not: the cap comes from the last measurement, and if the Wi-Fi
    /// worsens afterwards, the configured one no longer fits.
    scale: u32,
    last_decrease: Option<Instant>,
    last_change: Option<Instant>,
}

impl Default for Qos {
    fn default() -> Self {
        Self::new(GuardConfig::default())
    }
}

impl Qos {
    /// No known path: no cap and with the Wi-Fi target.
    pub fn new(guard: GuardConfig) -> Self {
        let (gate, _) = watch::channel(DataGate::Open);
        let (events, _) = broadcast::channel(32);
        Qos {
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner {
                    link: None,
                    own: None,
                    peer: None,
                    peer_from: None,
                    prefs: DataPrefs::default(),
                    measured: None,
                    limiter: None,
                    guard: LatencyGuard::new(guard, input_p99_target(None)),
                    last_eval: None,
                    paused_at: None,
                    scale: FULL,
                    last_decrease: None,
                    last_change: None,
                }),
                gate,
                events,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.shared
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Which path the connection goes over according to this end. Changing path changes the cap and
    /// the target, and lifts a pause: what the other path measured no longer holds.
    pub fn set_link(&self, link: Option<LinkKind>) {
        let mut s = self.lock();
        s.own = link;
        self.relabel(&mut s);
    }

    /// How the other end classifies this path. It is kept apart from its own and the
    /// label is recomputed every time, so that it recovers when the other announces
    /// a cable again. `None` when its connection closes.
    pub fn set_peer_link(&self, link: Option<LinkKind>) {
        let mut s = self.lock();
        s.peer = link;
        s.peer_from = None;
        self.relabel(&mut s);
    }

    /// Like `set_peer_link(Some(..))`, recording which connection announced it.
    pub fn set_peer_link_from(&self, conn: usize, link: LinkKind) {
        let mut s = self.lock();
        s.peer = Some(link);
        s.peer_from = Some(conn);
        self.relabel(&mut s);
    }

    /// Erases the other's class when `conn` closes, but only if it announced it: a new
    /// connection of the same path that already announced its own does not lose the current one.
    pub fn clear_peer_link_of(&self, conn: usize) {
        let mut s = self.lock();
        if s.peer_from == Some(conn) {
            s.peer = None;
            s.peer_from = None;
            self.relabel(&mut s);
        }
    }

    fn relabel(&self, s: &mut Inner) {
        let link = match (s.own, s.peer) {
            (Some(a), Some(b)) => Some(crate::iface::worst(a, b)),
            (a, b) => a.or(b),
        };
        if s.link == link {
            return;
        }
        s.link = link;
        s.scale = FULL;
        let target = input_p99_target(link);
        if let Some(Transition::Resume { .. }) = s.guard.reset(target) {
            self.resumed(s, Instant::now());
        }
        self.update_cap(s);
    }

    /// The class of this end, without the other's.
    pub fn own_link(&self) -> Option<LinkKind> {
        self.lock().own
    }

    /// The effective label: the worse of the two classes.
    pub fn link(&self) -> Option<LinkKind> {
        self.lock().link
    }

    /// The desk's preferences (`[data]`).
    pub fn set_prefs(&self, prefs: DataPrefs) {
        let mut s = self.lock();
        if s.prefs != prefs {
            s.prefs = prefs;
            self.update_cap(&mut s);
        }
    }

    pub fn prefs(&self) -> DataPrefs {
        self.lock().prefs
    }

    /// The last bandwidth measured on the current path, in bytes per second.
    pub fn set_measured(&self, bytes_per_sec: u64) {
        let mut s = self.lock();
        if s.measured != Some(bytes_per_sec) {
            // A new measurement says how much the path gives now: it is used whole again.
            s.measured = Some(bytes_per_sec);
            s.scale = FULL;
            self.update_cap(&mut s);
        }
    }

    /// The last bandwidth measured ([`Self::set_measured`]).
    pub fn measured(&self) -> Option<u64> {
        self.lock().measured
    }

    /// The p50 and p99 of input in the last window (1 s); `None` with no input.
    pub fn input_latency(&self) -> Option<(Duration, Duration)> {
        self.lock().guard.percentiles(Instant::now())
    }

    /// The p99 tolerated for input on this path.
    pub fn input_target(&self) -> Duration {
        self.lock().guard.target()
    }

    /// The current cap in bytes per second; `None` means no cap.
    pub fn cap(&self) -> Option<u64> {
        self.lock().limiter.as_ref().map(RateLimiter::cap)
    }

    /// The configured cap (without the part that was lowered for latency).
    pub fn configured_cap(&self) -> Option<u64> {
        let s = self.lock();
        data_cap(s.link, &s.prefs, s.measured)
    }

    fn update_cap(&self, s: &mut Inner) {
        let cap = data_cap(s.link, &s.prefs, s.measured).map(|c| {
            let scaled = u128::from(c) * u128::from(s.scale) / u128::from(FULL);
            u64::try_from(scaled).unwrap_or(c)
        });
        let now = Instant::now();
        let before = s.limiter.as_ref().map(RateLimiter::cap);
        s.limiter = match (s.limiter.take(), cap) {
            (Some(mut l), Some(cap)) => {
                l.set_cap(cap, now);
                Some(l)
            }
            (None, Some(cap)) => Some(RateLimiter::new(cap, now)),
            (_, None) => None,
        };
        // The real cap may be the minimum and not the requested one.
        let cap = s.limiter.as_ref().map(RateLimiter::cap);
        if cap == before {
            return;
        }
        if s.scale == FULL {
            tracing::info!(link = ?s.link, ?cap, "data cap");
        } else {
            tracing::debug!(link = ?s.link, ?cap, scale = s.scale, "data cap adjusted for latency");
        }
        let _ = self.shared.events.send(QosEvent::Cap { link: s.link, cap });
    }

    /// Adjusts the part of the cap that is used according to the input p99: it drops at once if it
    /// nears the target (at most once per window, which is how long the p99 takes to see
    /// the effect) and rises little by little if there is margin. That way a Wi-Fi that worsens and stays
    /// bad does not make data pause and resume in a cycle.
    fn adapt(&self, s: &mut Inner, now: Instant, paused: bool) {
        if s.limiter.is_none() {
            return;
        }
        let target = s.guard.target();
        let window = s.guard.window();
        let since =
            |t: Option<Instant>| t.map_or(Duration::MAX, |t| now.saturating_duration_since(t));
        let scale = match s.guard.last_p99() {
            _ if paused => return,
            Some(p) if p > target * 3 / 4 => {
                if since(s.last_decrease) < window {
                    return;
                }
                s.last_decrease = Some(now);
                // Past the target, half; near it, a bit less.
                let factor = if p > target { 500 } else { 700 };
                (s.scale * factor / FULL).max(MIN_SCALE)
            }
            Some(p) if p > target / 2 => return,
            _ => {
                if s.scale >= FULL || since(s.last_change) < RAMP_EVERY {
                    return;
                }
                (s.scale + s.scale / 4 + 5).min(FULL)
            }
        };
        s.last_change = Some(now);
        s.scale = scale;
        self.update_cap(s);
    }

    /// A measured input latency (from whoever captures to whoever injects).
    pub fn record_input_latency(&self, latency: Duration) {
        let now = Instant::now();
        let mut s = self.lock();
        s.guard.record(now, latency);
        if s.last_eval
            .is_some_and(|t| now.duration_since(t) < EVALUATE_EVERY)
        {
            return;
        }
        self.evaluate(&mut s, now);
    }

    /// The p99 of input latency in the guard's window; `None` without samples.
    pub fn input_p99(&self) -> Option<Duration> {
        self.lock().guard.p99(Instant::now())
    }

    /// Checks the pause without a new sample (without input it also has to resume).
    pub fn tick(&self) {
        let mut s = self.lock();
        self.evaluate(&mut s, Instant::now());
    }

    fn evaluate(&self, s: &mut Inner, now: Instant) {
        s.last_eval = Some(now);
        let transition = s.guard.evaluate(now);
        self.adapt(s, now, s.guard.is_paused());
        match transition {
            Some(Transition::Pause { p99 }) => {
                let target = s.guard.target();
                s.paused_at = Some(now);
                // What there was was not enough: on returning, half.
                s.scale = (s.scale / 2).max(MIN_SCALE);
                s.last_decrease = Some(now);
                s.last_change = Some(now);
                self.update_cap(s);
                tracing::warn!(
                    link = ?s.link,
                    p99_ms = p99.as_secs_f64() * 1e3,
                    target_ms = target.as_secs_f64() * 1e3,
                    "data paused: input exceeded its target"
                );
                self.shared
                    .gate
                    .send_replace(DataGate::Paused { p99, target });
                let _ = self.shared.events.send(QosEvent::Paused {
                    link: s.link,
                    p99,
                    target,
                });
            }
            Some(Transition::Resume { .. }) => self.resumed(s, now),
            None => {}
        }
    }

    fn resumed(&self, s: &mut Inner, now: Instant) {
        let after = s
            .paused_at
            .take()
            .map(|t| now.saturating_duration_since(t))
            .unwrap_or_default();
        tracing::info!(link = ?s.link, after_ms = after.as_millis(), "data resumed");
        self.shared.gate.send_replace(DataGate::Open);
        let _ = self.shared.events.send(QosEvent::Resumed {
            link: s.link,
            after,
        });
    }

    /// Whether data may go out now.
    pub fn gate(&self) -> DataGate {
        *self.shared.gate.borrow()
    }

    /// To follow the pause.
    pub fn watch_gate(&self) -> watch::Receiver<DataGate> {
        self.shared.gate.subscribe()
    }

    /// Pauses, resumes and cap changes, in order. Whoever falls behind loses the oldest
    /// (`RecvError::Lagged`); the current state is always in [`Self::gate`].
    pub fn events(&self) -> broadcast::Receiver<QosEvent> {
        self.shared.events.subscribe()
    }

    /// Waits until data may go out and returns how many of the `want` may go out now
    /// (at least 1 if `want > 0`).
    pub async fn acquire(&self, want: usize) -> usize {
        if want == 0 {
            return 0;
        }
        let mut gate = self.watch_gate();
        loop {
            if !gate.borrow_and_update().is_open() {
                // Without input no samples arrive: the pause is checked on its own every so often.
                let _ = time::timeout(RECHECK, gate.changed()).await;
                self.tick();
                continue;
            }
            let taken = {
                let mut s = self.lock();
                match s.limiter.as_mut() {
                    Some(l) => l.take(Instant::now(), want),
                    None => Ok(want),
                }
            };
            match taken {
                Ok(n) => return n,
                // Short: a pause or a new cap is noticed right away.
                Err(wait) => time::sleep(wait.min(RECHECK)).await,
            }
        }
    }

    /// Wraps `writer` so that everything written respects the pause and the cap.
    pub fn data_stream<W>(&self, writer: W) -> DataStream<W> {
        DataStream::new(self.clone(), writer)
    }
}

/// One [`Qos`] per path, shared by all the connections with the other agent over that
/// path. Cheap to clone.
#[derive(Clone, Default)]
pub struct LinkQos {
    inner: Arc<Mutex<LinkQosInner>>,
}

#[derive(Default)]
struct LinkQosInner {
    prefs: DataPrefs,
    by_link: BTreeMap<LinkKind, Qos>,
}

impl std::fmt::Debug for LinkQos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkQos").finish_non_exhaustive()
    }
}

impl LinkQos {
    fn lock(&self) -> MutexGuard<'_, LinkQosInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The one for `kind`; the first to ask for it creates it with the current preferences.
    pub fn get(&self, kind: LinkKind) -> Qos {
        let mut s = self.lock();
        let prefs = s.prefs;
        s.by_link
            .entry(kind)
            .or_insert_with(|| {
                let qos = Qos::default();
                qos.set_link(Some(kind));
                qos.set_prefs(prefs);
                qos
            })
            .clone()
    }

    /// The desk's `[data]`, for all paths (those that already exist and those to come).
    pub fn set_prefs(&self, prefs: DataPrefs) {
        let mut s = self.lock();
        s.prefs = prefs;
        for qos in s.by_link.values() {
            qos.set_prefs(prefs);
        }
    }

    pub fn prefs(&self) -> DataPrefs {
        self.lock().prefs
    }
}

impl Qos {
    /// Whether `self` and `other` are the same one (they share state).
    pub fn same(&self, other: &Qos) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}

#[cfg(test)]
mod tests;
