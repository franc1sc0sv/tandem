//! Multiple-link manager and plane policy (DESIGN §6).
//!
//! Each configured link (USB4, Ethernet, Wi-Fi) has its own connection
//! to the other agent and a health ping every second. The active link is
//! the healthy one of highest priority: if it drops, it moves to the next as soon as a
//! single ping fails (without waiting for QUIC's idle timeout). To go back to a
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
use tokio::time::{self, Instant, MissedTickBehavior};

pub use kind::{LinkCandidate, LinkKind};
pub use probe::Prober;
pub use quic::{QuicProber, QuicSession, answer_pings};
pub use table::{LinkEvent, LinkStatus};
// `LinkControl` and `ManagerGone` are defined below.

use table::LinkTable;

/// Health check timings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthConfig {
    /// How often each link is pinged (DESIGN §6: 1 s).
    pub ping_interval: Duration,
    /// How long to wait for the Pong. `ping_interval + ping_timeout` is the most
    /// it takes to detect a drop, and it has to stay under 1.5 s.
    pub ping_timeout: Duration,
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
            ping_timeout: Duration::from_millis(250),
            connect_timeout: Duration::from_secs(1),
            settle: Duration::from_secs(3),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManagerError {
    #[error("no link is configured")]
    NoCandidates,
    #[error("the link {0} is configured twice")]
    DuplicateLink(LinkKind),
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
            config.ping_timeout,
            config.connect_timeout,
        ]
        .contains(&Duration::ZERO)
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

        let prober = Arc::new(prober);
        let mut links = JoinSet::new();
        let mut pingers: BTreeMap<LinkKind, AbortHandle> = BTreeMap::new();
        for c in table.candidates() {
            let kind = c.kind;
            let handle = links.spawn(run_link(prober.clone(), c, config, report_tx.clone()));
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
) {
    let mut tick = time::interval(config.ping_interval);
    // After a slow connection, late pings are not chained.
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut session = None;
    let mut was_up = false;
    loop {
        tick.tick().await;
        let up = match probe_once(&*prober, link.addr, &mut session, &config).await {
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
    addr: SocketAddr,
    session: &mut Option<P::Session>,
    config: &HealthConfig,
) -> Result<(P::Handle, Duration), Failure<P::Error>> {
    let s = match session {
        Some(s) => s,
        None => match time::timeout(config.connect_timeout, prober.connect(addr)).await {
            Ok(Ok(s)) => session.insert(s),
            Ok(Err(e)) => return Err(Failure::Connect(e)),
            Err(_) => return Err(Failure::ConnectTimeout),
        },
    };
    let start = Instant::now();
    let result = match time::timeout(config.ping_timeout, prober.ping(s)).await {
        Ok(Ok(())) => return Ok((P::handle(s), start.elapsed())),
        Ok(Err(e)) => Failure::Ping(e),
        Err(_) => Failure::PingTimeout,
    };
    // A ping cut off halfway may leave the stream misaligned: the
    // session is discarded and the next tick connects again.
    *session = None;
    Err(result)
}

#[cfg(test)]
mod tests;
