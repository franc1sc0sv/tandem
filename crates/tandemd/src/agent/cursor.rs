//! The live cursor in the agent (T-128): position and crossings for the UI, only while
//! someone is watching.
//!
//! The input engine ([`crate::input`]) publishes samples (at 30 Hz at most) and crossings
//! when [`CursorGate`] is on, and the agent distributes them: to the UI connections
//! here that asked for [`tandem_proto::UiCommand::WatchCursor`] and, over the control plane,
//! to the other machine if it watches. Samples come from whoever has focus, so the UI
//! of the other machine receives them from here ([`ControlMsg::Cursor`]); that path discards what
//! lags ([`PeerLink::send_lossy`](crate::orchestrator::PeerLink::send_lossy)).
//!
//! The switch turns on with the first subscription (from here or from the other machine) and
//! turns off with the last. The other machine's request is a lease, like the resources one
//! ([`super::env`]): if it stops being renewed (the link dropped), it turns off on its own.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use tandem_input::session::{CursorEvent, CursorGate};
use tandem_proto::ControlMsg;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::Agent;
use super::env::{LEASE, RENEW};

/// How many notices a slow UI connection waits before starting to lose the
/// old ones.
const BACKLOG: usize = 64;

pub(super) struct CursorState {
    gate: Arc<CursorGate>,
    /// What is published here and what the other machine sends, for the UI connections.
    events: broadcast::Sender<CursorEvent>,
    local: Mutex<Local>,
    serving: Mutex<Serving>,
    /// How many events came out of the input engine (tests look at it).
    published: AtomicU64,
}

#[derive(Default)]
struct Local {
    /// UI connections here that are watching, and the task that renews the request to the other.
    watchers: usize,
    renew: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Serving {
    /// Until when the other machine wants this one's cursor.
    until: Option<Instant>,
    running: bool,
}

impl std::fmt::Debug for CursorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CursorState")
            .field("gate", &self.gate)
            .finish_non_exhaustive()
    }
}

