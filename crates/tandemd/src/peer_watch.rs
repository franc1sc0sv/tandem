//! Reconciles the peer's power state with the links, on both machines (DESIGN §3.7).
//!
//! If the other machine loses power (or suspends) nobody runs its `PowerOff`:
//! without this, this agent would keep believing it on and no plan would send the
//! Wake-on-LAN again. When no link sees it for [`crate::PowerConfig::peer_lost_after`],
//! it is considered off: `on` marks it off and the UI receives [`PowerEvent::PeerLost`] with
//! the reason. The input engine removes its screens from the geometry as soon as the state
//! changes, so the cursor does not go to powered-off screens and, without the other, the keyboard and
//! mouse stay here. A shorter drop changes nothing.
//!
//! Conversely, if the other is seen again while marked off (it was turned on by hand, it
//! woke up, or this agent started without knowing), it is considered on.
//!
//! None of this tramples a transaction: the orchestrator is taken like any other change, so
//! a shutdown in progress finishes first (and leaves it off itself).

use std::sync::Arc;
use std::time::Duration;

use tandem_proto::PowerEvent;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::agent::Agent;

/// How often to retry if the orchestrator is busy with a change.
pub const BUSY_RETRY: Duration = Duration::from_secs(1);

/// Watches the links to the other machine until `reachable` or the agent closes.
///
/// `reachable` is true while some link sees the other's agent (in the agent,
/// the link manager's active link).
pub async fn watch(agent: Arc<Agent>, mut reachable: watch::Receiver<bool>) {
    let peer = agent.machine().other();
    let lost_after = agent.config().power.peer_lost_after();
    let mut states = agent.subscribe();
    let mut up = *reachable.borrow_and_update();
    let mut down_since = (!up).then(Instant::now);
    // The other was seen appearing and it was not yet checked whether it was on.
    let mut appeared = up;
    loop {
        let peer_on = agent.state().is_on(peer);
        let mut wake = None;
        if up {
            if appeared && !peer_on {
                if mark_on(&agent) {
                    appeared = false;
                    continue;
                }
                wake = Some(Instant::now() + BUSY_RETRY);
            } else {
                appeared = false;
            }
        } else if let Some(since) = down_since
            && peer_on
        {
            let due = since + lost_after;
            if Instant::now() >= due {
                if mark_lost(&agent, since) {
                    continue;
                }
                wake = Some(Instant::now() + BUSY_RETRY);
            } else {
                wake = Some(due);
            }
        }
        // The link first: if it returns at the same time as a state arrives that considers it on (the
        // end of a power on), the state cannot be seen with the old link, down since
        // before the threshold (review of #82).
        tokio::select! {
            biased;
            changed = reachable.changed() => {
                if changed.is_err() {
                    return;
                }
                let now_up = *reachable.borrow_and_update();
                if now_up != up {
                    up = now_up;
                    down_since = (!up).then(Instant::now);
                    appeared = up;
                }
            }
            changed = states.changed() => {
                if changed.is_err() {
                    return;
                }
                states.mark_unchanged();
            }
            () = sleep_until(wake), if wake.is_some() => {}
        }
    }
}

async fn sleep_until(at: Option<Instant>) {
    if let Some(at) = at {
        tokio::time::sleep_until(at).await;
    }
}

/// Releases the orchestrator when done, even if something panics.
struct Held<'a>(&'a Agent);

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.0.orchestrator().end();
    }
}

/// Considers the other off. `false` if the orchestrator is busy with a change.
fn mark_lost(agent: &Agent, since: Instant) -> bool {
    if agent.orchestrator().try_begin().is_none() {
        return false;
    }
    let held = Held(agent);
    let peer = agent.machine().other();
    let mut state = agent.state();
    // The change that finished while waiting may have turned it off.
    if !state.is_on(peer) {
        return true;
    }
    state.on.set(peer, false);
    let secs = since.elapsed().as_secs();
    let name = agent.machine_name(peer);
    let reason = format!(
        "The {name} has not responded on any link for {secs} s without anyone turning it off \
         (power cut or suspended?): it is considered off"
    );
    tracing::warn!("{reason}");
    // First the state, then the notice: whoever receives it already sees it off.
    if let Err(e) = agent.set_state(state) {
        tracing::warn!("{e}");
    }
    agent.emit_power(PowerEvent::PeerLost {
        machine: peer,
        reason,
    });
    drop(held);
    true
}

