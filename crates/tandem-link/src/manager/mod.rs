//! Multiple-link manager and plane policy (DESIGN §6).
//!
//! Each configured link (USB4, Ethernet, Wi-Fi) has its own connection
//! to the other agent and a health ping: every second on the backups and every
//! 200 ms on the active one, so a cut cable is noticed well inside DESIGN §6's
//! 1 s budget (T-310). The active link is the healthy one of highest priority: if it
//! drops, it moves to the next as soon as its ping goes unanswered for
//! [`HealthConfig::misses_to_drop`] windows in a row, or fails outright (without
//! waiting for QUIC's idle timeout). With no healthy backup to move to, the active
//! link waits longer for its Pong ([`HealthConfig::ping_interval`]): dropping it
//! early would gain nothing and cost a reconnect. To go back to a
//! better link that already dropped, it has to have
//! [`HealthConfig::settle`] answering without cuts (hysteresis: a cable that
//! flickers does not drag traffic back and forth), and it does not go up while there is
//! a transaction in progress ([`LinkManager::hold`]). Every change comes out as a
//! [`LinkEvent`], in order.
//!
//! All traffic goes over the active one, data too (DECISIONS A2): what holds
//! it back on Wi-Fi to protect input is each connection's [`Qos`](crate::qos::Qos),
//! not the manager.

mod kind;
mod probe;
mod quic;
mod table;

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::task::{AbortHandle, JoinHandle, JoinSet};
use tokio::time::{self, Instant};

pub use kind::{LinkCandidate, LinkKind};
pub use probe::Prober;
pub use quic::{QuicProber, QuicSession, answer_pings};
pub use table::{LinkEvent, LinkStatus};
// `LinkControl` and `ManagerGone` are defined below.

use table::LinkTable;

/// Health check timings.
///
/// A cut on the active link is noticed at most [`HealthConfig::worst_detection`]
/// after its last answer (200 ms + 2 × 200 ms = 600 ms by default), and the switch to
/// an already healthy backup is immediate: DESIGN §6 asks for under 1 s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthConfig {
    /// How often each backup link is pinged (DESIGN §6: 1 s). Slow on purpose:
    /// it only has to know the backup is there, and it keeps Wi-Fi radios quiet.
    pub ping_interval: Duration,
    /// How often the active link is pinged: its cut has to be seen fast.
    pub active_ping_interval: Duration,
    /// One window of waiting for the Pong. A window without it is a missed probe.
    pub ping_timeout: Duration,
    /// Windows in a row without the Pong before the link counts as down. The
    /// control stream is reliable, so the next window keeps waiting for the same
    /// Pong: a single lost packet is retransmitted by QUIC and only costs one
    /// window, which does not switch (hysteresis on the way down).
    pub misses_to_drop: u32,
    /// Cap for opening the connection of a dropped link.
    pub connect_timeout: Duration,
    /// How long a link that already dropped before has to answer without cuts before
    /// going back to it. With 0 it goes back on the first good ping.
    pub settle: Duration,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(1),
            active_ping_interval: Duration::from_millis(200),
            ping_timeout: Duration::from_millis(200),
            misses_to_drop: 2,
            connect_timeout: Duration::from_secs(1),
            settle: Duration::from_secs(3),
        }
    }
}

impl HealthConfig {
    /// The longest a cut on the active link can go unnoticed: from its last answer
    /// to the next ping, plus every window that ping waits for its Pong.
    pub fn worst_detection(&self) -> Duration {
        self.active_ping_interval + self.drop_after()
    }

    /// How long a ping waits for its Pong while there is a healthy link to move to.
    fn drop_after(&self) -> Duration {
        self.ping_timeout * self.misses_to_drop
    }

    /// How long the active link's ping waits when there is no healthy backup: a
    /// drop would leave input with no path at all, so a stall is given as long
    /// as a backup's ping interval (1 s), and never less than the normal wait.
    fn lone_patience(&self) -> Duration {
        self.ping_interval.max(self.drop_after())
    }
}

/// What each ping task needs to know about the others to pick its pace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Cadence {
    active: Option<LinkKind>,
    /// Some link other than the active one is healthy.
    backup_ready: bool,
}

