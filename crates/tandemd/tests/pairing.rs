//! Emparejamiento desde la UI entre dos `tandemd` armados con [`daemon::start`], sin
//! confianza previa: QUIC real en loopback, la Mac busca a la PC por el enlace fijo de
//! la config y cada UI habla por la IPC en memoria.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use tandem_core::{Machine, Mode};
use tandem_display::switcher::ManualSwitcher;
use tandem_input::geometry::{Rect, Screen, ScreenId};
use tandem_input::session::SessionConfig;
use tandem_link::identity::{Identity, TrustStore};
use tandem_proto::{
    AgentToUi, InputStatus, Link, PairingStatus, PeerStatus, Reply, TxnEvent, UiCommand,
};
use tandemd::agent::NO_NETWORK;
use tandemd::daemon::{self, Backends, Daemon};
use tandemd::input::fake::FakeInputBackends;
use tandemd::input::{InputOptions, InputSetup};
use tandemd::ipc::{IpcClient, mem};
use tandemd::network::{NOT_PAIRED, NOTHING_TO_CONFIRM, NetOptions, PAIRING_IN_PROGRESS};
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::pairing::PairingTimeouts;
use tandemd::paths::Paths;
use tandemd::{Agent, AgentConfig, StoreError};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const DESK: &str = r#"
[[monitors]]
id = "m34"
label = '34"'
wiring = { pc = "displayport", mac = "hdmi" }
"#;

const TIMEOUT: Duration = Duration::from_secs(30);

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .expect("la prueba superó el tiempo máximo")
}

/// Pasos simulados que siempre salen bien.
struct Ok_;

impl LocalSteps for Ok_ {
    fn execute<'a>(&'a self, _step: &'a tandem_core::Step) -> StepFuture<'a> {
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
    paths: Paths,
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

    async fn ask(&mut self, cmd: UiCommand) -> Reply {
        within(self.ui.request(cmd)).await.unwrap()
    }

    /// Espera el aviso de emparejamiento que cumpla `done` (los demás se saltan).
    async fn until_pairing(&mut self, done: impl Fn(&PairingStatus) -> bool) -> PairingStatus {
        within(async {
            loop {
                if let AgentToUi::Pairing(s) = self.ui.next_event().await.unwrap().unwrap()
                    && done(&s)
                {
                    break s;
                }
            }
        })
        .await
    }

    fn trusted(&self, other: &Identity) -> bool {
        TrustStore::open(self.paths.trust_file())
            .unwrap()
            .is_trusted(&other.public_key())
    }
}

/// Un agente sin par: conoce la dirección del otro por el enlace fijo, nada más.
async fn start(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer_port: u16,
    pairing_port: u16,
    timeouts: PairingTimeouts,
) -> Running {
    start_with_input(
        machine,
        identity,
        (port, peer_port, pairing_port),
        timeouts,
        None,
    )
    .await
}

/// Como [`start`], con el motor de entrada en marcha si se le dan backends.
async fn start_with_input(
    machine: Machine,
    identity: &Identity,
    (port, peer_port, pairing_port): (u16, u16, u16),
    timeouts: PairingTimeouts,
    input: Option<InputSetup>,
) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        config: dir.path().join("tandem.toml"),
        data_dir: dir.path().to_owned(),
    };
    identity.save(&paths.identity_file()).unwrap();
    let cfg = AgentConfig::from_toml(&format!(
        "machine = \"{machine}\"\n[network]\nport = {port}\npairing_port = {pairing_port}\n\
         peers = [{{ kind = \"usb4\", addr = \"{}\" }}]\n{DESK}",
        loopback(peer_port)
    ))
    .unwrap();
    let Daemon {
        agent,
        network,
        input,
    } = daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: Arc::new(Ok_),
            manual: Arc::new(ManualSwitcher::new()),
            input: input.map(Box::new),
        },
        NetOptions {
            bind: Some(loopback(port)),
            mdns: false,
            pairing: timeouts,
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
        paths,
        stop: Some(stop),
        task,
        _dir: dir,
    }
}

struct Desk {
    mac: Running,
    pc: Running,
    mac_id: Identity,
    pc_id: Identity,
}

async fn desk(seed: u8, timeouts: PairingTimeouts) -> Desk {
    let (mac_id, pc_id) = (
        Identity::from_seed([seed; 32]),
        Identity::from_seed([seed + 1; 32]),
    );
    let (mac_port, pc_port, pairing_port) = (free_port(), free_port(), free_port());
    let mac = start(
        Machine::Mac,
        &mac_id,
        mac_port,
        pc_port,
        pairing_port,
        timeouts,
    )
    .await;
    let pc = start(
        Machine::Pc,
        &pc_id,
        pc_port,
        mac_port,
        pairing_port,
        timeouts,
    )
    .await;
    Desk {
        mac,
        pc,
        mac_id,
        pc_id,
    }
}

