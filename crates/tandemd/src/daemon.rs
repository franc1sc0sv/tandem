//! The assembled agent ([`start`]) and its main loop ([`run`]): accepts connections from the
//! UI until shutdown and does so in order.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tandem_core::Machine;
use tandem_link::identity::{Identity, IdentityError, PublicKey, TrustStore};
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::input::{Input, InputSetup};
use crate::ipc::{Listener, serve_connection};
use crate::network::{NetError, NetOptions, Network};
use crate::orchestrator::LocalSteps;
use crate::paths::Paths;
use crate::store::{StateStore, StoreError};

/// The steps this machine knows how to do.
pub enum Backends {
    /// The platform's ([`crate::PowerSteps::for_machine`] and
    /// [`InputSetup::platform`]).
    Platform,
    /// Others (in tests, simulated). Without `input` there is no shared keyboard, mouse or
    /// clipboard; without `env` no environments are found nor resources measured.
    Custom {
        local: Arc<dyn LocalSteps>,
        input: Option<Box<InputSetup>>,
        env: Option<Arc<tandem_env::Host>>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("this machine's identity: {0}")]
    Identity(#[source] IdentityError),
    #[error("trust in the peer: {0}")]
    Trust(#[source] IdentityError),
    #[error(transparent)]
    Net(#[from] NetError),
}

/// An agent with its network running; the UI remains to be served with [`Daemon::serve`].
pub struct Daemon {
    pub agent: Arc<Agent>,
    pub network: Network,
    /// The input engine; `None` without input backends.
    pub input: Option<Input>,
}

/// This machine's side according to the identity (DECISIONS A1): relative to the
/// trusted peer, the lower key is `A`. `None` without a peer (or with the same key, which is not a
/// peer): then what was saved holds.
pub fn resolve_side(own: &PublicKey, trust: &TrustStore) -> Option<Machine> {
    let peer = trust.peers().next()?;
    Machine::of_keys(own.as_bytes(), peer.public_key.as_bytes())
}

/// Builds the complete agent over the data directory of `paths`: the Ed25519 identity
/// (created the first time), trust in the peer, the side that comes from the two, the saved
/// state and the network. It must be called inside a tokio runtime.
pub fn start(
    cfg: AgentConfig,
    paths: &Paths,
    backends: Backends,
    net: NetOptions,
) -> Result<Daemon, StartError> {
    let identity =
        Identity::load_or_create(&paths.identity_file()).map_err(StartError::Identity)?;
    tracing::info!(
        key = %identity.public_key().to_hex(),
        "this machine's identity"
    );
    let trust = TrustStore::open(paths.trust_file()).map_err(StartError::Trust)?;
    let cfg = match resolve_side(&identity.public_key(), &trust) {
        Some(side) => {
            if side != cfg.machine {
                tracing::info!(side = %side, "by the keys, this machine is the other side");
            }
            cfg.with_side(side)
        }
        None => cfg,
    };
    let store = StateStore::new(paths.state_file());
    let platform = matches!(backends, Backends::Platform);
    let (agent, input) = match backends {
        Backends::Platform => (
            Agent::with_platform(cfg, store),
            Some(Box::new(InputSetup::platform())),
        ),
        Backends::Custom { local, input, env } => {
            let agent = Agent::with_local_steps(cfg, store, local);
            let agent = match env {
                Some(host) => agent.with_env_host(host),
                None => agent,
            };
            (agent, input)
        }
    };
    let agent = Arc::new(agent);
    agent.set_config_file(paths.config.clone());
    if platform {
        start_monitors(&agent);
        start_displays(&agent, paths);
    }
    // Only macOS asks for Local Network (T-344): the probe also brings up its alert, and the
    // agent lives on after it, which the alert needs.
    #[cfg(target_os = "macos")]
    if platform {
        drop(crate::local_network::start(
            &agent,
            Arc::new(crate::local_network::UdpProbe),
            crate::local_network::Times::default(),
        ));
    }
    // Environments are scanned in the background: they can take a while (WSL starts its distro).
    agent.rescan();
    let network = Network::start(agent.clone(), &identity, trust, net)?;
    let input =
        input.map(|setup| crate::input::start(agent.clone(), network.active_link(), *setup));
    // Destination of file drags (T-121): on Windows, `DoDragDrop`; on macOS,
    // `NSDraggingSession` on the process's main thread, which `main` leaves serving the
    // run loop de AppKit ([`tandem_files::drag_macos::run_app_loop`]).
    #[cfg(any(windows, target_os = "macos"))]
    if platform {
        let (agent, base) = (agent.clone(), paths.data_dir.join("drag"));
        drop(tokio::spawn(async move {
            #[cfg(windows)]
            let starter = tandem_files::drag_windows::WindowsDragStarter;
            #[cfg(target_os = "macos")]
            let starter = tandem_files::drag_macos::MacDragStarter::new(
                tandem_files::drag_macos::main_queue(),
            );
            crate::drag::serve(&agent, tandem_files::drag::DragIn::new(starter, base)).await;
        }));
    }
    // The tunnels follow the active link; the task ends with the network.
    drop(crate::tunnels::follow_link(
        agent.clone(),
        network.active_link(),
    ));
    Ok(Daemon {
        agent,
        network,
        input,
    })
}

/// The monitors with the system's DDC bus (`dxva2` on Windows, `IOAVService` on macOS). Without a
/// backend the module is left "unavailable" with its reason.
fn start_monitors(agent: &Arc<Agent>) {
    match monitor_ddc::system_bus() {
        Ok(bus) => {
            let mut core = tandem_monitor::LocalMonitors::new(
                bus,
                Arc::new(monitor_ddc::SystemClock::new()),
                tandem_monitor::local::system_wall(),
                tandem_monitor::LocalConfig::default(),
            );
            // Displays the DDC bus does not enumerate are still announced by their EDID (T-353).
            if let Some(edids) = tandem_input::displays::system_edids() {
                core = core.with_system_edids(Arc::new(edids));
            }
            // The task lives as long as the agent: it ends on its own when it is dropped.
            drop(agent.start_monitors(core, crate::monitors::MonitorTimes::default(), false));
            // A display plugged in, unplugged, turned on or off: rescan now (T-349).
            if let Some(changes) = tandem_input::hotplug::system()
                && let Err(e) = agent.monitor_hub().watch_displays(changes.as_ref())
            {
                tracing::warn!("display changes are not followed, only the periodic scan: {e}");
            }
        }
        Err(e) => tracing::info!("monitors: {e}"),
    }
}

/// Display control (T-341): this machine's display of a monitor that shows the other goes off.
/// What it turned off goes back on at start-up, at exit ([`Daemon::serve`]), on a panic and when
/// AppKit ends the process at the end of the session ([`crate::displays::restore_at_process_exit`]).
fn start_displays(agent: &Arc<Agent>, paths: &Paths) {
    let guard = Arc::new(crate::displays::DisplayGuard::new(
        tandem_input::displays::system_display_power(),
        Some(paths.displays_off_file()),
    ));
    match guard.control() {
        tandem_input::displays::DisplayControl::NotNeeded => {}
        tandem_input::displays::DisplayControl::Available => {
            tracing::info!("display control available");
        }
        tandem_input::displays::DisplayControl::Unavailable(why) => {
            tracing::info!("display control unavailable: {why}");
        }
    }
    guard.register_for_process_exit();
    let weak = Arc::downgrade(&guard);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(g) = weak.upgrade() {
            g.restore_on_panic();
        }
        previous(info);
    }));
    // The task lives as long as the agent.
    drop(agent.start_displays(guard));
}

