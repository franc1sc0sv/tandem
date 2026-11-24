//! Per-path metrics for the Network screen and the Desk (T-102, DESIGN §6).
//!
//! - [`NetMetrics`]: pure logic. It receives the link manager's snapshot, the input latency of
//!   each [`Qos`](crate::qos::Qos), the path changes and the measurements, and
//!   builds the [`NetworkStatus`] that the UI sees: state of each path, RTT, p50/p99 of
//!   input, last bandwidth, 60 s of history and the last 50 changes.
//! - [`Metrics`]: the same, shared (the agent keeps it even if the manager is rebuilt).
//! - [`follow`]: the task that feeds it from a running [`LinkManager`](crate::manager::LinkManager),
//!   one sample per second.
//! - [`probe`]: the bandwidth test.

mod notices;
pub mod probe;

pub use notices::link_notices;

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tandem_proto::net::{HISTORY_SECS, MAX_SWITCHES};
use tandem_proto::{Bandwidth, LatencySample, NetworkStatus, PathMetrics, PathState, PathSwitch};
use tokio::sync::{mpsc, watch};
use tokio::time::{self, MissedTickBehavior};

use crate::manager::{LinkEvent, LinkKind, LinkStatus, Snapshot};
use crate::qos::{LinkQos, input_p99_target};

/// How often a sample is taken.
pub const SAMPLE_EVERY: Duration = Duration::from_secs(1);

/// What is known about a path.
#[derive(Debug, Clone, Default)]
struct PathRecord {
    rtt: Option<Duration>,
    ever_up: bool,
    input: Option<(Duration, Duration)>,
    bandwidth: Option<Bandwidth>,
    history: VecDeque<LatencySample>,
}

/// The metrics of the three paths. No I/O or clock: time arrives as an argument.
#[derive(Debug, Clone)]
pub struct NetMetrics {
    paths: BTreeMap<LinkKind, PathRecord>,
    active: Option<LinkKind>,
    /// From newest to oldest.
    switches: VecDeque<PathSwitch>,
    measuring: Option<LinkKind>,
    notices: Vec<tandem_proto::NetworkNotice>,
    /// About the link set itself (T-223), kept apart from the cable notices because a
    /// different task replaces those.
    link_notices: Vec<tandem_proto::NetworkNotice>,
}

impl Default for NetMetrics {
    fn default() -> Self {
        NetMetrics {
            paths: LinkKind::ALL
                .into_iter()
                .map(|k| (k, PathRecord::default()))
                .collect(),
            active: None,
            switches: VecDeque::new(),
            measuring: None,
            notices: Vec::new(),
            link_notices: Vec::new(),
        }
    }
}

fn micros(d: Duration) -> u32 {
    u32::try_from(d.as_micros()).unwrap_or(u32::MAX)
}

impl NetMetrics {
    /// The manager's snapshot: what answers and which one carries the traffic. A path that is not in
    /// `links` (not configured) counts as not answering.
    pub fn observe(&mut self, links: &[LinkStatus], active: Option<LinkKind>) {
        for (kind, p) in &mut self.paths {
            p.rtt = links.iter().find(|l| l.kind == *kind).and_then(|l| l.rtt);
            p.ever_up |= p.rtt.is_some();
        }
        self.active = active;
    }

    /// The p50 and p99 of input per `kind` in the last second.
    pub fn set_input(&mut self, kind: LinkKind, input: Option<(Duration, Duration)>) {
        if let Some(p) = self.paths.get_mut(&kind) {
            p.input = input;
        }
    }

    /// A second went by: one more sample in each path's history.
    pub fn tick(&mut self) {
        for p in self.paths.values_mut() {
            p.history.push_back(LatencySample {
                rtt_us: p.rtt.map(micros),
                input_p99_us: p.input.map(|(_, p99)| micros(p99)),
            });
            while p.history.len() > HISTORY_SECS {
                p.history.pop_front();
            }
        }
    }

    /// A change of the active path, `at_ms` in Unix milliseconds.
    pub fn switched(&mut self, event: &LinkEvent, at_ms: u64) {
        self.active = event.to;
        self.switches.push_front(PathSwitch {
            at_ms,
            from: event.from.map(Into::into),
            to: event.to.map(Into::into),
            took_ms: event
                .took
                .map(|d| u32::try_from(d.as_millis()).unwrap_or(u32::MAX)),
        });
        self.switches.truncate(MAX_SWITCHES);
    }

    /// A bandwidth measurement of `kind`.
    pub fn measured(&mut self, kind: LinkKind, bytes_per_sec: u64, at_ms: u64) {
        if let Some(p) = self.paths.get_mut(&kind) {
            p.bandwidth = Some(Bandwidth {
                bytes_per_sec,
                at_ms,
            });
        }
    }

