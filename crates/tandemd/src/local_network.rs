//! The macOS Local Network permission of `tandemd` (T-344).
//!
//! macOS has no API that reads it (Apple TN3179). What it does: the first local network
//! operation of a program whose permission is undetermined shows the alert, and while the
//! permission is undetermined or denied a BSD socket send to a local address fails with
//! `EHOSTUNREACH`, the same errno in both cases. So the agent tells them apart by time and
//! history: a block right after a successful send means the user turned it off, and a block
//! that lasts more than [`Times::grace`] means the alert was denied or left unanswered.
//!
//! The probe is a zero-length UDP datagram to the discard port of the mDNS multicast group:
//! every multicast address is a local network address, and the send needs no ARP. The agent
//! keeps running after it, which matters: macOS does not show the alert for a process that
//! exits right after the blocked operation (TN3179). The findings and sources are in
//! `docs/research/macos-local-network.md`.

use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::{Arc, Weak};
use std::time::Duration;

use tandem_proto::LocalNetwork;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::agent::Agent;

/// Where the probe sends: the discard port (9) of the mDNS group, which Tandem already uses.
pub const TARGET: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 251), 9);

/// What one try at the local network did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attempt {
    /// The datagram left: the process has local network access.
    Reached,
    /// `EHOSTUNREACH`: what macOS answers while the permission is undetermined or denied.
    Blocked,
    /// No route to the local network (no interface up): nothing can be told.
    NoNetwork,
    /// Any other error, with its text: nothing can be told either.
    Failed(String),
}

/// What a send's result means for the permission.
pub fn classify(result: io::Result<()>) -> Attempt {
    match result {
        Ok(()) => Attempt::Reached,
        Err(e) => match e.kind() {
            io::ErrorKind::HostUnreachable => Attempt::Blocked,
            io::ErrorKind::NetworkUnreachable | io::ErrorKind::AddrNotAvailable => {
                Attempt::NoNetwork
            }
            _ => Attempt::Failed(e.to_string()),
        },
    }
}

/// One try at the local network. Behind a trait so the state machine and the task are tested
/// without the system's network.
pub trait Probe: Send + Sync {
    fn attempt(&self) -> Attempt;
}

/// The real probe: a UDP socket connected to [`TARGET`] that sends nothing but a header. The
/// calls return at once (UDP does not wait for anyone), so it runs on the task's thread.
#[derive(Debug, Default, Clone, Copy)]
pub struct UdpProbe;

impl Probe for UdpProbe {
    fn attempt(&self) -> Attempt {
        classify(send_once(TARGET))
    }
}

fn send_once(to: SocketAddrV4) -> io::Result<()> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    // Connecting is what TN3179 uses to bring up the alert; the send tells the result.
    socket.connect(to)?;
    socket.send(&[]).map(|_| ())
}

/// The permission from the history of attempts.
#[derive(Debug, Clone)]
pub struct Tracker {
    state: LocalNetwork,
    /// When the current run of blocked attempts started.
    blocked_since: Option<Instant>,
    grace: Duration,
}

impl Tracker {
    pub fn new(grace: Duration) -> Self {
        Tracker {
            state: LocalNetwork::Unknown,
            blocked_since: None,
            grace,
        }
    }

    pub fn state(&self) -> LocalNetwork {
        self.state
    }

    /// Takes in one attempt made at `now` and returns the permission it leaves.
    pub fn observe(&mut self, attempt: &Attempt, now: Instant) -> LocalNetwork {
        self.state = match attempt {
            Attempt::Reached => {
                self.blocked_since = None;
                LocalNetwork::Granted
            }
            Attempt::Blocked => {
                let since = *self.blocked_since.get_or_insert(now);
                // After a successful send macOS never asks again: the user turned it off.
                if self.state == LocalNetwork::Granted
                    || now.saturating_duration_since(since) >= self.grace
                {
                    LocalNetwork::Denied
                } else {
                    // The alert may be on screen: macOS refuses before the user answers.
                    self.state
                }
            }
            Attempt::NoNetwork => {
                self.blocked_since = None;
                LocalNetwork::Unknown
            }
            // A one-off error says nothing about the permission.
            Attempt::Failed(_) => self.state,
        };
        self.state
    }
}