fn comparing(s: &PairingStatus) -> bool {
    matches!(s, PairingStatus::Comparing { .. })
}

fn code_of(s: &PairingStatus) -> String {
    match s {
        PairingStatus::Comparing { code, .. } => code.clone(),
        other => panic!("se esperaba un código: {other:?}"),
    }
}

async fn until_input(agent: &Agent, f: impl Fn(&InputStatus) -> bool) {
    let mut rx = agent.subscribe_input();
    within(rx.wait_for(|s| f(s))).await.unwrap();
}

async fn until_peer(agent: &Agent, want: PeerStatus) {
    let mut rx = agent.subscribe_peer();
    within(rx.wait_for(|s| *s == want)).await.unwrap();
}

/// Iniciar en las dos → el mismo código en las dos UIs → confirmar en las dos →
/// emparejados, guardados en disco y conectados por el enlace fijo sin reiniciar.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_compare_confirm_and_both_are_paired() {
    let Desk {
        mut mac,
        mut pc,
        mac_id,
        pc_id,
    } = desk(40, PairingTimeouts::default()).await;
    assert_eq!(mac.agent.peer_status(), PeerStatus::Unpaired);
    // Confirmar sin código no hace nada.
    assert_eq!(
        mac.ask(UiCommand::ConfirmPairing).await,
        Reply::Rejected(NOTHING_TO_CONFIRM.into())
    );

    // La PC primero: el orden da igual, la Mac la busca hasta encontrarla.
    assert_eq!(pc.ask(UiCommand::StartPairing).await, Reply::Done);
    assert_eq!(
        pc.ask(UiCommand::GetPairing).await,
        Reply::Pairing(PairingStatus::Searching)
    );
    assert_eq!(
        pc.ask(UiCommand::StartPairing).await,
        Reply::Rejected(PAIRING_IN_PROGRESS.into())
    );
    assert_eq!(mac.ask(UiCommand::StartPairing).await, Reply::Done);

    let on_mac = mac.until_pairing(comparing).await;
    let on_pc = pc.until_pairing(comparing).await;
    assert_eq!(code_of(&on_mac), code_of(&on_pc));
    assert_eq!(code_of(&on_mac).len(), 6);
    assert!(
        matches!(&on_mac, PairingStatus::Comparing { peer, confirmed: false, .. } if peer == "PC")
    );
    assert!(
        matches!(&on_pc, PairingStatus::Comparing { peer, confirmed: false, .. } if peer == "Mac")
    );

    // Confirmar en uno solo todavía no empareja a nadie.
    assert_eq!(mac.ask(UiCommand::ConfirmPairing).await, Reply::Done);
    let waiting = mac
        .until_pairing(|s| {
            matches!(
                s,
                PairingStatus::Comparing {
                    confirmed: true,
                    ..
                }
            )
        })
        .await;
    assert_eq!(code_of(&waiting), code_of(&on_mac));
    assert!(!mac.trusted(&pc_id) && !pc.trusted(&mac_id));

    assert_eq!(pc.ask(UiCommand::ConfirmPairing).await, Reply::Done);
    let done = |s: &PairingStatus| matches!(s, PairingStatus::Done { .. });
    assert_eq!(
        pc.until_pairing(done).await,
        PairingStatus::Done { peer: "Mac".into() }
    );
    assert_eq!(
        mac.until_pairing(done).await,
        PairingStatus::Done { peer: "PC".into() }
    );
    assert!(mac.trusted(&pc_id));
    assert!(pc.trusted(&mac_id));

    // Sin reiniciar, cada uno sigue al otro por el enlace fijo y el modo cambia.
    until_peer(
        &mac.agent,
        PeerStatus::Paired {
            name: "PC".into(),
            link: Some(Link::Usb4),
        },
    )
    .await;
    until_peer(
        &pc.agent,
        PeerStatus::Paired {
            name: "Mac".into(),
            link: Some(Link::Usb4),
        },
    )
    .await;
    let mut st = mac.agent.subscribe();
    within(st.wait_for(|s| s.on.pc)).await.unwrap();
    let txn = within(async {
        loop {
            match mac
                .ui
                .request(UiCommand::SetMode(Mode::SoloPc))
                .await
                .unwrap()
            {
                Reply::Accepted { txn } => break txn,
                Reply::Rejected(why) if why == tandemd::agent::BUSY => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                other => panic!("se esperaba Accepted: {other:?}"),
            }
        }
    })
    .await;
    within(async {
        loop {
            match mac.ui.next_event().await.unwrap().unwrap() {
                AgentToUi::Txn(TxnEvent::Committed { txn: t, .. }) if t == txn => break,
                AgentToUi::Txn(
                    e @ (TxnEvent::RolledBack { .. } | TxnEvent::Inconsistent { .. }),
                ) => {
                    panic!("el cambio falló: {e:?}")
                }
                _ => {}
            }
        }
    })
    .await;

    // Ya emparejado, no se vuelve a emparejar sin desemparejar.
    assert!(matches!(
        mac.ask(UiCommand::StartPairing).await,
        Reply::Rejected(why) if why.contains("desemparéjalo")
    ));
    pc.stop().await;
    mac.stop().await;
}

