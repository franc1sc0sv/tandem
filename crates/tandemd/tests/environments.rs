//! Entornos y recursos entre dos `tandemd` reales por loopback (T-105): lo que escanea una
//! máquina llega a la UI de la otra en el anuncio de capacidades, la UI de una puede pedir
//! que la otra vuelva a escanear, y los recursos en vivo de la otra llegan mientras la UI
//! mira y dejan de muestrearse cuando deja de mirar.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tandem_core::{Machine, Step};
use tandem_env::fake::{FakeMeter, FakeRunner};
use tandem_env::scan::PROBE_SCRIPT;
use tandem_env::{Host, MeterInfo, Platform, Reading};
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_proto::{AgentToUi, Capabilities, EnvKind, Reply, UiCommand};
use tandemd::daemon::{self, Backends, Daemon};
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

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .expect("la prueba superó el tiempo máximo")
}

/// La primera es la del lado A (DECISIONS A1).
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

/// Una máquina simulada: su shell con git, su CPU y su RAM. `load` es el uso de CPU que da
/// cada lectura, para reconocer de quién es cada muestra.
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
        cpu: Some(format!("CPU de prueba de {cores} núcleos")),
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
    task: JoinHandle<Result<(), StoreError>>,
    _dir: tempfile::TempDir,
}

impl Running {
    async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        within(self.task).await.unwrap().unwrap();
    }

    /// Pide las capacidades hasta que las de la otra cumplan `pred`.
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

    /// Espera un aviso de las máquinas en que la otra cumpla `pred`.
    async fn until_remote(&mut self, pred: impl Fn(&Capabilities) -> bool) -> Capabilities {
        within(async {
            loop {
                match self.ui.next_event().await.unwrap() {
                    Some(AgentToUi::Machines {
                        remote: Some(remote),
                        ..
                    }) if pred(&remote) => return *remote,
                    Some(_) => {}
                    None => panic!("el agente cerró la IPC"),
                }
            }
        })
        .await
    }
}

/// Un agente armado como en el binario, ya emparejado con el otro, con el enlace de
/// loopback hacia él y `host` como su máquina.
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
            // Sin reserva: un puerto tomado falla enseguida y se eligen otros.
            fallback_ports: Vec::new(),
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

    // (a) La UI de B ve los entornos y el equipo que escaneó A al arrancar.
    let seen = b.until_peer(|p| has_env(p, "bash")).await;
    assert_eq!(seen.machine, Machine::A);
    let bash = &seen.environments[0];
    assert_eq!((bash.kind, bash.default), (EnvKind::Shell, true));
    assert_eq!(bash.detail, "git");
    assert_eq!(seen.hardware.cores, 4);
    assert_eq!(seen.hardware.memory, 16 * GIB);
    // Y A ve los de B.
    a.until_peer(|p| has_env(p, "zsh") && p.hardware.cores == 12)
        .await;

    // (b) A tiene un shell nuevo; la UI de B le pide volver a escanear y lo ve llegar.
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

    // (c) La UI de B mira los recursos: llegan los de las dos máquinas.
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

    // Con `on: false`, A deja de muestrear (a lo más con la muestra que ya estaba en curso).
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
        "A siguió muestreando sin nadie mirando"
    );

    a.stop().await;
    b.stop().await;
}