/// How often the probe runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Times {
    /// How long a run of blocks may last before it counts as denied: enough to read the
    /// alert and press a button.
    pub grace: Duration,
    /// Between attempts while it is not granted: a permission granted in System Settings is
    /// seen within this time, without restarting the agent.
    pub retry: Duration,
    /// Between attempts while it is granted, to notice it being turned off.
    pub recheck: Duration,
}

impl Default for Times {
    fn default() -> Self {
        Times {
            grace: Duration::from_secs(20),
            retry: Duration::from_secs(5),
            recheck: Duration::from_secs(60),
        }
    }
}

/// Probes the local network for as long as the agent lives, and publishes the permission in
/// its capabilities. [`tandem_proto::UiCommand::CheckLocalNetwork`] makes it try at once.
pub fn start(agent: &Arc<Agent>, probe: Arc<dyn Probe>, times: Times) -> JoinHandle<()> {
    let checks = agent.subscribe_local_network_checks();
    tokio::spawn(watch(Arc::downgrade(agent), probe, times, checks))
}

async fn watch(
    agent: Weak<Agent>,
    probe: Arc<dyn Probe>,
    times: Times,
    mut checks: tokio::sync::watch::Receiver<u64>,
) {
    checks.borrow_and_update();
    let mut tracker = Tracker::new(times.grace);
    loop {
        let attempt = probe.attempt();
        let before = tracker.state();
        let state = tracker.observe(&attempt, Instant::now());
        if state != before {
            match state {
                LocalNetwork::Denied => tracing::warn!(
                    "local network: macOS blocks it; allow tandemd in System Settings → \
                     Privacy & Security → Local Network"
                ),
                other => tracing::info!("local network: {other:?}"),
            }
        }
        if let Attempt::Failed(why) = &attempt {
            tracing::debug!("local network: the probe failed: {why}");
        }
        let Some(a) = agent.upgrade() else { return };
        a.set_local_network(state);
        drop(a);
        let wait = if state == LocalNetwork::Granted {
            times.recheck
        } else {
            times.retry
        };
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            changed = checks.changed() => {
                // The agent is gone.
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use proptest::prelude::*;
    use tandem_proto::{Reply, UiCommand};

    use super::*;

    const GRACE: Duration = Duration::from_secs(20);

    fn errno(kind: io::ErrorKind) -> io::Result<()> {
        Err(io::Error::from(kind))
    }

    #[test]
    fn the_send_result_is_read_as_the_permission_hints() {
        assert_eq!(classify(Ok(())), Attempt::Reached);
        assert_eq!(
            classify(errno(io::ErrorKind::HostUnreachable)),
            Attempt::Blocked
        );
        assert_eq!(
            classify(errno(io::ErrorKind::NetworkUnreachable)),
            Attempt::NoNetwork
        );
        assert_eq!(
            classify(errno(io::ErrorKind::AddrNotAvailable)),
            Attempt::NoNetwork
        );
        assert!(matches!(
            classify(errno(io::ErrorKind::PermissionDenied)),
            Attempt::Failed(_)
        ));
    }

    /// The errno macOS gives is `EHOSTUNREACH` (65 on macOS, 113 on Linux): the raw code
    /// must map to the same kind as the named one.
    #[cfg(unix)]
    #[test]
    fn the_raw_ehostunreach_is_a_block() {
        let e = io::Error::from_raw_os_error(libc_ehostunreach());
        assert_eq!(classify(Err(e)), Attempt::Blocked);
    }

    #[cfg(target_os = "macos")]
    fn libc_ehostunreach() -> i32 {
        65
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn libc_ehostunreach() -> i32 {
        113
    }

    #[test]
    fn the_target_is_a_multicast_local_network_address() {
        assert!(TARGET.ip().is_multicast());
        assert_eq!(TARGET.port(), 9);
    }

    /// Where there is no such permission (Linux CI), the real probe is never read as a block.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_real_probe_is_not_blocked_without_the_permission() {
        assert_ne!(UdpProbe.attempt(), Attempt::Blocked);
    }

    #[test]
    fn a_send_that_goes_through_is_granted() {
        let t0 = Instant::now();
        let mut t = Tracker::new(GRACE);
        assert_eq!(t.state(), LocalNetwork::Unknown);
        assert_eq!(t.observe(&Attempt::Reached, t0), LocalNetwork::Granted);
    }

    #[test]
    fn a_short_block_may_be_the_alert_on_screen() {
        let t0 = Instant::now();
        let mut t = Tracker::new(GRACE);
        assert_eq!(t.observe(&Attempt::Blocked, t0), LocalNetwork::Unknown);
        assert_eq!(
            t.observe(&Attempt::Blocked, t0 + GRACE - Duration::from_millis(1)),
            LocalNetwork::Unknown
        );
        // The user pressed "Allow".
        assert_eq!(
            t.observe(&Attempt::Reached, t0 + GRACE),
            LocalNetwork::Granted
        );
    }

    #[test]
    fn a_block_longer_than_the_grace_is_denied() {
        let t0 = Instant::now();
        let mut t = Tracker::new(GRACE);
        t.observe(&Attempt::Blocked, t0);
        assert_eq!(
            t.observe(&Attempt::Blocked, t0 + GRACE),
            LocalNetwork::Denied
        );
        // Granted later in System Settings.
        assert_eq!(
            t.observe(&Attempt::Reached, t0 + GRACE * 2),
            LocalNetwork::Granted
        );
    }

    #[test]
    fn a_block_after_a_grant_is_denied_at_once() {
        let t0 = Instant::now();
        let mut t = Tracker::new(GRACE);
        t.observe(&Attempt::Reached, t0);
        assert_eq!(
            t.observe(&Attempt::Blocked, t0 + Duration::from_secs(1)),
            LocalNetwork::Denied
        );
    }

    #[test]
    fn without_a_network_nothing_is_known_and_the_grace_starts_over() {
        let t0 = Instant::now();
        let mut t = Tracker::new(GRACE);
        t.observe(&Attempt::Blocked, t0);
        assert_eq!(
            t.observe(&Attempt::NoNetwork, t0 + GRACE),
            LocalNetwork::Unknown
        );
        assert_eq!(
            t.observe(&Attempt::Blocked, t0 + GRACE * 2),
            LocalNetwork::Unknown,
            "a new run of blocks gets its own grace"
        );
    }

    #[test]
    fn a_one_off_error_keeps_what_was_known() {
        let t0 = Instant::now();
        let mut t = Tracker::new(GRACE);
        t.observe(&Attempt::Reached, t0);
        assert_eq!(
            t.observe(&Attempt::Failed("EMFILE".into()), t0),
            LocalNetwork::Granted
        );
    }

    fn attempt() -> impl Strategy<Value = Attempt> {
        prop_oneof![
            Just(Attempt::Reached),
            Just(Attempt::Blocked),
            Just(Attempt::NoNetwork),
            Just(Attempt::Failed("x".into())),
        ]
    }

    proptest! {
        /// Granted only if the last telling attempt went through, and denied only after a
        /// block: an error or a missing network never invents a decision.
        #[test]
        fn the_state_follows_the_last_telling_attempt(
            steps in prop::collection::vec((attempt(), 0u64..30_000), 1..40)
        ) {
            let mut now = Instant::now();
            let mut t = Tracker::new(GRACE);
            let mut last_telling: Option<Attempt> = None;
            for (a, ms) in steps {
                now += Duration::from_millis(ms);
                let s = t.observe(&a, now);
                if !matches!(a, Attempt::Failed(_)) {
                    last_telling = Some(a.clone());
                }
                match s {
                    LocalNetwork::Granted => prop_assert_eq!(&last_telling, &Some(Attempt::Reached)),
                    LocalNetwork::Denied => prop_assert_eq!(&last_telling, &Some(Attempt::Blocked)),
                    LocalNetwork::Unknown => prop_assert_ne!(&last_telling, &Some(Attempt::Reached)),
                }
            }
        }
    }

    /// A double of the network: answers what the test queued and then repeats the last one.
    struct Scripted {
        answers: Mutex<VecDeque<Attempt>>,
        last: Mutex<Attempt>,
        calls: Mutex<u32>,
    }

    impl Scripted {
        fn new(answers: &[Attempt]) -> Arc<Self> {
            Arc::new(Scripted {
                answers: Mutex::new(answers.iter().cloned().collect()),
                last: Mutex::new(Attempt::NoNetwork),
                calls: Mutex::new(0),
            })
        }

        fn then(&self, a: Attempt) {
            self.answers.lock().unwrap().push_back(a);
        }

        fn calls(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    impl Probe for Scripted {
        fn attempt(&self) -> Attempt {
            *self.calls.lock().unwrap() += 1;
            let mut last = self.last.lock().unwrap();
            if let Some(a) = self.answers.lock().unwrap().pop_front() {
                *last = a;
            }
            last.clone()
        }
    }

    async fn settle() {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    fn announced(agent: &Agent) -> Option<LocalNetwork> {
        agent.capabilities().here.local_network
    }

    /// The agent announces the permission over IPC (capabilities): unknown while the alert may
    /// be up, denied after the grace, and granted as soon as a send goes through.
    #[tokio::test(start_paused = true)]
    async fn the_agent_announces_the_permission_it_probes() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(crate::agent::tests::agent(dir.path()));
        assert_eq!(announced(&agent), None, "nobody probed it yet");
        let probe = Scripted::new(&[Attempt::Blocked]);
        let task = start(&agent, probe.clone(), Times::default());
        settle().await;
        assert_eq!(announced(&agent), Some(LocalNetwork::Unknown));
        tokio::time::sleep(Duration::from_secs(21)).await;
        settle().await;
        assert_eq!(announced(&agent), Some(LocalNetwork::Denied));
        // Granted in System Settings: seen on the next retry.
        probe.then(Attempt::Reached);
        tokio::time::sleep(Duration::from_secs(5)).await;
        settle().await;
        assert_eq!(announced(&agent), Some(LocalNetwork::Granted));
        task.abort();
    }

    /// "Ask again" makes the agent try at once instead of waiting for the next retry.
    #[tokio::test(start_paused = true)]
    async fn asking_again_probes_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(crate::agent::tests::agent(dir.path()));
        assert!(
            matches!(
                agent.handle(UiCommand::CheckLocalNetwork),
                Reply::Rejected(_)
            ),
            "without a probe there is nothing to check"
        );
        let probe = Scripted::new(&[Attempt::Reached]);
        let task = start(&agent, probe.clone(), Times::default());
        settle().await;
        assert_eq!(probe.calls(), 1);
        assert_eq!(agent.handle(UiCommand::CheckLocalNetwork), Reply::Done);
        settle().await;
        assert_eq!(probe.calls(), 2, "no need to wait for the recheck");
        task.abort();
    }

    /// The task does not keep the agent alive and stops when it goes.
    #[tokio::test(start_paused = true)]
    async fn the_task_ends_with_the_agent() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(crate::agent::tests::agent(dir.path()));
        let task = start(&agent, Scripted::new(&[Attempt::Reached]), Times::default());
        settle().await;
        drop(agent);
        tokio::time::sleep(Duration::from_secs(61)).await;
        settle().await;
        assert!(task.is_finished());
    }
}