/// Rechazar en una máquina (aunque la otra ya confirmó) deja a las dos sin par.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejecting_on_one_side_leaves_both_unpaired() {
    let Desk {
        mut mac,
        mut pc,
        mac_id,
        pc_id,
    } = desk(50, PairingTimeouts::default()).await;
    assert_eq!(mac.ask(UiCommand::StartPairing).await, Reply::Done);
    assert_eq!(pc.ask(UiCommand::StartPairing).await, Reply::Done);
    mac.until_pairing(comparing).await;
    pc.until_pairing(comparing).await;

    assert_eq!(pc.ask(UiCommand::ConfirmPairing).await, Reply::Done);
    assert_eq!(mac.ask(UiCommand::CancelPairing).await, Reply::Done);

    // Quien rechazó vuelve a reposo; el otro se entera de por qué.
    assert_eq!(
        mac.until_pairing(|s| !comparing(s)).await,
        PairingStatus::Idle
    );
    let failed = pc
        .until_pairing(|s| matches!(s, PairingStatus::Failed { .. }))
        .await;
    assert!(
        matches!(&failed, PairingStatus::Failed { reason } if reason.contains("rechazó en la Mac")),
        "{failed:?}"
    );
    assert!(!mac.trusted(&pc_id));
    assert!(!pc.trusted(&mac_id));
    assert_eq!(mac.agent.peer_status(), PeerStatus::Unpaired);
    assert_eq!(pc.agent.peer_status(), PeerStatus::Unpaired);

    // Cerrar el aviso del fallo lo deja en reposo, y se puede volver a intentar.
    assert_eq!(pc.ask(UiCommand::CancelPairing).await, Reply::Done);
    assert_eq!(pc.agent.pairing_status(), PairingStatus::Idle);
    assert_eq!(pc.ask(UiCommand::StartPairing).await, Reply::Done);
    pc.stop().await;
    mac.stop().await;
}

/// Si la otra máquina nunca abre el emparejamiento, se avisa y no queda nada a medias.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn searching_alone_gives_up() {
    let timeouts = PairingTimeouts {
        search: Duration::from_millis(1500),
        ..PairingTimeouts::default()
    };
    let Desk {
        mut mac, mut pc, ..
    } = desk(60, timeouts).await;
    assert_eq!(mac.ask(UiCommand::StartPairing).await, Reply::Done);
    let failed = mac
        .until_pairing(|s| matches!(s, PairingStatus::Failed { .. }))
        .await;
    assert!(
        matches!(&failed, PairingStatus::Failed { reason } if reason.contains("no apareció la PC")),
        "{failed:?}"
    );
    // La PC nunca se enteró.
    assert_eq!(
        pc.ask(UiCommand::GetPairing).await,
        Reply::Pairing(PairingStatus::Idle)
    );
    pc.stop().await;
    mac.stop().await;
}

/// Cancelar mientras se busca vuelve a reposo enseguida.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_the_search() {
    let Desk { mut mac, pc, .. } = desk(70, PairingTimeouts::default()).await;
    assert_eq!(mac.ask(UiCommand::StartPairing).await, Reply::Done);
    assert_eq!(mac.ask(UiCommand::CancelPairing).await, Reply::Done);
    assert_eq!(
        mac.until_pairing(|s| *s == PairingStatus::Idle).await,
        PairingStatus::Idle
    );
    pc.stop().await;
    mac.stop().await;
}

