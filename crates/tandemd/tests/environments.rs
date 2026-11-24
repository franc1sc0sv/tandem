//! Environments and resources between two real `tandemd`s over loopback (T-105): what one
//! machine scans reaches the other's UI in the capabilities announcement, one's UI can ask
//! the other to scan again, and the other's live resources arrive while the UI
//! watches and stop being sampled when it stops watching.

mod common;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tandem_core::{Machine, Step};
use tandem_env::fake::{FakeMeter, FakeRunner};
use tandem_env::scan::PROBE_SCRIPT;
use tandem_env::{Host, MeterInfo, Platform, Reading};
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_proto::{AgentToUi, Capabilities, EnvKind, Reply, UiCommand};
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
const GIB: u64 = 1 << 30;

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

/// A simulated machine: its shell with git, its CPU and its RAM. `load` is the CPU usage each
/// reading gives, to recognize whose each sample is.
fn machine(shell: &str, cores: u32, load: f32) -> (FakeRunner, FakeMeter, Arc<Host>) {
    let runner = FakeRunner::new();
    runner.set_var("SHELL", &format!("/bin/{shell}"));
    runner.on(
        shell,
        &["-lic", PROBE_SCRIPT],
        "@@os\n@@path\n/usr/bin:/bin\n@@tool git\ngit version 2.47.3\n",
    );
    runner.on(shell, &["--version"], format!("{shell} 5.9\n"));
    let meter = FakeMeter::new(MeterInfo {
        cpu: Some(format!("Test CPU with {cores} cores")),
        cores,
        memory: 16 * GIB,
    });
    meter.push(Reading {
        cpu: load,
        cores: vec![load; cores as usize],
        memory_used: 6 * GIB,
        memory_total: 16 * GIB,
        ..Reading::default()
    });
    let host = Arc::new(Host::new(
        Platform::Linux,
        Arc::new(runner.clone()),
        Box::new(meter.clone()),
    ));
    (runner, meter, host)
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

    /// Asks for capabilities until the other's satisfy `pred`.
    async fn until_peer(&mut self, pred: impl Fn(&Capabilities) -> bool) -> Capabilities {
        within(async {
            loop {
                if let Reply::Capabilities(c) =
                    self.ui.request(UiCommand::GetCapabilities).await.unwrap()
                    && let Some(peer) = c.peer
                    && pred(&peer)
                {
                    return peer;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
    }

    /// Waits for a machines notice in which the other satisfies `pred`.
    async fn until_remote(&mut self, pred: impl Fn(&Capabilities) -> bool) -> Capabilities {
        within(async {
            loop {
                match self.ui.next_event().await.unwrap() {
                    Some(AgentToUi::Machines {
                        remote: Some(remote),
                        ..
                    }) if pred(&remote) => return *remote,
                    Some(_) => {}
                    None => panic!("the agent closed the IPC"),
                }
            }
        })
        .await
    }
}

/// An agent built as in the binary, already paired with the other, with the loopback
/// link to it and `host` as its machine.
async fn start(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: (PublicKey, u16),
    host: Arc<Host>,
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
            env: Some(host),
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

fn has_env(caps: &Capabilities, id: &str) -> bool {
    caps.environments.iter().any(|e| e.id == id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_other_machine_environments_and_resources_reach_this_ui() {
    let (a_id, b_id) = sides(Identity::from_seed([41; 32]), Identity::from_seed([42; 32]));
    let (a_runner, a_meter, a_host) = machine("bash", 4, 42.0);
    let (_b_runner, _b_meter, b_host) = machine("zsh", 12, 7.0);
    let (mut a, mut b) = tandemd::start_pair!(
        |a_port, b_port| start(
            Machine::A,
            &a_id,
            a_port,
            (b_id.public_key(), b_port),
            a_host.clone(),
        ),
        start(
            Machine::B,
            &b_id,
            b_port,
            (a_id.public_key(), a_port),
            b_host.clone(),
        )
    );

    // (a) B's UI sees the environments and hardware A scanned on start.
    let seen = b.until_peer(|p| has_env(p, "bash")).await;
    assert_eq!(seen.machine, Machine::A);
    let bash = &seen.environments[0];
    assert_eq!((bash.kind, bash.default), (EnvKind::Shell, true));
    assert_eq!(bash.detail, "git");
    assert_eq!(seen.hardware.cores, 4);
    assert_eq!(seen.hardware.memory, 16 * GIB);
    // And A sees B's.
    a.until_peer(|p| has_env(p, "zsh") && p.hardware.cores == 12)
        .await;

    // (b) A has a new shell; B's UI asks it to scan again and sees it arrive.
    a_runner.on("fish", &["--version"], "fish, version 3.7.1\n");
    assert!(!has_env(&b.until_peer(|_| true).await, "fish"));
    assert_eq!(
        b.ui.request(UiCommand::Rescan {
            machine: Machine::A
        })
        .await
        .unwrap(),
        Reply::Done
    );
    let rescanned = b.until_remote(|r| has_env(r, "fish")).await;
    assert!(has_env(&rescanned, "bash"));

    // (c) B's UI watches the resources: those of both machines arrive.
    assert_eq!(
        b.ui.request(UiCommand::WatchResources { on: true })
            .await
            .unwrap(),
        Reply::Done
    );
    let (mut from_a, mut from_b) = (None, None);
    within(async {
        while from_a.is_none() || from_b.is_none() {
            if let Some(AgentToUi::Resources { machine, resources }) =
                b.ui.next_event().await.unwrap()
            {
                match machine {
                    Machine::A => from_a = Some(resources),
                    Machine::B => from_b = Some(resources),
                }
            }
        }
    })
    .await;
    let from_a = from_a.unwrap();
    assert_eq!(from_a.cpu.load, 420);
    assert_eq!(from_a.cpu.cores, [420; 4]);
    assert_eq!(from_a.memory.used, 6 * GIB);
    assert_eq!(from_b.unwrap().cpu.load, 70);
    assert!(a_meter.reads() > 0);

    // With `on: false`, A stops sampling (at most with the sample that was already in progress).
    assert_eq!(
        b.ui.request(UiCommand::WatchResources { on: false })
            .await
            .unwrap(),
        Reply::Done
    );
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let reads = a_meter.reads();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        a_meter.reads(),
        reads,
        "A kept sampling with nobody watching"
    );

    a.stop().await;
    b.stop().await;
}