impl Daemon {
    /// Serves the UI until `shutdown` ends; then closes the network. Returns why it stopped.
    pub async fn serve<L: Listener, R: Into<ShutdownReason>>(
        self,
        listener: L,
        shutdown: impl Future<Output = R>,
    ) -> Result<ShutdownReason, StoreError> {
        let agent = self.agent.clone();
        let result = run_until(self.agent, listener, shutdown).await;
        // A display turned off by this machine does not stay off after it exits (T-341).
        let _ = tokio::task::spawn_blocking(move || agent.restore_displays()).await;
        // First the capture is released: the keyboard and mouse return to this machine.
        drop(self.input);
        self.network.shutdown().await;
        result
    }
}

/// How long to wait for open connections to finish before cutting them.
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// [`run_until`] for whoever does not need the reason (the UI's and the CLI's tests).
pub async fn run<L: Listener>(
    agent: Arc<Agent>,
    listener: L,
    shutdown: impl Future<Output = ()>,
) -> Result<(), StoreError> {
    run_until(agent, listener, shutdown).await.map(drop)
}

/// Runs the agent until `shutdown` ends, and returns why it ended (T-334).
///
/// Orderly shutdown: it stops accepting (the listener is dropped and, on Unix, deletes its
/// socket), connections are notified, they are given [`DRAIN_TIMEOUT`] and the state is saved.
pub async fn run_until<L: Listener, R: Into<ShutdownReason>>(
    agent: Arc<Agent>,
    mut listener: L,
    shutdown: impl Future<Output = R>,
) -> Result<ShutdownReason, StoreError> {
    let (stop, stop_rx) = watch::channel(false);
    let mut conns = JoinSet::new();
    tokio::pin!(shutdown);
    let reason = loop {
        tokio::select! {
            reason = &mut shutdown => break reason.into(),
            accepted = listener.accept() => match accepted {
                Ok(stream) => {
                    let (agent, stop_rx) = (agent.clone(), stop_rx.clone());
                    conns.spawn(async move {
                        if let Err(e) = serve_connection(stream, agent, stop_rx).await {
                            tracing::debug!("IPC connection ended: {e}");
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!("could not accept an IPC connection: {e}");
                    // Avoids spinning idle if the error repeats.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
        }
    };

    tracing::info!("shutting down ({reason})");
    drop(listener);
    stop.send_replace(true);
    let drained = tokio::time::timeout(DRAIN_TIMEOUT, async {
        while conns.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        tracing::warn!("IPC connections that did not close in time: they are cut");
        conns.shutdown().await;
    }
    agent.persist()?;
    tracing::info!("state saved at {}", agent.store_path().display());
    Ok(reason)
}

/// Why the agent's orderly shutdown started (T-334), for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownReason {
    /// Ctrl+C in a terminal (SIGINT on Unix).
    CtrlC,
    /// launchd's stop (`bootout`, logout) or a `kill`.
    Sigterm,
    /// Windows: the console window was closed.
    ConsoleClosed,
    /// Windows: the system is shutting down.
    SystemShutdown,
    /// Whoever embeds the agent stopped it (the tests).
    Requested,
}

/// A shutdown future that only says "stop" is [`ShutdownReason::Requested`].
impl From<()> for ShutdownReason {
    fn from((): ()) -> Self {
        ShutdownReason::Requested
    }
}

impl std::fmt::Display for ShutdownReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ShutdownReason::CtrlC => "Ctrl+C",
            ShutdownReason::Sigterm => "SIGTERM",
            ShutdownReason::ConsoleClosed => "console closed",
            ShutdownReason::SystemShutdown => "system shutdown",
            ShutdownReason::Requested => "stop requested",
        })
    }
}

/// Ends with Ctrl+C, or with SIGTERM (launchd) / system shutdown (Windows), and says which.
///
/// The signals are registered when this is called, not on the first poll, so a signal that
/// arrives before the agent's loop starts is not lost. Call it inside the runtime.
pub fn shutdown_signal() -> impl Future<Output = ShutdownReason> {
    #[cfg(unix)]
    let term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
    #[cfg(windows)]
    let windows = (
        tokio::signal::windows::ctrl_close(),
        tokio::signal::windows::ctrl_shutdown(),
    );
    async move {
        #[cfg(unix)]
        match term {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => ShutdownReason::CtrlC,
                    _ = term.recv() => ShutdownReason::Sigterm,
                }
            }
            Err(e) => {
                tracing::warn!("could not listen for SIGTERM: {e}");
                let _ = tokio::signal::ctrl_c().await;
                ShutdownReason::CtrlC
            }
        }
        #[cfg(windows)]
        match windows {
            (Ok(mut close), Ok(mut shut)) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => ShutdownReason::CtrlC,
                    _ = close.recv() => ShutdownReason::ConsoleClosed,
                    _ = shut.recv() => ShutdownReason::SystemShutdown,
                }
            }
            _ => {
                tracing::warn!("could not listen for the Windows shutdown");
                let _ = tokio::signal::ctrl_c().await;
                ShutdownReason::CtrlC
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = tokio::signal::ctrl_c().await;
            ShutdownReason::CtrlC
        }
    }
}

