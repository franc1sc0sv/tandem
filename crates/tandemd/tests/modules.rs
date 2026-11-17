//! Módulos entre dos `tandemd` reales por loopback (T-098): lo que la UI de una máquina
//! enciende o apaga en la otra llega por el plano de control, y las dos UIs ven el estado
//! nuevo en [`AgentToUi::Machines`].

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use tandem_core::{Machine, Step};
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_proto::{AgentToUi, Capabilities, ModuleId, ModuleState, Reply, UiCommand};
use tandemd::daemon::{self, Backends, Daemon};
use tandemd::ipc::{IpcClient, mem};
use tandemd::network::NetOptions;
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::paths::Paths;
use tandemd::{AgentConfig, StoreError, autoconfig};
use tokio::io::DuplexStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const TIMEOUT: Duration = Duration::from_secs(20);

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

fn free_port() -> u16 {
    UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
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

    /// Espera un aviso de las máquinas que cumpla `pred`.
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
                    None => panic!("el agente cerró la IPC"),
                }
            }
        })
        .await;
    }
}

/// Un agente armado como en el binario, ya emparejado con el otro, con el enlace de
/// loopback hacia él. Sin motor de entrada: solo importa el estado de los módulos.
async fn start(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: (PublicKey, u16),
) -> Running {
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
    let Daemon {
        agent,
        network,
        input,
    } = daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: Arc::new(Ok_),
            input: None,
        },
        NetOptions {
            bind: Some(loopback(port)),
            mdns: false,
            ..NetOptions::default()
        },
    )
    .unwrap();
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
    Running {
        ui,
        stop: Some(stop),
        task,
        _dir: dir,
    }
}

fn module(caps: &Capabilities, id: ModuleId) -> ModuleState {
    caps.modules
        .iter()
        .find(|m| m.id == id)
        .map(|m| m.state.clone())
        .unwrap_or_else(|| panic!("{} no anuncia {id:?}", caps.info.name))
}

/// Apagar un módulo de la otra máquina desde la UI de esta cambia su estado en las dos UIs;
/// y encenderlo desde la otra UI, también.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn switching_a_remote_module_shows_in_both_uis() {
    let (a_id, b_id) = sides(Identity::from_seed([7; 32]), Identity::from_seed([8; 32]));
    let (a_port, b_port) = (free_port(), free_port());
    let mut a = start(Machine::A, &a_id, a_port, (b_id.public_key(), b_port)).await;
    let mut b = start(Machine::B, &b_id, b_port, (a_id.public_key(), a_port)).await;

    // Hasta que A tiene enlace con B y conoce sus módulos (su anuncio llega al conectarse).
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

    // Desde la UI de B, lo suyo vuelve a encenderse y A lo ve.
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

    // Lo que la otra no puede apagar se rechaza aquí, con su razón.
    let Reply::Rejected(why) =
        a.ui.request(UiCommand::SetModule {
            machine: Machine::B,
            module: ModuleId::Tunnels,
            on: false,
        })
        .await
        .unwrap()
    else {
        panic!("los túneles no se apagan");
    };
    assert!(why.contains("no se puede"), "{why}");

    a.stop().await;
    b.stop().await;
}