impl Cadence {
    fn of<H>(table: &LinkTable<H>) -> Self {
        let active = table.active().map(|(kind, _)| kind);
        let backup_ready = table
            .statuses()
            .iter()
            .any(|l| Some(l.kind) != active && l.rtt.is_some());
        Self {
            active,
            backup_ready,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManagerError {
    #[error("no link is configured")]
    NoCandidates,
    #[error("the link {0} is configured twice")]
    DuplicateLink(LinkKind),
    /// A timing or the miss count is 0.
    #[error("the health check timings cannot be 0")]
    ZeroDuration,
}

/// Active link and what is needed to send traffic over it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveLink<H> {
    pub kind: LinkKind,
    pub addr: SocketAddr,
    pub handle: H,
}

/// Snapshot of the manager; updated with every ping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot<H> {
    pub active: Option<ActiveLink<H>>,
    /// All the configured links, in priority order.
    pub links: Vec<LinkStatus>,
    /// The connection of each healthy link (the active one included).
    pub handles: BTreeMap<LinkKind, H>,
}

/// Running manager. Dropping it stops the pings and closes the
/// connections of all the links.
pub struct LinkManager<H> {
    state: watch::Receiver<Snapshot<H>>,
    holds: Arc<watch::Sender<usize>>,
    control: LinkControl,
    task: JoinHandle<()>,
}

enum Command {
    Add(LinkCandidate),
    Remove(LinkKind),
}

/// To add and remove links with the manager running (T-094): a cable that appears or
/// is disconnected. Cheap to clone.
#[derive(Debug, Clone)]
pub struct LinkControl {
    tx: mpsc::UnboundedSender<Command>,
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Command")
    }
}

/// The manager no longer exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the link manager already ended")]
pub struct ManagerGone;

impl LinkControl {
    /// Adds a link and starts pinging it. If it was already there with that address nothing
    /// changes. If it came back after being removed healthy, it enters with the usual hysteresis.
    pub fn add(&self, link: LinkCandidate) -> Result<(), ManagerGone> {
        self.tx.send(Command::Add(link)).map_err(|_| ManagerGone)
    }

    /// Removes a link at once, without waiting for the ping: if it was the active one, input moves
    /// to the next.
    pub fn remove(&self, kind: LinkKind) -> Result<(), ManagerGone> {
        self.tx.send(Command::Remove(kind)).map_err(|_| ManagerGone)
    }
}

/// While it exists, the manager does not switch links unless the active one drops.
/// On dropping the last one, if a better link is already settled, it moves to it right away.
#[must_use = "the hold ends when it is dropped"]
pub struct LinkHold {
    holds: Arc<watch::Sender<usize>>,
}

impl fmt::Debug for LinkHold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinkHold").finish_non_exhaustive()
    }
}

impl Drop for LinkHold {
    fn drop(&mut self) {
        self.holds.send_modify(|n| *n = n.saturating_sub(1));
    }
}

type Report<H> = (LinkKind, Option<(H, Duration)>);

