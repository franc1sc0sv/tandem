//! Environments and resources in the agent (T-105): the scan on start and on request, and the
//! live resources of both machines for whoever watches.
//!
//! Resources are sampled only with someone watching ([`tandem_env::ResourceMonitor`]): a
//! UI connection on this machine ([`Agent::watch_resources`]) or the other machine, which
//! asks for them for its UI. That request is a lease: whoever watches renews it every [`RENEW`] and if it stops
//! arriving (the link dropped) the other stops sending after [`LEASE`], without depending on
//! knowing when it was cut.

use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use tandem_core::Machine;
use tandem_env::{Host, ResourceMonitor, Scan, Watch};
use tandem_proto::{Capabilities, ControlMsg, Reply, Resources};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::Agent;

/// How often whoever watches renews the request to the other machine.
pub const RENEW: Duration = Duration::from_secs(3);
/// How long a resources request from the other machine is valid without renewal.
pub const LEASE: Duration = Duration::from_secs(10);

pub(super) struct EnvState {
    host: Arc<Host>,
    scan: Mutex<Scan>,
    scanning: Mutex<Scanning>,
    monitor: ResourceMonitor,
    /// The samples the other machine sends.
    peer_resources: broadcast::Sender<Resources>,
    /// The UI connections here that watch, and the task that renews the request to the other.
    local: Mutex<Local>,
    /// Until when the other machine wants this one's resources.
    serving: Mutex<Serving>,
}

#[derive(Default)]
struct Scanning {
    running: bool,
    /// Another scan was requested while one was running: when it finishes it is done again.
    again: bool,
}

#[derive(Default)]
struct Local {
    watchers: usize,
    renew: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Serving {
    until: Option<Instant>,
    running: bool,
}

impl std::fmt::Debug for EnvState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvState")
            .field("host", &self.host)
            .field("monitor", &self.monitor)
            .finish_non_exhaustive()
    }
}