/// Considers the other on. `false` if a change is in progress: it lets it finish.
///
/// It does not reserve the orchestrator: the other just appeared and may already be asking for its
/// first step, which with the orchestrator reserved would be rejected.
fn mark_on(agent: &Agent) -> bool {
    if !agent.orchestrator().is_idle() {
        return false;
    }
    let peer = agent.machine().other();
    let mut state = agent.state();
    if !state.is_on(peer) {
        tracing::info!("{peer} responded again: considered on");
        state.on.set(peer, true);
        if let Err(e) = agent.set_state(state) {
            tracing::warn!("{e}");
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tandem_core::{Machine, State};
    use tokio::sync::broadcast;

    use super::*;
    use crate::config::fixtures;
    use crate::orchestrator::tests::{FakePeer, FakeSteps};
    use crate::store::StateStore;

    const LOST_AFTER: Duration = Duration::from_secs(15);
    const BOTH: [Machine; 2] = [Machine::A, Machine::B];

    struct Rig {
        agent: Arc<Agent>,
        local: Arc<FakeSteps>,
        peer: Arc<FakeSteps>,
        up: watch::Sender<bool>,
        events: broadcast::Receiver<PowerEvent>,
        task: tokio::task::JoinHandle<()>,
    }

    /// The agent of `me`, with the other on or not and visible according to `up`. The same tests hold from both
    /// machines: there is no one that watches and one that is watched.
    fn rig(dir: &Path, me: Machine, peer_on: bool, up: bool) -> Rig {
        let mut cfg = fixtures::desk();
        cfg.machine = me;
        let local = Arc::new(FakeSteps::default());
        let peer = Arc::new(FakeSteps::default());
        let agent =
            Agent::with_local_steps(cfg, StateStore::new(dir.join("state.json")), local.clone());
        agent.set_peer(Arc::new(FakePeer(peer.clone())));
        let mut s = agent.state();
        s.on.set(me.other(), peer_on);
        agent.set_state(s).unwrap();
        let agent = Arc::new(agent);
        let (up, rx) = watch::channel(up);
        let events = agent.subscribe_power();
        let task = tokio::spawn(watch(agent.clone(), rx));
        Rig {
            agent,
            local,
            peer,
            up,
            events,
            task,
        }
    }

    fn peer_on(r: &Rig) -> bool {
        r.agent.state().is_on(r.agent.machine().other())
    }

    async fn sleep(secs: f64) {
        tokio::time::sleep(Duration::from_secs_f64(secs)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_short_drop_changes_nothing() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let mut r = rig(dir.path(), me, true, true);
            let before = r.agent.state();
            for _ in 0..5 {
                r.up.send_replace(false);
                sleep(LOST_AFTER.as_secs_f64() - 1.0).await;
                r.up.send_replace(true);
                sleep(2.0).await;
            }
            sleep(60.0).await;
            assert_eq!(r.agent.state(), before, "{me}");
            assert!(r.events.try_recv().is_err());
            assert!(r.local.steps().is_empty());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn after_the_threshold_the_peer_is_off_and_the_ui_is_told_why() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let mut r = rig(dir.path(), me, true, true);
            let before = r.agent.state();
            r.up.send_replace(false);
            sleep(LOST_AFTER.as_secs_f64() - 0.1).await;
            assert!(peer_on(&r), "{me}: still inside the threshold");
            sleep(0.2).await;

            let mut want = before.clone();
            want.on.set(me.other(), false);
            assert_eq!(r.agent.state(), want, "{me}");
            assert!(r.agent.state().is_on(me), "{me}: never off for itself");
            let PowerEvent::PeerLost { machine, reason } = r.events.try_recv().unwrap();
            assert_eq!(machine, me.other());
            assert!(reason.contains("15 s"), "{reason}");
            assert!(
                reason.starts_with(&format!(
                    "The {} has not responded",
                    r.agent.machine_name(me.other())
                )),
                "{reason}"
            );
            // No step: neither here nor to the other.
            assert!(r.local.steps().is_empty());
            assert!(r.peer.steps().is_empty());
            // And it is saved.
            let saved = StateStore::new(dir.path().join("state.json"))
                .load()
                .unwrap();
            assert_eq!(saved, Some(want));
            // The orchestrator was left free.
            assert!(r.agent.orchestrator().try_begin().is_some());

            // It is still not seen: the notice is not repeated.
            r.agent.orchestrator().end();
            sleep(120.0).await;
            assert!(r.events.try_recv().is_err());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_change_in_progress_is_not_interrupted() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let mut r = rig(dir.path(), me, true, true);
            r.agent.orchestrator().try_begin().unwrap();
            r.up.send_replace(false);
            sleep(LOST_AFTER.as_secs_f64() + 30.0).await;
            assert!(peer_on(&r));
            assert!(r.events.try_recv().is_err());
            r.agent.orchestrator().end();
            sleep(BUSY_RETRY.as_secs_f64() + 0.1).await;
            assert!(!peer_on(&r), "{me}");
            assert!(r.events.try_recv().is_ok());
        }
    }

    /// Review of #82: a transaction that powers it on ends just when its link
    /// returns. Both notices arrive at once; with the new state seen before the link,
    /// the watch considered it lost (with the link down since before the threshold).
    #[tokio::test(start_paused = true)]
    async fn the_link_coming_back_wins_over_the_state_that_powers_it_on() {
        // `select!` picks at random between two ready notices: several rounds make it certain.
        for _ in 0..16 {
            let dir = tempfile::tempdir().unwrap();
            let mut r = rig(dir.path(), Machine::A, false, false);
            r.agent.orchestrator().try_begin().unwrap();
            sleep(LOST_AFTER.as_secs_f64() + 30.0).await;
            // The transaction turned it on: first the link appears, then the state.
            r.up.send_replace(true);
            let mut s = r.agent.state();
            s.on.b = true;
            r.agent.set_state(s).unwrap();
            r.agent.orchestrator().end();
            sleep(BUSY_RETRY.as_secs_f64() * 3.0).await;
            assert!(peer_on(&r));
            assert!(r.events.try_recv().is_err());
        }
    }

    /// A shutdown in progress leaves it off itself: there is no cut to notify.
    #[tokio::test(start_paused = true)]
    async fn a_shutdown_in_progress_is_not_a_power_cut() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let mut r = rig(dir.path(), me, true, true);
            r.agent.orchestrator().try_begin().unwrap();
            r.up.send_replace(false);
            sleep(LOST_AFTER.as_secs_f64() + 5.0).await;
            let mut s = r.agent.state();
            s.on.set(me.other(), false);
            r.agent.set_state(s.clone()).unwrap();
            r.agent.orchestrator().end();
            sleep(60.0).await;
            assert_eq!(r.agent.state(), s);
            assert!(r.events.try_recv().is_err());
            assert!(r.local.steps().is_empty());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn with_the_peer_already_off_nothing_happens() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let mut r = rig(dir.path(), me, false, false);
            let before = r.agent.state();
            sleep(120.0).await;
            assert_eq!(r.agent.state(), before);
            assert!(r.events.try_recv().is_err());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_peer_coming_back_is_on_again() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let mut r = rig(dir.path(), me, true, true);
            r.up.send_replace(false);
            sleep(LOST_AFTER.as_secs_f64() + 1.0).await;
            assert!(!peer_on(&r));
            r.events.try_recv().unwrap();

            // It was turned on by hand (or woke up).
            r.up.send_replace(true);
            sleep(0.1).await;
            let s = r.agent.state();
            assert!(s.is_on(me.other()), "{me}");
            assert!(r.events.try_recv().is_err());
        }
    }

    /// The agent starts with the initial state (the other off) and the other was already on.
    #[tokio::test(start_paused = true)]
    async fn a_peer_seen_at_startup_is_on() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let mut cfg = fixtures::desk();
            cfg.machine = me;
            let agent = Arc::new(Agent::new(
                cfg,
                StateStore::new(dir.path().join("state.json")),
            ));
            let (_up, rx) = watch::channel(true);
            let task = tokio::spawn(watch(agent.clone(), rx));
            sleep(0.1).await;
            let mut want = State::initial(me);
            want.on.set(me.other(), true);
            assert_eq!(agent.state(), want, "{me}");
            task.abort();
        }
    }

    /// If the other appears in the middle of a change (powering it on), it waits for it to finish.
    #[tokio::test(start_paused = true)]
    async fn the_peer_appearing_during_a_change_waits_for_it() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let r = rig(dir.path(), me, false, false);
            r.agent.orchestrator().try_begin().unwrap();
            r.up.send_replace(true);
            sleep(5.0).await;
            assert!(!peer_on(&r));
            r.agent.orchestrator().end();
            sleep(BUSY_RETRY.as_secs_f64() + 0.1).await;
            assert!(peer_on(&r), "{me}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn ends_when_the_link_source_goes_away() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let r = rig(dir.path(), me, true, true);
            drop(r.up);
            r.task.await.unwrap();
        }
    }
}