#[cfg(test)]
mod tests {
    use tandem_core::State;
    use tokio::sync::oneshot;

    use super::*;
    use crate::agent::tests::agent;
    use crate::ipc::{IpcClient, mem};
    use crate::store::StateStore;

    #[tokio::test]
    async fn serves_clients_and_saves_on_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (listener, connector) = mem::pair();
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(run(agent.clone(), listener, async {
            let _ = stopped.await;
        }));

        let mut a = IpcClient::handshake(connector.connect().unwrap())
            .await
            .unwrap();
        let mut b = IpcClient::handshake(connector.connect().unwrap())
            .await
            .unwrap();
        assert_eq!(a.get_state().await.unwrap(), agent.state());
        assert_eq!(b.get_state().await.unwrap(), agent.state());

        // The state lives in memory: the file is deleted to see that shutdown writes it.
        let store = StateStore::new(dir.path().join("state.json"));
        let _ = std::fs::remove_file(store.path());
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();

        assert_eq!(store.load().unwrap(), Some(agent.state()));
        // The connections closed and new ones are not accepted.
        assert!(matches!(a.next_event().await, Ok(None)));
        assert!(matches!(b.next_event().await, Ok(None)));
        assert!(connector.connect().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn stuck_connection_does_not_block_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (listener, connector) = mem::pair();
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(run(agent.clone(), listener, async {
            let _ = stopped.await;
        }));
        // A client that connects and never greets.
        let _silent = connector.connect().unwrap();
        tokio::task::yield_now().await;
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
        let store = StateStore::new(dir.path().join("state.json"));
        assert_eq!(
            store.load().unwrap(),
            Some(State::initial(tandem_core::Machine::A))
        );
    }