impl EnvState {
    pub(super) fn new(host: Arc<Host>) -> Self {
        let sampler = host.clone();
        EnvState {
            host,
            scan: Mutex::new(Scan::default()),
            scanning: Mutex::new(Scanning::default()),
            monitor: ResourceMonitor::new(move || sampler.sample()),
            peer_resources: broadcast::channel(8).0,
            local: Mutex::new(Local::default()),
            serving: Mutex::new(Serving::default()),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Agent {
    /// Scan with `host` instead of the one built when the agent was created (tests and the
    /// real-backend agent).
    pub fn with_env_host(mut self, host: Arc<Host>) -> Self {
        self.env = EnvState::new(host);
        self
    }

    /// What the last scan found.
    pub fn environment_scan(&self) -> Scan {
        lock(&self.env.scan).clone()
    }

    /// What was found goes into what this machine announces.
    pub(super) fn add_environments(&self, caps: &mut Capabilities) {
        let scan = lock(&self.env.scan);
        caps.environments = scan.environments.clone();
        caps.hardware = scan.hardware.clone();
    }

    /// Scans in the background; what is found goes out in the capabilities (the UI and the other
    /// machine see it with the next announcement). One at a time: asking while one runs repeats
    /// the scan when it finishes. Outside a runtime it does nothing.
    pub fn rescan(self: &Arc<Self>) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("no runtime: environments are not scanned");
            return;
        };
        {
            let mut s = lock(&self.env.scanning);
            if s.running {
                s.again = true;
                return;
            }
            s.running = true;
        }
        let agent = self.clone();
        rt.spawn(async move {
            loop {
                let host = agent.env.host.clone();
                let started = std::time::Instant::now();
                match tokio::task::spawn_blocking(move || host.scan()).await {
                    Ok(scan) => {
                        tracing::info!(
                            entornos = scan.environments.len(),
                            gpus = scan.hardware.gpus.len(),
                            ms = started.elapsed().as_millis() as u64,
                            "environments scanned"
                        );
                        *lock(&agent.env.scan) = scan;
                        agent.refresh_capabilities();
                    }
                    Err(e) => tracing::warn!("the environment scan failed: {e}"),
                }
                let mut s = lock(&agent.env.scanning);
                if s.again {
                    s.again = false;
                } else {
                    s.running = false;
                    return;
                }
            }
        });
    }

    /// [`tandem_proto::UiCommand::Rescan`]: here it is scanned; the other is asked.
    pub(super) fn rescan_machine(self: &Arc<Self>, machine: Machine) -> Reply {
        if machine == self.machine() {
            self.rescan();
            return Reply::Done;
        }
        if !*self.orch.peer_connected().borrow() {
            return Reply::Rejected(format!(
                "the {} is not connected",
                self.machine_name(machine)
            ));
        }
        self.orch.peer().send(ControlMsg::Rescan);
        Reply::Done
    }

    /// The other machine asked to scan this one.
    pub fn receive_rescan(self: &Arc<Self>) {
        self.rescan();
    }

    /// Start receiving the resources of both machines. This one's are sampled while one
    /// exists; the other's are asked for (and the request renewed) while there is any.
    pub fn watch_resources(self: &Arc<Self>) -> ResourceWatch {
        let watch = self.env.monitor.watch();
        let local = self.env.monitor.subscribe();
        let peer = self.env.peer_resources.subscribe();
        let mut l = lock(&self.env.local);
        l.watchers += 1;
        if l.watchers == 1
            && let Ok(rt) = tokio::runtime::Handle::try_current()
        {
            let weak = Arc::downgrade(self);
            l.renew = Some(rt.spawn(async move {
                loop {
                    match weak.upgrade() {
                        Some(a) => a.orch.peer().send(ControlMsg::WatchResources { on: true }),
                        None => return,
                    }
                    tokio::time::sleep(RENEW).await;
                }
            }));
        }
        drop(l);
        ResourceWatch {
            agent: Arc::downgrade(self),
            _watch: watch,
            local,
            peer,
        }
    }

    fn unwatch_resources(&self) {
        let mut l = lock(&self.env.local);
        l.watchers = l.watchers.saturating_sub(1);
        if l.watchers == 0 {
            if let Some(task) = l.renew.take() {
                task.abort();
            }
            drop(l);
            self.orch
                .peer()
                .send(ControlMsg::WatchResources { on: false });
        }
    }

    /// The other machine wants (or no longer wants) this one's resources.
    pub fn receive_watch_resources(self: &Arc<Self>, on: bool) {
        let mut s = lock(&self.env.serving);
        if !on {
            s.until = None;
            return;
        }
        s.until = Some(Instant::now() + LEASE);
        if s.running {
            return;
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        s.running = true;
        drop(s);
        let weak = Arc::downgrade(self);
        let watch = self.env.monitor.watch();
        let mut samples = self.env.monitor.subscribe();
        rt.spawn(async move {
            let _watch = watch;
            loop {
                let sample = samples.recv().await;
                let Some(agent) = weak.upgrade() else { return };
                {
                    let mut s = lock(&agent.env.serving);
                    if s.until.is_none_or(|until| Instant::now() >= until) {
                        s.until = None;
                        s.running = false;
                        return;
                    }
                }
                match sample {
                    Ok(r) => agent.orch.peer().send(ControlMsg::Resources(Box::new(r))),
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => {
                        lock(&agent.env.serving).running = false;
                        return;
                    }
                }
            }
        });
    }

    /// A sample from the other machine, for whoever watches here.
    pub fn receive_peer_resources(&self, resources: Resources) {
        // With nobody watching it is not an error: the request may have been canceled on the way.
        let _ = self.env.peer_resources.send(resources);
    }
}

/// The resources of both machines for a UI connection. Dropping it stops watching.
pub struct ResourceWatch {
    agent: Weak<Agent>,
    _watch: Watch,
    local: broadcast::Receiver<Resources>,
    peer: broadcast::Receiver<Resources>,
}

impl std::fmt::Debug for ResourceWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResourceWatch")
    }
}