impl Default for CursorState {
    fn default() -> Self {
        CursorState {
            gate: Arc::new(CursorGate::default()),
            events: broadcast::channel(BACKLOG).0,
            local: Mutex::new(Local::default()),
            serving: Mutex::new(Serving::default()),
            published: AtomicU64::new(0),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Agent {
    /// The switch the input engine reads: on while there is someone watching the
    /// cursor, from here or from the other machine.
    pub fn cursor_gate(&self) -> Arc<CursorGate> {
        self.cursor.gate.clone()
    }

    /// How many cursor events the input engine published since the agent started.
    pub fn cursor_published(&self) -> u64 {
        self.cursor.published.load(Ordering::Relaxed)
    }

    fn peer_watches_cursor(&self) -> bool {
        lock(&self.cursor.serving)
            .until
            .is_some_and(|until| Instant::now() < until)
    }

    /// Turns the switch on or off depending on whether anyone is watching.
    fn refresh_cursor_gate(&self) {
        let local = lock(&self.cursor.local).watchers > 0;
        self.cursor.gate.set(local || self.peer_watches_cursor());
    }

    /// A sample or a crossing of this input engine: it goes to the connections here and, if
    /// the other machine watches, to it.
    pub fn publish_cursor(&self, event: CursorEvent) {
        self.cursor.published.fetch_add(1, Ordering::Relaxed);
        if self.peer_watches_cursor() {
            match &event {
                CursorEvent::Sample(sample) => self
                    .orch
                    .peer()
                    .send_lossy(ControlMsg::Cursor(sample.clone())),
                // A crossing is rare and the UI cannot lose it: it goes over the usual path.
                CursorEvent::Crossed { from, to, edge } => {
                    self.orch.peer().send(ControlMsg::Crossed {
                        from: *from,
                        to: *to,
                        edge: *edge,
                    });
                }
            }
        }
        // Without UI connections it is not an error.
        let _ = self.cursor.events.send(event);
    }

    /// A sample or a crossing the other machine sent, for whoever watches here.
    pub fn receive_peer_cursor(&self, event: CursorEvent) {
        let _ = self.cursor.events.send(event);
    }

    /// Start receiving the cursor of both machines. While any exists the switch
    /// is turned on here and the other machine is asked (and the request renewed).
    pub fn watch_cursor(self: &Arc<Self>) -> CursorWatch {
        let events = self.cursor.events.subscribe();
        let mut l = lock(&self.cursor.local);
        l.watchers += 1;
        if l.watchers == 1
            && let Ok(rt) = tokio::runtime::Handle::try_current()
        {
            let weak = Arc::downgrade(self);
            l.renew = Some(rt.spawn(async move {
                loop {
                    match weak.upgrade() {
                        Some(a) => a.orch.peer().send(ControlMsg::WatchCursor { on: true }),
                        None => return,
                    }
                    tokio::time::sleep(RENEW).await;
                }
            }));
        }
        drop(l);
        self.refresh_cursor_gate();
        CursorWatch {
            agent: Arc::downgrade(self),
            events,
        }
    }

    fn unwatch_cursor(&self) {
        let mut l = lock(&self.cursor.local);
        l.watchers = l.watchers.saturating_sub(1);
        let last = l.watchers == 0;
        if last && let Some(task) = l.renew.take() {
            task.abort();
        }
        drop(l);
        self.refresh_cursor_gate();
        if last {
            self.orch.peer().send(ControlMsg::WatchCursor { on: false });
        }
    }

    /// The other machine wants (or no longer wants) this one's cursor.
    pub fn receive_watch_cursor(self: &Arc<Self>, on: bool) {
        let mut s = lock(&self.cursor.serving);
        if !on {
            s.until = None;
            drop(s);
            self.refresh_cursor_gate();
            return;
        }
        s.until = Some(Instant::now() + LEASE);
        let start = !std::mem::replace(&mut s.running, true);
        drop(s);
        self.refresh_cursor_gate();
        if !start {
            return;
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            lock(&self.cursor.serving).running = false;
            return;
        };
        rt.spawn(lease(Arc::downgrade(self)));
    }
}

/// What to do with the other machine's lease at this instant.
enum Lease {
    /// Still valid until that moment.
    Until(Instant),
    /// It expired and is already cleaned.
    Expired,
}

/// Decides and cleans in a single critical section: if it decided and cleaned separately, a
/// renewal between the two steps would be erased and the switch would stay off until the
/// next one (and `running` false would leave it without a task to watch it).
fn settle(serving: &Mutex<Serving>, now: Instant) -> Lease {
    let mut s = lock(serving);
    match s.until {
        Some(until) if now < until => Lease::Until(until),
        _ => {
            s.until = None;
            s.running = false;
            Lease::Expired
        }
    }
}

/// Watches the other machine's lease and turns the switch off when it expires unrenewed.
async fn lease(weak: Weak<Agent>) {
    loop {
        let Some(agent) = weak.upgrade() else { return };
        match settle(&agent.cursor.serving, Instant::now()) {
            Lease::Until(until) => {
                drop(agent);
                tokio::time::sleep_until(until).await;
            }
            Lease::Expired => {
                agent.refresh_cursor_gate();
                return;
            }
        }
    }
}

/// The cursor of both machines for a UI connection. Dropping it stops watching.
pub struct CursorWatch {
    agent: Weak<Agent>,
    events: broadcast::Receiver<CursorEvent>,
}

impl std::fmt::Debug for CursorWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CursorWatch")
    }
}

impl CursorWatch {
    /// The next event; `None` if the agent no longer exists. If the connection lagged,
    /// what was lost is discarded: the next sample already says where the cursor is. It can
    /// be canceled without losing events.
    pub async fn next(&mut self) -> Option<CursorEvent> {
        loop {
            match self.events.recv().await {
                Ok(event) => return Some(event),
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return None,
            }
        }
    }
}

impl Drop for CursorWatch {
    fn drop(&mut self) {
        if let Some(agent) = self.agent.upgrade() {
            agent.unwatch_cursor();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use tandem_core::{Machine, Os, Side, State, Step};
    use tandem_proto::CursorSample;

    use super::*;
    use crate::StateStore;
    use crate::config::fixtures;
    use crate::orchestrator::{PeerLink, StepFuture};

    /// Records what the agent sends to the other machine, and over which path.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<(bool, ControlMsg)>>);

    impl PeerLink for Recorder {
        fn execute<'a>(&'a self, _step: &'a Step) -> StepFuture<'a> {
            Box::pin(async { Ok(()) })
        }
        fn publish_state(&self, _state: &State) {}
        fn send(&self, msg: ControlMsg) {
            self.0.lock().unwrap().push((false, msg));
        }
        fn send_lossy(&self, msg: ControlMsg) {
            self.0.lock().unwrap().push((true, msg));
        }
    }

    impl Recorder {
        fn taken(&self) -> Vec<(bool, ControlMsg)> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    fn agent(dir: &std::path::Path) -> (Arc<Agent>, Arc<Recorder>) {
        let mut cfg = fixtures::desk();
        cfg.os = Os::MacOs;
        let a = Arc::new(Agent::new(cfg, StateStore::new(dir.join("state.json"))));
        let peer = Arc::new(Recorder::default());
        a.set_peer(peer.clone());
        peer.taken();
        (a, peer)
    }

    fn sample_of(x: i32) -> CursorSample {
        CursorSample {
            machine: Machine::A,
            screen: "mac-1".into(),
            x,
            y: 0,
        }
    }

    fn sample(x: i32) -> CursorEvent {
        CursorEvent::Sample(sample_of(x))
    }

    fn crossed() -> CursorEvent {
        CursorEvent::Crossed {
            from: Machine::A,
            to: Machine::B,
            edge: Side::Right,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn settle_keeps_a_renewed_lease_and_clears_an_expired_one_atomically() {
        let serving = Mutex::new(Serving {
            until: Some(Instant::now() + Duration::from_secs(5)),
            running: true,
        });
        // Renewed: neither `until` is cleaned nor `running` lowered.
        let Lease::Until(until) = settle(&serving, Instant::now()) else {
            panic!("the lease was still valid");
        };
        assert!(lock(&serving).running);
        // Expired: everything is cleaned in the same call, without leaving `running` half done.
        tokio::time::advance(Duration::from_secs(6)).await;
        assert!(matches!(settle(&serving, Instant::now()), Lease::Expired));
        let s = lock(&serving);
        assert!(s.until.is_none() && !s.running);
        assert!(until > Instant::now() - Duration::from_secs(6));
    }

    /// The real race (renewing between reading and cleaning) only happens between threads; here
    /// `settle` is punished against concurrent renewals: it must never delete a valid one.
    #[test]
    fn a_renewal_is_never_wiped_by_a_concurrent_settle() {
        let base = Instant::now();
        let far = std::time::Duration::from_secs(3600);
        for _ in 0..200 {
            let serving = Arc::new(Mutex::new(Serving {
                until: Some(base),
                running: true,
            }));
            let renewer = {
                let serving = serving.clone();
                std::thread::spawn(move || {
                    let mut s = lock(&serving);
                    s.until = Some(base + far);
                    s.running = true;
                })
            };
            let verdict = settle(&serving, base + Duration::from_secs(1));
            renewer.join().unwrap();
            let s = lock(&serving);
            // Either it saw the renewal (still valid) or it expired earlier and the renewal arrived
            // afterwards: in both cases the renewal survives.
            assert!(
                s.until.is_some() && s.running,
                "{}",
                matches!(verdict, Lease::Expired)
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_gate_follows_the_local_watchers_and_the_other_machine_is_asked() {
        let dir = tempfile::tempdir().unwrap();
        let (a, peer) = agent(dir.path());
        let gate = a.cursor_gate();
        assert!(!gate.is_on());

        let first = a.watch_cursor();
        let second = a.watch_cursor();
        assert!(gate.is_on());
        tokio::task::yield_now().await;
        // The request to the other machine goes out once and is renewed while someone watches.
        assert_eq!(
            peer.taken(),
            vec![(false, ControlMsg::WatchCursor { on: true })]
        );
        tokio::time::sleep(RENEW + Duration::from_millis(10)).await;
        assert_eq!(
            peer.taken(),
            vec![(false, ControlMsg::WatchCursor { on: true })]
        );

        // With one connection leaving, there is still someone watching.
        drop(first);
        assert!(gate.is_on());
        assert!(peer.taken().is_empty());
        drop(second);
        assert!(!gate.is_on());
        assert_eq!(
            peer.taken(),
            vec![(false, ControlMsg::WatchCursor { on: false })]
        );
        // It is no longer renewed.
        tokio::time::sleep(RENEW * 3).await;
        assert!(peer.taken().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn the_other_machine_rents_the_cursor_and_the_rent_expires() {
        let dir = tempfile::tempdir().unwrap();
        let (a, peer) = agent(dir.path());
        let gate = a.cursor_gate();

        // Nobody watches: what the engine publishes goes nowhere.
        a.publish_cursor(sample(1));
        assert!(peer.taken().is_empty());

        a.receive_watch_cursor(true);
        assert!(gate.is_on());
        a.publish_cursor(sample(2));
        a.publish_cursor(crossed());
        // Samples go over the path that discards; the crossing, over the normal one.
        assert_eq!(
            peer.taken(),
            vec![
                (true, ControlMsg::Cursor(sample_of(2))),
                (
                    false,
                    ControlMsg::Crossed {
                        from: Machine::A,
                        to: Machine::B,
                        edge: Side::Right
                    }
                ),
            ]
        );

        // Renewed in time, it continues; without renewing, it expires and the switch turns off on its own.
        tokio::time::sleep(LEASE - Duration::from_secs(1)).await;
        a.receive_watch_cursor(true);
        tokio::time::sleep(LEASE - Duration::from_secs(1)).await;
        assert!(gate.is_on(), "it was renewed");
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(!gate.is_on(), "it expired without renewal");
        a.publish_cursor(sample(3));
        assert!(peer.taken().is_empty());

        // And it can be canceled by hand.
        a.receive_watch_cursor(true);
        assert!(gate.is_on());
        a.receive_watch_cursor(false);
        assert!(!gate.is_on());
    }

    #[tokio::test(start_paused = true)]
    async fn local_watchers_get_both_machines_events_and_lost_ones_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let (a, _peer) = agent(dir.path());
        let mut watch = a.watch_cursor();

        a.publish_cursor(sample(1));
        a.receive_peer_cursor(crossed());
        assert_eq!(watch.next().await, Some(sample(1)));
        assert_eq!(watch.next().await, Some(crossed()));
        assert_eq!(a.cursor_published(), 1);

        // A connection that does not read lags: it loses the old and continues with the new.
        for x in 0..(BACKLOG as i32 + 10) {
            a.receive_peer_cursor(sample(x));
        }
        let Some(CursorEvent::Sample(first)) = watch.next().await else {
            panic!("a sample was expected");
        };
        assert!(first.x >= 10, "the oldest were discarded: {first:?}");
    }
}