/// Desemparejar desde la UI: se borra de la confianza, se corta el enlace y el otro
/// deja de verlo (su endpoint ya no lo acepta).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unpairing_from_the_ui() {
    let Desk {
        mut mac,
        mut pc,
        mac_id,
        pc_id,
    } = desk(80, PairingTimeouts::default()).await;
    assert_eq!(
        mac.ask(UiCommand::Unpair).await,
        Reply::Rejected(NOT_PAIRED.into())
    );
    assert_eq!(mac.ask(UiCommand::StartPairing).await, Reply::Done);
    assert_eq!(pc.ask(UiCommand::StartPairing).await, Reply::Done);
    mac.until_pairing(comparing).await;
    pc.until_pairing(comparing).await;
    assert_eq!(mac.ask(UiCommand::ConfirmPairing).await, Reply::Done);
    assert_eq!(pc.ask(UiCommand::ConfirmPairing).await, Reply::Done);
    let usb4 = |name: &str| PeerStatus::Paired {
        name: name.into(),
        link: Some(Link::Usb4),
    };
    until_peer(&mac.agent, usb4("PC")).await;
    until_peer(&pc.agent, usb4("Mac")).await;

    assert_eq!(pc.ask(UiCommand::Unpair).await, Reply::Done);
    assert_eq!(pc.agent.peer_status(), PeerStatus::Unpaired);
    assert!(!pc.trusted(&mac_id));
    assert!(!*pc.agent.orchestrator().peer_connected().borrow());
    // La Mac todavía confía en la PC, pero ya no la ve por ningún enlace.
    until_peer(
        &mac.agent,
        PeerStatus::Paired {
            name: "PC".into(),
            link: None,
        },
    )
    .await;
    assert!(mac.trusted(&pc_id));
    pc.stop().await;
    mac.stop().await;
}

/// Un agente sin la red armada (solo IPC) rechaza emparejar con un motivo claro.
#[tokio::test]
async fn without_network_pairing_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = AgentConfig::from_toml(&format!("machine = \"mac\"\n{DESK}")).unwrap();
    let agent = Arc::new(Agent::new(
        cfg,
        tandemd::StateStore::new(dir.path().join("state.json")),
    ));
    assert_eq!(
        agent.handle(UiCommand::StartPairing),
        Reply::Rejected(NO_NETWORK.into())
    );
}

fn input_setup(monitor: &str) -> InputSetup {
    let screen = Screen::new(
        ScreenId::Monitor(monitor.into()),
        Rect::new(0, 0, 1920, 1080),
        1.0,
    );
    InputSetup {
        backends: Arc::new(FakeInputBackends::new(vec![screen])),
        opts: InputOptions {
            session: SessionConfig {
                heartbeat_interval: Duration::from_millis(100),
                heartbeat_timeout: Duration::from_millis(1500),
                ..SessionConfig::default()
            },
            report_every: Duration::from_millis(200),
            screens_every: Duration::from_millis(200),
            permissions_every: Duration::from_millis(50),
            retry: Duration::from_millis(100),
            ..InputOptions::default()
        },
    }
}

/// Los agentes arrancan sin par y con el motor de entrada en marcha. Al emparejar
/// desde la UI, el motor ve el enlace nuevo y abre la sesión sin reiniciar; al
/// desemparejar, la cierra.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pairing_hot_opens_the_input_session() {
    let (mac_id, pc_id) = (Identity::from_seed([90; 32]), Identity::from_seed([91; 32]));
    let (mac_port, pc_port, pairing_port) = (free_port(), free_port(), free_port());
    let timeouts = PairingTimeouts::default();
    let mut mac = start_with_input(
        Machine::Mac,
        &mac_id,
        (mac_port, pc_port, pairing_port),
        timeouts,
        Some(input_setup("m34")),
    )
    .await;
    let mut pc = start_with_input(
        Machine::Pc,
        &pc_id,
        (pc_port, mac_port, pairing_port),
        timeouts,
        Some(input_setup("m34")),
    )
    .await;
    assert!(!mac.agent.input_status().session);

    assert_eq!(mac.ask(UiCommand::StartPairing).await, Reply::Done);
    assert_eq!(pc.ask(UiCommand::StartPairing).await, Reply::Done);
    mac.until_pairing(comparing).await;
    pc.until_pairing(comparing).await;
    assert_eq!(mac.ask(UiCommand::ConfirmPairing).await, Reply::Done);
    assert_eq!(pc.ask(UiCommand::ConfirmPairing).await, Reply::Done);

    until_input(&mac.agent, |s| s.session).await;
    until_input(&pc.agent, |s| s.session).await;

    assert_eq!(pc.ask(UiCommand::Unpair).await, Reply::Done);
    until_input(&pc.agent, |s| !s.session).await;
    until_input(&mac.agent, |s| !s.session).await;
    pc.stop().await;
    mac.stop().await;
}