impl<H: Clone + Send + Sync + 'static> LinkManager<H> {
    /// Starts one ping per link. It also returns the queue of
    /// [`LinkEvent`]s, which keeps the order and loses no events even if the
    /// consumer falls behind. It has to be called inside a tokio runtime.
    pub fn spawn<P: Prober<Handle = H>>(
        candidates: &[LinkCandidate],
        prober: P,
        config: HealthConfig,
    ) -> Result<(Self, mpsc::UnboundedReceiver<LinkEvent>), ManagerError> {
        if [
            config.ping_interval,
            config.active_ping_interval,
            config.ping_timeout,
            config.connect_timeout,
        ]
        .contains(&Duration::ZERO)
            || config.misses_to_drop == 0
        {
            return Err(ManagerError::ZeroDuration);
        }
        let mut table = LinkTable::new(candidates, config.settle)?;
        let (state_tx, state) = watch::channel(Snapshot {
            active: None,
            links: table.statuses(),
            handles: BTreeMap::new(),
        });
        let (event_tx, events) = mpsc::unbounded_channel();
        let (report_tx, mut reports) = mpsc::unbounded_channel::<Report<H>>();
        let holds = Arc::new(watch::Sender::new(0usize));
        let mut held = holds.subscribe();
        // Each ping task looks at this to know whether it is the active link and
        // has to ping at the fast rate.
        let (active_tx, active_rx) = watch::channel(Cadence::default());

        let prober = Arc::new(prober);
        let mut links = JoinSet::new();
        let mut pingers: BTreeMap<LinkKind, AbortHandle> = BTreeMap::new();
        for c in table.candidates() {
            let kind = c.kind;
            let handle = links.spawn(run_link(
                prober.clone(),
                c,
                config,
                report_tx.clone(),
                active_rx.clone(),
            ));
            pingers.insert(kind, handle);
        }
        let (control_tx, mut commands) = mpsc::unbounded_channel::<Command>();

        // A single task applies the results: that way events come out in the
        // order in which the table changed. When it is aborted, `links` is dropped and
        // aborts the pings.
        let task = tokio::spawn(async move {
            let mut links = links;
            loop {
                let event = tokio::select! {
                    Some(command) = commands.recv() => match command {
                        Command::Add(c) => {
                            if table.add(c) {
                                if let Some(old) = pingers.remove(&c.kind) {
                                    old.abort();
                                }
                                let handle = links.spawn(run_link(
                                    prober.clone(),
                                    c,
                                    config,
                                    report_tx.clone(),
                                    active_rx.clone(),
                                ));
                                pingers.insert(c.kind, handle);
                            }
                            // A link without a ping yet does not change the active one.
                            None
                        }
                        Command::Remove(kind) => {
                            if let Some(old) = pingers.remove(&kind) {
                                old.abort();
                            }
                            table.remove(kind, Instant::now())
                        }
                    },
                    report = reports.recv() => {
                        let Some((kind, up)) = report else { return };
                        let held = *held.borrow_and_update() > 0;
                        table.report(kind, up, Instant::now(), held)
                    }
                    Ok(()) = held.changed() => {
                        let held = *held.borrow_and_update() > 0;
                        table.reevaluate(Instant::now(), held)
                    }
                };
                // The snapshot first: whoever receives the event already sees the new state.
                state_tx.send_replace(snapshot(&table));
                let cadence = Cadence::of(&table);
                active_tx.send_if_modified(|c| {
                    let changed = *c != cadence;
                    *c = cadence;
                    changed
                });
                if let Some(e) = event {
                    tracing::info!(
                        from = ?e.from,
                        to = ?e.to,
                        "active link changed"
                    );
                    // If nobody listens to the events, the manager carries on the same.
                    let _ = event_tx.send(e);
                }
            }
        });
        Ok((
            Self {
                state,
                holds,
                control: LinkControl { tx: control_tx },
                task,
            },
            events,
        ))
    }
}

impl<H> LinkManager<H> {
    /// To add and remove links while running.
    pub fn control(&self) -> LinkControl {
        self.control.clone()
    }

    /// Holds the active link during a transaction: the steps of one
    /// change do not jump links midway unless the active one drops.
    pub fn hold(&self) -> LinkHold {
        self.holds.send_modify(|n| *n += 1);
        LinkHold {
            holds: self.holds.clone(),
        }
    }
}

impl<H: Clone> LinkManager<H> {
    pub fn snapshot(&self) -> Snapshot<H> {
        self.state.borrow().clone()
    }

    /// To follow the snapshot (the UI draws the links with this).
    pub fn watch(&self) -> watch::Receiver<Snapshot<H>> {
        self.state.clone()
    }

    pub fn active(&self) -> Option<ActiveLink<H>> {
        self.state.borrow().active.clone()
    }

    pub fn links(&self) -> Vec<LinkStatus> {
        self.state.borrow().links.clone()
    }
}

impl<H> Drop for LinkManager<H> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn snapshot<H: Clone>(table: &LinkTable<H>) -> Snapshot<H> {
    let links = table.statuses();
    let active = table.active().and_then(|(kind, handle)| {
        let addr = links.iter().find(|l| l.kind == kind)?.addr;
        Some(ActiveLink {
            kind,
            addr,
            handle: handle.clone(),
        })
    });
    Snapshot {
        active,
        links,
        handles: table.handles(),
    }
}

