//! Modules between two real `tandemd`s over loopback (T-098): what one machine's UI
//! turns on or off on the other arrives over the control plane, and both UIs see the new
//! state in [`AgentToUi::Machines`].

mod common;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tandem_core::{Machine, Step};
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_proto::{AgentToUi, Capabilities, ModuleId, ModuleState, Reply, UiCommand};
use tandemd::daemon::{self, Backends, Daemon, ShutdownReason};
use tandemd::ipc::{IpcClient, mem};
use tandemd::network::NetOptions;
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::paths::Paths;
use tandemd::test_ports::{PortTaken, port_taken};
use tandemd::{AgentConfig, StoreError, autoconfig};
use tokio::io::DuplexStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const TIMEOUT: Duration = Duration::from_secs(20);

/// Says where it waited, to know which step hung (T-230).
#[track_caller]
fn within<F: Future>(f: F) -> impl Future<Output = F::Output> {
    let at = std::panic::Location::caller();
    async move {
        match tokio::time::timeout(TIMEOUT, f).await {
            Ok(v) => v,
            Err(_) => panic!("the test exceeded the maximum time waiting at {at}"),
        }
    }
}

/// The first is side A's (DECISIONS A1).
fn sides(x: Identity, y: Identity) -> (Identity, Identity) {
    if x.public_key().as_bytes() < y.public_key().as_bytes() {
        (x, y)
    } else {
        (y, x)
    }
}

struct Ok_;

impl LocalSteps for Ok_ {
    fn execute<'a>(&'a self, _step: &'a Step) -> StepFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

struct Running {
    ui: IpcClient<DuplexStream>,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<ShutdownReason, StoreError>>,
    _dir: tempfile::TempDir,
}

impl Running {
    async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        within(self.task).await.unwrap().unwrap();
    }

    /// Waits for a machines notice that satisfies `pred`.
    async fn until_machines(
        &mut self,
        pred: impl Fn(&Capabilities, Option<&Capabilities>) -> bool,
    ) {
        within(async {
            loop {
                match self.ui.next_event().await.unwrap() {
                    Some(AgentToUi::Machines { local, remote }) => {
                        if pred(&local, remote.as_deref()) {
                            return;
                        }
                    }
                    Some(_) => {}
                    None => panic!("the agent closed the IPC"),
                }
            }
        })
        .await;
    }
}

/// An agent built as in the binary, already paired with the other, with the loopback
/// link to it. Without an input engine: only the modules' state matters.
async fn start(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: (PublicKey, u16),
) -> Result<Running, PortTaken> {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        config: dir.path().join("tandem.toml"),
        data_dir: dir.path().to_owned(),
    };
    identity.save(&paths.identity_file()).unwrap();
    let mut trust = TrustStore::open(paths.trust_file()).unwrap();
    trust
        .add_and_save(TrustedPeer {
            public_key: peer.0,
            name: machine.other().to_string(),
        })
        .unwrap();
    let text = autoconfig::generate(machine).replace(
        &format!("port = {}", tandemd::config::DEFAULT_PORT),
        &format!(
            "port = {port}\npeers = [{{ kind = \"ethernet\", addr = \"{}\" }}]",
            loopback(peer.1)
        ),
    );
    std::fs::write(&paths.config, &text).unwrap();
    let cfg = AgentConfig::load(&paths.config).unwrap();
    let started = daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: Arc::new(Ok_),
            input: None,
            env: None,
        },
        NetOptions {
            bind: Some(loopback(port)),
            mdns: false,
            // Without reservation: a taken port fails right away and others are chosen.
            fallback_ports: Vec::new(),
            health: common::health(),
            ..NetOptions::default()
        },
    );
    let Daemon {
        agent,
        network,
        input,
    } = port_taken(started).unwrap()?;
    let (listener, connector) = mem::pair();
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(
        Daemon {
            agent,
            network,
            input,
        }
        .serve(listener, async {
            let _ = stopped.await;
        }),
    );
    let ui = IpcClient::handshake(connector.connect().unwrap())
        .await
        .unwrap();
    Ok(Running {
        ui,
        stop: Some(stop),
        task,
        _dir: dir,
    })
}

fn module(caps: &Capabilities, id: ModuleId) -> ModuleState {
    caps.modules
        .iter()
        .find(|m| m.id == id)
        .map(|m| m.state.clone())
        .unwrap_or_else(|| panic!("{} does not announce {id:?}", caps.info.name))
}

/// Turning off a module of the other machine from this one's UI changes its state in both UIs;
/// and turning it on from the other UI, too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn switching_a_remote_module_shows_in_both_uis() {
    let (a_id, b_id) = sides(Identity::from_seed([7; 32]), Identity::from_seed([8; 32]));
    let (mut a, mut b) = tandemd::start_pair!(
        |a_port, b_port| start(Machine::A, &a_id, a_port, (b_id.public_key(), b_port)),
        start(Machine::B, &b_id, b_port, (a_id.public_key(), a_port))
    );

    // Until A has a link to B and knows its modules (its announcement arrives on connecting).
    within(async {
        while !matches!(
            a.ui.request(UiCommand::PingPeer).await.unwrap(),
            Reply::Pong { .. }
        ) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    let peer = within(async {
        loop {
            match a.ui.request(UiCommand::GetCapabilities).await.unwrap() {
                Reply::Capabilities(c) if c.peer.is_some() => break c.peer.unwrap(),
                _ => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    })
    .await;
    assert_eq!(peer.machine, Machine::B);
    assert_eq!(module(&peer, ModuleId::Clipboard), ModuleState::Active);

    let reply =
        a.ui.request(UiCommand::SetModule {
            machine: Machine::B,
            module: ModuleId::Clipboard,
            on: false,
        })
        .await
        .unwrap();
    assert_eq!(reply, Reply::Done);
    b.until_machines(|local, _| module(local, ModuleId::Clipboard) == ModuleState::Off)
        .await;
    a.until_machines(|_, remote| {
        remote.is_some_and(|r| module(r, ModuleId::Clipboard) == ModuleState::Off)
    })
    .await;

    // From B's UI, its own turns on again and A sees it.
    let reply =
        b.ui.request(UiCommand::SetModule {
            machine: Machine::B,
            module: ModuleId::Clipboard,
            on: true,
        })
        .await
        .unwrap();
    assert_eq!(reply, Reply::Done);
    a.until_machines(|_, remote| {
        remote.is_some_and(|r| module(r, ModuleId::Clipboard) == ModuleState::Active)
    })
    .await;

    // What the other cannot turn off is rejected here, with its reason.
    let Reply::Rejected(why) =
        a.ui.request(UiCommand::SetModule {
            machine: Machine::B,
            module: ModuleId::Tunnels,
            on: false,
        })
        .await
        .unwrap()
    else {
        panic!("tunnels cannot be turned off");
    };
    assert!(why.contains("cannot"), "{why}");

    a.stop().await;
    b.stop().await;
}