impl ResourceWatch {
    /// The next sample from either of the two, with its machine; `None` if the agent no longer
    /// exists. It can be canceled without losing samples.
    pub async fn next(&mut self) -> Option<(Machine, Resources)> {
        loop {
            let (here, got) = tokio::select! {
                r = self.local.recv() => (true, r),
                r = self.peer.recv() => (false, r),
            };
            let agent = self.agent.upgrade()?;
            let me = agent.machine();
            match got {
                Ok(r) => return Some((if here { me } else { me.other() }, r)),
                // An old sample is no use to anyone: continue with the next.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return None,
            }
        }
    }
}

impl Drop for ResourceWatch {
    fn drop(&mut self) {
        if let Some(agent) = self.agent.upgrade() {
            agent.unwatch_resources();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tandem_core::{Os, State, Step};
    use tandem_env::fake::{FakeMeter, FakeRunner};
    use tandem_env::{MeterInfo, Platform, Reading};
    use tandem_proto::{EnvKind, UiCommand};

    use super::*;
    use crate::StateStore;
    use crate::config::fixtures;
    use crate::orchestrator::{PeerLink, StepFuture};

    #[derive(Default)]
    struct Recorder(Mutex<Vec<ControlMsg>>);

    impl PeerLink for Recorder {
        fn execute<'a>(&'a self, _step: &'a Step) -> StepFuture<'a> {
            Box::pin(async { Ok(()) })
        }
        fn publish_state(&self, _state: &State) {}
        fn send(&self, msg: ControlMsg) {
            self.0.lock().unwrap().push(msg);
        }
    }

    impl Recorder {
        fn taken(&self) -> Vec<ControlMsg> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
        fn watches(&self) -> Vec<bool> {
            self.taken()
                .into_iter()
                .filter_map(|m| match m {
                    ControlMsg::WatchResources { on } => Some(on),
                    _ => None,
                })
                .collect()
        }
        fn samples(&self) -> usize {
            self.taken()
                .into_iter()
                .filter(|m| matches!(m, ControlMsg::Resources(_)))
                .count()
        }
    }

    fn host() -> Arc<Host> {
        let r = FakeRunner::new();
        r.set_var("SHELL", "/bin/zsh");
        r.on("zsh", &["--version"], "zsh 5.9 (arm64-apple-darwin24.0)\n");
        let meter = FakeMeter::new(MeterInfo {
            cpu: Some("Apple M5".into()),
            cores: 12,
            memory: 24 << 30,
        });
        meter.push(Reading {
            cpu: 18.0,
            cores: vec![18.0; 12],
            memory_used: 12 << 30,
            memory_total: 24 << 30,
            ..Reading::default()
        });
        Arc::new(Host::new(Platform::MacOs, Arc::new(r), Box::new(meter)))
    }

    fn agent(dir: &std::path::Path) -> (Arc<Agent>, Arc<Recorder>) {
        let mut cfg = fixtures::desk();
        cfg.os = Os::MacOs;
        let a = Arc::new(
            Agent::new(cfg, StateStore::new(dir.join("state.json"))).with_env_host(host()),
        );
        let peer = Arc::new(Recorder::default());
        a.set_peer(peer.clone());
        peer.taken();
        (a, peer)
    }

    #[tokio::test(start_paused = true)]
    async fn a_rescan_is_announced_in_the_capabilities() {
        let dir = tempfile::tempdir().unwrap();
        let (a, _peer) = agent(dir.path());
        assert!(a.capabilities().here.environments.is_empty());
        let mut caps = a.subscribe_capabilities();
        caps.mark_unchanged();
        assert_eq!(
            a.handle(UiCommand::Rescan {
                machine: a.machine()
            }),
            Reply::Done
        );
        caps.changed().await.unwrap();
        let here = caps.borrow_and_update().here.clone();
        let [zsh] = here.environments.as_slice() else {
            panic!("{:#?}", here.environments);
        };
        assert_eq!((zsh.kind, zsh.id.as_str()), (EnvKind::Shell, "zsh"));
        assert!(zsh.default);
        assert_eq!(here.hardware.cpu.as_deref(), Some("Apple M5"));
        assert_eq!(here.hardware.cores, 12);
        assert_eq!(a.environment_scan().environments.len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn rescanning_the_other_machine_asks_it() {
        let dir = tempfile::tempdir().unwrap();
        let (a, peer) = agent(dir.path());
        let other = a.machine().other();
        assert_eq!(a.handle(UiCommand::Rescan { machine: other }), Reply::Done);
        assert_eq!(peer.taken(), [ControlMsg::Rescan]);
        a.orchestrator().clear_peer();
        let Reply::Rejected(why) = a.handle(UiCommand::Rescan { machine: other }) else {
            panic!("without the other connected it cannot");
        };
        assert!(why.contains("is not connected"), "{why}");
    }

    #[tokio::test(start_paused = true)]
    async fn watching_samples_here_and_asks_the_other_with_renewals() {
        let dir = tempfile::tempdir().unwrap();
        let (a, peer) = agent(dir.path());
        let mut w = a.watch_resources();
        let (machine, r) = w.next().await.unwrap();
        assert_eq!(machine, a.machine());
        assert_eq!(r.cpu.load, 180);
        assert_eq!(r.memory.used, 12 << 30);
        // Request on start and renewed every RENEW.
        tokio::time::sleep(RENEW * 3).await;
        let renewals = peer.watches();
        assert!(
            renewals.len() >= 3 && renewals.iter().all(|on| *on),
            "{renewals:?}"
        );
        // A second connection does not ask again; dropping the last notifies once.
        let w2 = a.watch_resources();
        drop(w);
        assert!(peer.watches().iter().all(|on| *on));
        drop(w2);
        assert_eq!(peer.watches(), [false]);
        tokio::time::sleep(RENEW * 3).await;
        assert!(peer.watches().is_empty(), "with nobody watching it is not renewed");
    }

    #[tokio::test(start_paused = true)]
    async fn the_other_machine_samples_arrive_tagged_with_its_side() {
        let dir = tempfile::tempdir().unwrap();
        let (a, _peer) = agent(dir.path());
        let mut w = a.watch_resources();
        // The first local sample goes out right away; then the other's arrives.
        assert_eq!(w.next().await.unwrap().0, a.machine());
        let mut theirs = Resources::default();
        theirs.cpu.load = 970;
        a.receive_peer_resources(theirs.clone());
        assert_eq!(w.next().await.unwrap(), (a.machine().other(), theirs));
    }

    #[tokio::test(start_paused = true)]
    async fn serving_the_other_machine_stops_when_the_lease_ends() {
        let dir = tempfile::tempdir().unwrap();
        let (a, peer) = agent(dir.path());
        a.receive_watch_resources(true);
        tokio::time::sleep(Duration::from_millis(4500)).await;
        let sent = peer.samples();
        assert!((4..=6).contains(&sent), "{sent} samples in 4.5 s");
        // Renewing extends the lease.
        a.receive_watch_resources(true);
        tokio::time::sleep(LEASE - Duration::from_secs(1)).await;
        assert!(peer.samples() >= 8);
        // Without renewal, it cuts itself off.
        tokio::time::sleep(Duration::from_secs(3)).await;
        peer.taken();
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(peer.samples(), 0, "it kept sending without a lease");
        assert_eq!(a.env.monitor.watchers(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn the_other_machine_can_stop_watching_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let (a, peer) = agent(dir.path());
        a.receive_watch_resources(true);
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(peer.samples() >= 1);
        a.receive_watch_resources(false);
        tokio::time::sleep(Duration::from_secs(1)).await;
        peer.taken();
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(peer.samples(), 0);
        // And asking again starts it again.
        a.receive_watch_resources(true);
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert!(peer.samples() >= 1);
    }

    #[test]
    fn watch_resources_through_handle_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (a, _peer) = agent(dir.path());
            assert!(matches!(
                a.handle(UiCommand::WatchResources { on: true }),
                Reply::Rejected(_)
            ));
        });
    }
}