    pub fn state(&self, kind: LinkKind) -> PathState {
        let Some(p) = self.paths.get(&kind) else {
            return PathState::Unplugged;
        };
        match (p.rtt.is_some(), p.ever_up) {
            (true, _) if self.active == Some(kind) => PathState::Primary,
            (true, _) => PathState::Backup,
            (false, true) => PathState::Down,
            (false, false) => PathState::Unplugged,
        }
    }

    pub fn status(&self) -> NetworkStatus {
        NetworkStatus {
            paths: self
                .paths
                .iter()
                .map(|(kind, p)| PathMetrics {
                    link: (*kind).into(),
                    state: self.state(*kind),
                    rtt_us: p.rtt.map(micros),
                    input_p50_us: p.input.map(|(p50, _)| micros(p50)),
                    input_p99_us: p.input.map(|(_, p99)| micros(p99)),
                    input_target_us: micros(input_p99_target(Some(*kind))),
                    bandwidth: p.bandwidth,
                    history: p.history.iter().copied().collect(),
                })
                .collect(),
            switches: self.switches.iter().copied().collect(),
            measuring: self.measuring.map(Into::into),
            notices: self
                .link_notices
                .iter()
                .chain(&self.notices)
                .cloned()
                .collect(),
        }
    }
}

/// The shared metrics. Cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct Metrics {
    inner: Arc<Mutex<NetMetrics>>,
}

/// While it exists, there is a measurement in progress on its path.
#[must_use = "the measurement ends when it is dropped"]
#[derive(Debug)]
pub struct Measuring {
    metrics: Metrics,
}

impl Drop for Measuring {
    fn drop(&mut self) {
        self.metrics.lock().measuring = None;
    }
}

/// The Unix time in milliseconds, for the dates the UI sees.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

impl Metrics {
    fn lock(&self) -> MutexGuard<'_, NetMetrics> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The notices for the user (replaced whole).
    pub fn set_notices(&self, notices: Vec<tandem_proto::NetworkNotice>) {
        self.lock().notices = notices;
    }

    /// The notices about the link set: shadowed candidates or a manager that could not
    /// start (replaced whole, see [`link_notices`]).
    pub fn set_link_notices(&self, notices: Vec<tandem_proto::NetworkNotice>) {
        self.lock().link_notices = notices;
    }

    pub fn status(&self) -> NetworkStatus {
        self.lock().status()
    }

    pub fn update<R>(&self, f: impl FnOnce(&mut NetMetrics) -> R) -> R {
        f(&mut self.lock())
    }

    /// Marks that `kind` is being measured. `None` if there is already another measurement: two at once
    /// share the path and both would give half.
    pub fn begin_measure(&self, kind: LinkKind) -> Option<Measuring> {
        let mut m = self.lock();
        if m.measuring.is_some() {
            return None;
        }
        m.measuring = Some(kind);
        Some(Measuring {
            metrics: self.clone(),
        })
    }

    /// Stores a measurement of `kind` and gives it to its `Qos`: the data cap over Wi-Fi
    /// comes from the last thing measured (T-101).
    pub fn record_bandwidth(&self, kind: LinkKind, bytes_per_sec: u64, qos: &LinkQos) {
        self.lock().measured(kind, bytes_per_sec, now_ms());
        qos.get(kind).set_measured(bytes_per_sec);
    }
}

/// Feeds `metrics` from a running link manager (its snapshot and its change queue)
/// and the input latency of each path (`qos`), with one sample every `every`.
/// It ends when the manager is dropped.
pub async fn follow<H: Clone>(
    metrics: Metrics,
    mut links: watch::Receiver<Snapshot<H>>,
    mut events: mpsc::UnboundedReceiver<LinkEvent>,
    qos: LinkQos,
    every: Duration,
) {
    let observe = |links: &watch::Receiver<Snapshot<H>>| {
        let s = links.borrow();
        let active = s.active.as_ref().map(|a| a.kind);
        metrics.update(|m| m.observe(&s.links, active));
    };
    let mut tick = time::interval(every);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            // The changes first: the sample of the same instant already sees them.
            biased;
            event = events.recv() => {
                let Some(event) = event else { return };
                metrics.update(|m| m.switched(&event, now_ms()));
                observe(&links);
            }
            changed = links.changed() => {
                if changed.is_err() {
                    return;
                }
                observe(&links);
            }
            _ = tick.tick() => {
                observe(&links);
                let input: Vec<_> = LinkKind::ALL
                    .into_iter()
                    .map(|k| (k, qos.get(k).input_latency()))
                    .collect();
                metrics.update(|m| {
                    for (k, i) in input {
                        m.set_input(k, i);
                    }
                    m.tick();
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