    #[tokio::test]
    async fn run_returns_the_reason_its_shutdown_gave() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (listener, _connector) = mem::pair();
        let reason = run_until(agent, listener, async { ShutdownReason::Sigterm }).await;
        assert_eq!(reason.unwrap(), ShutdownReason::Sigterm);
    }

    #[tokio::test]
    async fn a_shutdown_that_only_says_stop_is_requested() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (listener, _connector) = mem::pair();
        let reason = run_until(agent, listener, async {}).await;
        assert_eq!(reason.unwrap(), ShutdownReason::Requested);
    }

    #[test]
    fn the_shutdown_reason_reads_well_in_the_log() {
        let texts: Vec<String> = [
            ShutdownReason::CtrlC,
            ShutdownReason::Sigterm,
            ShutdownReason::ConsoleClosed,
            ShutdownReason::SystemShutdown,
            ShutdownReason::Requested,
        ]
        .iter()
        .map(|r| format!("shutting down ({r})"))
        .collect();
        assert_eq!(
            texts,
            [
                "shutting down (Ctrl+C)",
                "shutting down (SIGTERM)",
                "shutting down (console closed)",
                "shutting down (system shutdown)",
                "shutting down (stop requested)",
            ]
        );
    }

    /// The signal is registered when `shutdown_signal` is called, so this SIGTERM is caught by
    /// it and does not kill the test process.
    #[cfg(unix)]
    #[tokio::test]
    async fn sigterm_is_reported_by_shutdown_signal() {
        let signal = shutdown_signal();
        let sent = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(std::process::id().to_string())
            .status()
            .unwrap();
        assert!(sent.success());
        let reason = tokio::time::timeout(Duration::from_secs(10), signal).await;
        assert_eq!(reason.unwrap(), ShutdownReason::Sigterm);
    }

    #[test]
    fn the_side_comes_from_the_keys_of_the_trusted_peer() {
        let dir = tempfile::tempdir().unwrap();
        let (low, high) = {
            let (x, y) = (Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
            if x.public_key().as_bytes() < y.public_key().as_bytes() {
                (x, y)
            } else {
                (y, x)
            }
        };
        let mut trust = TrustStore::open(dir.path().join("trust.toml")).unwrap();
        assert_eq!(
            resolve_side(&low.public_key(), &trust),
            None,
            "without a peer"
        );
        trust.add(tandem_link::identity::TrustedPeer {
            public_key: high.public_key(),
            name: "other".into(),
        });
        assert_eq!(resolve_side(&low.public_key(), &trust), Some(Machine::A));
        let mut theirs = TrustStore::open(dir.path().join("trust-2.toml")).unwrap();
        theirs.add(tandem_link::identity::TrustedPeer {
            public_key: low.public_key(),
            name: "other".into(),
        });
        assert_eq!(resolve_side(&high.public_key(), &theirs), Some(Machine::B));
    }

    /// An agent that starts with a peer whose key is lower than its own stays with
    /// side B even if the file said A, and rewrites it.
    #[tokio::test]
    async fn starting_with_a_lower_peer_makes_this_machine_b() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config: dir.path().join("tandem.toml"),
            data_dir: dir.path().to_owned(),
        };
        let (low, high) = {
            let (x, y) = (Identity::from_seed([3; 32]), Identity::from_seed([4; 32]));
            if x.public_key().as_bytes() < y.public_key().as_bytes() {
                (x, y)
            } else {
                (y, x)
            }
        };
        high.save(&paths.identity_file()).unwrap();
        let mut trust = TrustStore::open(paths.trust_file()).unwrap();
        trust
            .add_and_save(tandem_link::identity::TrustedPeer {
                public_key: low.public_key(),
                name: "other".into(),
            })
            .unwrap();
        let text = "machine = \"a\"\n[input]\nkeyboard = \"a\"\n";
        std::fs::write(&paths.config, text).unwrap();
        let cfg = AgentConfig::from_toml(text).unwrap();
        let net = NetOptions {
            bind: Some("127.0.0.1:0".parse().unwrap()),
            mdns: false,
            fallback_ports: Vec::new(),
            ..NetOptions::default()
        };
        let backends = Backends::Custom {
            local: Arc::new(crate::orchestrator::Unimplemented),
            input: None,
            env: None,
        };
        let d = start(cfg, &paths, backends, net).unwrap();
        assert_eq!(d.agent.machine(), Machine::B);
        assert_eq!(
            d.agent.keyboard_declared(),
            Some(Machine::B),
            "the keyboard was this one's"
        );
        let saved = AgentConfig::load(&paths.config).unwrap();
        assert_eq!(saved.machine, Machine::B);
        assert_eq!(saved.input.keyboard, Some(Machine::B));
        d.network.shutdown().await;
    }
}
