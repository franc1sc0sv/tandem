//! El asistente de configuración inicial (DESIGN §5, sin monitores) con dos `tandemd`
//! reales por loopback, armados con la configuración del primer arranque: (1) esta máquina
//! y sus permisos, (2) emparejar y ver el ping del enlace, (3) «presiona una tecla» y (4)
//! el resumen. Solo los pasos de cada máquina son simulados.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tandem_core::{Machine, Step};
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_proto::{PeerStatus, Reply, UiCommand};
use tandemd::daemon::{self, Backends, Daemon};
use tandemd::ipc::{IpcClient, mem};
use tandemd::network::NetOptions;
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::paths::Paths;
use tandemd::{Agent, AgentConfig, StoreError, autoconfig};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const TIMEOUT: Duration = Duration::from_secs(20);

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .expect("la prueba superó el tiempo máximo")
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
    agent: Arc<Agent>,
    ui: IpcClient<tokio::io::DuplexStream>,
    config: PathBuf,
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
}

/// Un agente armado como en el binario, con la configuración que genera el primer arranque
/// y el enlace de loopback hacia el otro.
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
            "port = {port}\npeers = [{{ kind = \"usb4\", addr = \"{}\" }}]",
            loopback(peer.1)
        ),
    );
    std::fs::write(&paths.config, &text).unwrap();
    let cfg = AgentConfig::load(&paths.config).unwrap();
    assert_eq!(
        cfg.desk,
        tandem_core::Config::default(),
        "instalación limpia"
    );
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
            agent: agent.clone(),
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
        agent,
        ui,
        config: paths.config.clone(),
        stop: Some(stop),
        task,
        _dir: dir,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn el_asistente_reducido_de_punta_a_punta() {
    let (mac_id, pc_id) = (Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
    let (mac_port, pc_port) = (free_port(), free_port());
    let mut mac = start(
        Machine::Mac,
        &mac_id,
        mac_port,
        (pc_id.public_key(), pc_port),
    )
    .await;
    let mut pc = start(
        Machine::Pc,
        &pc_id,
        pc_port,
        (mac_id.public_key(), mac_port),
    )
    .await;

    // (1) Esta máquina y sus permisos: lo que puede y el proceso al que darle permisos.
    let Reply::Capabilities(caps) = mac.ui.request(UiCommand::GetCapabilities).await.unwrap()
    else {
        panic!("se esperaban capacidades");
    };
    assert_eq!(caps.here.machine, Machine::Mac);
    assert_eq!(caps.here.input.process.pid, std::process::id());
    assert_eq!(
        mac.ui.request(UiCommand::RequestPermissions).await.unwrap(),
        Reply::Done
    );
    let Reply::Input(input) = mac.ui.request(UiCommand::GetInput).await.unwrap() else {
        panic!("se esperaba el teclado y mouse");
    };
    assert!(
        !input.defined,
        "sin hub ni respuesta, el teclado está sin definir"
    );

    // (2) Emparejadas (la confianza ya está): se ven y el ping del enlace se muestra.
    let reply = within(async {
        loop {
            match mac.ui.request(UiCommand::PingPeer).await.unwrap() {
                Reply::Pong { link, millis } => break (link, millis),
                Reply::Rejected(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                other => panic!("{other:?}"),
            }
        }
    })
    .await;
    assert_eq!(reply.0, tandem_proto::Link::Usb4);
    assert!(reply.1 >= 1);
    assert!(matches!(
        mac.ui.request(UiCommand::GetPeer).await.unwrap(),
        Reply::Peer(PeerStatus::Paired { link: Some(_), .. })
    ));

    // (3) «Presiona una tecla»: el asistente de la Mac arma la prueba y la PC se entera.
    assert_eq!(
        mac.ui
            .request(UiCommand::KeyProbe { on: true })
            .await
            .unwrap(),
        Reply::Done
    );
    let mut probe = pc.agent.subscribe_key_probe();
    within(probe.wait_for(|s| s.armed)).await.unwrap();
    // La tecla llega a la PC.
    assert_eq!(
        pc.ui.request(UiCommand::HeardKey).await.unwrap(),
        Reply::Done
    );
    let mut probe = mac.agent.subscribe_key_probe();
    let heard = within(probe.wait_for(|s| s.heard.is_some()))
        .await
        .unwrap()
        .heard;
    assert_eq!(heard, Some(Machine::Pc));
    // Una segunda tecla en la otra no cambia lo que se oyó primero.
    mac.ui.request(UiCommand::HeardKey).await.unwrap();
    assert_eq!(mac.agent.key_probe_status().heard, Some(Machine::Pc));

    // «¿Es correcto?»: sí. Se guarda en las dos y deja de ser «sin definir».
    assert_eq!(
        mac.ui
            .request(UiCommand::SetKeyboard(Machine::Pc))
            .await
            .unwrap(),
        Reply::Done
    );
    let mut kb = pc.agent.subscribe_keyboard();
    within(kb.wait_for(|k| *k == Some(Machine::Pc)))
        .await
        .unwrap();
    for r in [&mac, &pc] {
        let mut st = r.agent.subscribe_input();
        within(st.wait_for(|i| i.defined && i.server == Machine::Pc))
            .await
            .unwrap();
        let saved = AgentConfig::load(&r.config).unwrap();
        assert_eq!(saved.input.keyboard, Some(Machine::Pc));
    }

    // (4) Resumen: de dónde salió la configuración y el puesto (vacío, sin monitores).
    let Reply::Setup(setup) = mac.ui.request(UiCommand::GetSetup).await.unwrap() else {
        panic!("se esperaba la configuración");
    };
    assert!(setup.config_file.is_some());
    assert_eq!(
        mac.ui.request(UiCommand::GetDesk).await.unwrap(),
        Reply::Desk(tandem_core::Config::default())
    );
    // Desactivar la escucha al terminar.
    mac.ui
        .request(UiCommand::KeyProbe { on: false })
        .await
        .unwrap();
    let mut probe = pc.agent.subscribe_key_probe();
    within(probe.wait_for(|s| !s.armed)).await.unwrap();

    mac.stop().await;
    pc.stop().await;
}