enum Failure<E> {
    Connect(E),
    ConnectTimeout,
    Ping(E),
    PingTimeout,
}

impl<E: fmt::Display> fmt::Display for Failure<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Connect(e) => write!(f, "cannot connect: {e}"),
            Failure::ConnectTimeout => f.write_str("the connection did not answer in time"),
            Failure::Ping(e) => write!(f, "ping failed: {e}"),
            Failure::PingTimeout => f.write_str("the ping did not answer in time"),
        }
    }
}

async fn run_link<P: Prober>(
    prober: Arc<P>,
    link: LinkCandidate,
    config: HealthConfig,
    reports: mpsc::UnboundedSender<Report<P::Handle>>,
    mut active: watch::Receiver<Cadence>,
) {
    let mut session = None;
    let mut was_up = false;
    let mut last_start: Option<Instant> = None;
    loop {
        // The next ping is due one interval after the previous one started, and the
        // interval depends on whether this link is the active one. If it becomes the
        // active one while waiting, the wait is cut short. A probe slower than the
        // interval is followed by the next one right away, never by a burst.
        loop {
            let is_active = active.borrow_and_update().active == Some(link.kind);
            let every = if is_active {
                config.active_ping_interval
            } else {
                config.ping_interval
            };
            let Some(due) = last_start.map(|s| s + every) else {
                break;
            };
            tokio::select! {
                () = time::sleep_until(due) => break,
                changed = active.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
        last_start = Some(Instant::now());
        let up = match probe_once(&*prober, link, &mut session, &config, &active).await {
            Ok(up) => {
                if !was_up {
                    tracing::info!(link = %link.kind, addr = %link.addr, "healthy link");
                }
                Some(up)
            }
            Err(failure) => {
                if was_up {
                    tracing::warn!(link = %link.kind, addr = %link.addr, "link down: {failure}");
                } else {
                    tracing::debug!(link = %link.kind, addr = %link.addr, "link down: {failure}");
                }
                None
            }
        };
        was_up = up.is_some();
        if reports.send((link.kind, up)).is_err() {
            return;
        }
    }
}

async fn probe_once<P: Prober>(
    prober: &P,
    link: LinkCandidate,
    session: &mut Option<P::Session>,
    config: &HealthConfig,
    cadence: &watch::Receiver<Cadence>,
) -> Result<(P::Handle, Duration), Failure<P::Error>> {
    let addr = link.addr;
    let s = match session {
        Some(s) => s,
        None => match time::timeout(config.connect_timeout, prober.connect(addr)).await {
            Ok(Ok(s)) => session.insert(s),
            Ok(Err(e)) => return Err(Failure::Connect(e)),
            Err(_) => return Err(Failure::ConnectTimeout),
        },
    };
    let start = Instant::now();
    let result = {
        let ping = prober.ping(s);
        tokio::pin!(ping);
        let mut misses = 0;
        loop {
            match time::timeout(config.ping_timeout, &mut ping).await {
                Ok(Ok(())) => break Ok(()),
                // An error (the connection closed, for example) is a drop right away.
                Ok(Err(e)) => break Err(Failure::Ping(e)),
                Err(_) => {
                    misses += 1;
                    // Looked at on every window: a backup that turns healthy while
                    // the active one stalls ends the extra patience.
                    let c = *cadence.borrow();
                    let lone = c.active == Some(link.kind) && !c.backup_ready;
                    let limit = if lone {
                        config.lone_patience()
                    } else {
                        config.drop_after()
                    };
                    if start.elapsed() >= limit {
                        break Err(Failure::PingTimeout);
                    }
                    tracing::debug!(%addr, misses, "missed probe, still waiting for the Pong");
                }
            }
        }
    };
    match result {
        Ok(()) => Ok((P::handle(s), start.elapsed())),
        Err(failure) => {
            // A ping cut off halfway may leave the stream misaligned: the
            // session is discarded and the next ping connects again.
            *session = None;
            Err(failure)
        }
    }
}

#[cfg(test)]
mod tests;
