//! Dos `tandemd` armados con [`daemon::start`], como en el binario: identidades
//! emparejadas en el directorio de datos, QUIC real en loopback y los enlaces fijos de
//! la config. Solo los pasos de cada máquina son simulados.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_core::{Machine, Mode, Step};
use tandem_display::switcher::ManualSwitcher;
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_proto::{AgentToUi, Link, PeerStatus, Reply, TxnEvent, UiCommand};
use tandemd::daemon::{self, Backends, Daemon};
use tandemd::ipc::{IpcClient, mem};
use tandemd::network::NetOptions;
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::paths::Paths;
use tandemd::{Agent, AgentConfig, StoreError};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const DESK: &str = r#"
[[monitors]]
id = "m34"
label = '34"'
wiring = { pc = "displayport", mac = "hdmi" }

[[monitors]]
id = "m32"
label = '32"'
wiring = { pc = "displayport", mac = "hdmi" }
"#;

/// Tope para que una prueba rota falle en vez de colgarse.
const TIMEOUT: Duration = Duration::from_secs(20);

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .expect("la prueba superó el tiempo máximo")
}

/// Pasos simulados: registran lo que se les pide.
#[derive(Default)]
struct Fake {
    log: Mutex<Vec<Step>>,
}

impl Fake {
    fn steps(&self) -> Vec<Step> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl LocalSteps for Fake {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
        Box::pin(async move {
            self.log
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(step.clone());
            Ok(())
        })
    }
}

/// Un puerto UDP libre de loopback. Se suelta enseguida para que lo tome el agente:
/// cada uno necesita conocer el del otro antes de arrancar.
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
    steps: Arc<Fake>,
    ui: IpcClient<tokio::io::DuplexStream>,
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

/// Un agente de `machine` armado como en el binario. `peer` es la llave y el puerto del
/// otro agente; sin él, el agente no tiene par emparejado.
async fn start(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: Option<(PublicKey, u16)>,
) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        config: dir.path().join("tandem.toml"),
        data_dir: dir.path().to_owned(),
    };
    identity.save(&paths.identity_file()).unwrap();
    let mut network = format!("[network]\nport = {port}\n");
    if let Some((key, peer_port)) = peer {
        let mut trust = TrustStore::open(paths.trust_file()).unwrap();
        trust
            .add_and_save(TrustedPeer {
                public_key: key,
                name: machine.other().to_string(),
            })
            .unwrap();
        network.push_str(&format!(
            "peers = [{{ kind = \"usb4\", addr = \"{}\" }}]\n",
            loopback(peer_port)
        ));
    }
    let cfg = AgentConfig::from_toml(&format!("machine = \"{machine}\"\n{network}{DESK}")).unwrap();
    let steps = Arc::new(Fake::default());
    let Daemon {
        agent,
        network,
        input,
    } = daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: steps.clone(),
            manual: Arc::new(ManualSwitcher::new()),
            input: None,
        },
        NetOptions {
            bind: Some(loopback(port)),
            mdns: false,
            ..NetOptions::default()
        },
    )
    .unwrap();
    assert_eq!(network.local_addr(), loopback(port));
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
        steps,
        ui,
        stop: Some(stop),
        task,
        _dir: dir,
    }
}

fn paired(name: &str, link: Option<Link>) -> PeerStatus {
    PeerStatus::Paired {
        name: name.into(),
        link,
    }
}

async fn until_peer(agent: &Agent, want: PeerStatus) {
    let mut rx = agent.subscribe_peer();
    within(rx.wait_for(|s| *s == want)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paired_agents_find_each_other_and_change_mode() {
    let (mac_id, pc_id) = (Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
    let (mac_port, pc_port) = (free_port(), free_port());
    let mut mac = start(
        Machine::Mac,
        &mac_id,
        mac_port,
        Some((pc_id.public_key(), pc_port)),
    )
    .await;
    let pc = start(
        Machine::Pc,
        &pc_id,
        pc_port,
        Some((mac_id.public_key(), mac_port)),
    )
    .await;

    // Cada uno ve al otro por el único enlace fijo, y la UI lo recibe.
    until_peer(&mac.agent, paired("pc", Some(Link::Usb4))).await;
    until_peer(&pc.agent, paired("mac", Some(Link::Usb4))).await;
    assert_eq!(
        mac.ui.request(UiCommand::GetPeer).await.unwrap(),
        Reply::Peer(paired("pc", Some(Link::Usb4)))
    );

    // La Mac ve aparecer a la PC y la da por prendida (pc_watch).
    let mut mac_state = mac.agent.subscribe();
    within(mac_state.wait_for(|s| s.pc_on)).await.unwrap();

    // El 34" a la PC: la Mac le pide sus pasos por el enlace. Darla por prendida toma
    // el orquestador un instante: si coincide, se reintenta.
    let mode = Mode::Shared([("m34".into(), Machine::Pc)].into());
    let txn = within(async {
        loop {
            match mac
                .ui
                .request(UiCommand::SetMode(mode.clone()))
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
    let committed = within(async {
        loop {
            match mac.ui.next_event().await.unwrap().unwrap() {
                AgentToUi::Txn(TxnEvent::Committed { txn: t, state }) if t == txn => break state,
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
    assert!(committed.pc_on);
    assert_eq!(committed.owner[&"m34".into()], Machine::Pc);
    assert_eq!(committed.owner[&"m32".into()], Machine::Mac);
    assert_eq!(mac.agent.state(), committed);

    let switch = Step::SwitchInput {
        monitor: "m34".into(),
        from: Machine::Mac,
        to: Machine::Pc,
        input: tandem_core::Input::DisplayPort,
    };
    let attach = Step::Attach {
        machine: Machine::Pc,
        monitor: "m34".into(),
    };
    let detach = Step::Detach {
        machine: Machine::Mac,
        monitor: "m34".into(),
    };
    assert_eq!(mac.steps.steps(), [detach, Step::UpdateInputGeometry]);
    assert_eq!(
        pc.steps.steps(),
        [switch, attach, Step::UpdateInputGeometry]
    );
    // La PC recibe el estado final de la Mac.
    let mut pc_state = pc.agent.subscribe();
    within(pc_state.wait_for(|s| *s == committed))
        .await
        .unwrap();

    // Al apagarse la PC, la Mac deja de verla enseguida y ya no tiene a quién pedirle pasos.
    let mac_agent = mac.agent.clone();
    pc.stop().await;
    until_peer(&mac_agent, paired("pc", None)).await;
    assert!(!*mac_agent.orchestrator().peer_connected().borrow());
    mac.stop().await;
}

#[tokio::test]
async fn unpaired_agent_works_alone_and_says_so() {
    let identity = Identity::from_seed([3; 32]);
    let mut a = start(Machine::Mac, &identity, free_port(), None).await;
    assert_eq!(a.agent.peer_status(), PeerStatus::Unpaired);
    assert_eq!(
        a.ui.request(UiCommand::GetPeer).await.unwrap(),
        Reply::Peer(PeerStatus::Unpaired)
    );
    assert_eq!(a.ui.get_state().await.unwrap(), a.agent.state());
    assert!(!*a.agent.orchestrator().peer_connected().borrow());
    // Lo que solo toca a esta máquina se hace igual: aquí, nada que cambiar.
    let reply =
        a.ui.request(UiCommand::SetMode(Mode::SoloMac))
            .await
            .unwrap();
    assert!(matches!(reply, Reply::Accepted { .. }), "{reply:?}");
    a.stop().await;
}

/// La primera vez el agente crea su identidad y la guarda; la siguiente la reusa.
#[tokio::test]
async fn identity_is_created_once_and_kept() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        config: dir.path().join("tandem.toml"),
        data_dir: dir.path().to_owned(),
    };
    let cfg = AgentConfig::from_toml(&format!("machine = \"pc\"\n{DESK}")).unwrap();
    let opts = NetOptions {
        bind: Some(loopback(0)),
        mdns: false,
        ..NetOptions::default()
    };
    let backends = || Backends::Custom {
        local: Arc::new(Fake::default()),
        manual: Arc::new(ManualSwitcher::new()),
        input: None,
    };
    let first = daemon::start(cfg.clone(), &paths, backends(), opts.clone()).unwrap();
    let key = Identity::load(&paths.identity_file()).unwrap().public_key();
    first.network.shutdown().await;
    let second = daemon::start(cfg, &paths, backends(), opts).unwrap();
    assert_eq!(
        Identity::load(&paths.identity_file()).unwrap().public_key(),
        key
    );
    assert_eq!(second.agent.peer_status(), PeerStatus::Unpaired);
}

/// Encender la PC termina cuando su agente aparece por un enlace, no antes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn power_on_waits_for_the_pc_agent() {
    let (mac_id, pc_id) = (Identity::from_seed([4; 32]), Identity::from_seed([5; 32]));
    let (mac_port, pc_port) = (free_port(), free_port());
    let mut mac = start(
        Machine::Mac,
        &mac_id,
        mac_port,
        Some((pc_id.public_key(), pc_port)),
    )
    .await;
    assert_eq!(mac.agent.peer_status(), paired("pc", None));
    let reply = mac.ui.request(UiCommand::SetPcPower(true)).await.unwrap();
    assert!(matches!(reply, Reply::Accepted { .. }), "{reply:?}");
    // El Wake-on-LAN salió y la transacción espera.
    within(async {
        while mac.steps.steps().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(!mac.agent.state().pc_on);

    let pc = start(
        Machine::Pc,
        &pc_id,
        pc_port,
        Some((mac_id.public_key(), mac_port)),
    )
    .await;
    let state = within(async {
        loop {
            match mac.ui.next_event().await.unwrap().unwrap() {
                AgentToUi::Txn(TxnEvent::Committed { state, .. }) => break state,
                AgentToUi::Txn(
                    e @ (TxnEvent::RolledBack { .. } | TxnEvent::Inconsistent { .. }),
                ) => {
                    panic!("no se encendió: {e:?}")
                }
                _ => {}
            }
        }
    })
    .await;
    assert!(state.pc_on);
    assert_eq!(mac.steps.steps()[0], Step::PowerOnPc);
    pc.stop().await;
    mac.stop().await;
}

/// El usuario frente a una UI conectada por la IPC: cambia la entrada a mano cuando se
/// le pide, hace clic en la sonda a pantalla completa si el monitor muestra su máquina
/// (`mine`) y, en la pregunta de la PC, dice qué ve salvo cuando ve la Mac (ahí hace
/// clic en la sonda de la Mac).
async fn calibration_user(
    ui: &mut IpcClient<tokio::io::DuplexStream>,
    sim: &tandem_display::calibrate::sim::SimMonitor,
    mine: tandem_core::Input,
) {
    use tandem_display::calibrate::sim::Showing;
    use tandem_proto::{ProbeAnswer, ProbeAsk, ProbeEvent, ProbePlace};
    loop {
        let Some(AgentToUi::Probe(ProbeEvent::Show { id, probe, place })) =
            ui.next_event().await.unwrap()
        else {
            continue;
        };
        if probe.ask == ProbeAsk::UserSwitch {
            sim.lock().showing = Showing::Input(probe.input);
        }
        let showing = sim.showing();
        let answer = match place {
            ProbePlace::Question => match showing {
                Showing::Input(tandem_core::Input::Hdmi) => None,
                Showing::Input(tandem_core::Input::DisplayPort) => {
                    Some(ProbeAnswer::Machine(Machine::Pc))
                }
                Showing::Black(_) => Some(ProbeAnswer::Black),
            },
            _ if probe.input == mine && showing == Showing::Input(mine) => Some(ProbeAnswer::Seen),
            _ => None,
        };
        if let Some(answer) = answer {
            // Puede llegar tarde (otro Show de la misma sonda): un rechazo no importa.
            let _ = ui
                .request(UiCommand::AnswerProbe { id, answer })
                .await
                .unwrap();
        }
    }
}

/// T-078 por QUIC real: la PC calibra el 32" simulado y la Mac confirma lo de HDMI con
/// clics de su UI, por la IPC y el enlace en loopback.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn calibration_is_confirmed_from_the_other_machine() {
    use tandem_core::{DdcProfile, Input};
    use tandem_display::calibrate::sim::{Showing, SimMonitor, samsung_32};
    use tandem_display::calibrate::{CalibrationOptions, CalibrationRequest, CancelToken};

    let (mac_id, pc_id) = (Identity::from_seed([5; 32]), Identity::from_seed([6; 32]));
    let (mac_port, pc_port) = (free_port(), free_port());
    let mut mac = start(
        Machine::Mac,
        &mac_id,
        mac_port,
        Some((pc_id.public_key(), pc_port)),
    )
    .await;
    let mut pc = start(
        Machine::Pc,
        &pc_id,
        pc_port,
        Some((mac_id.public_key(), mac_port)),
    )
    .await;
    until_peer(&mac.agent, paired("pc", Some(Link::Usb4))).await;
    until_peer(&pc.agent, paired("mac", Some(Link::Usb4))).await;

    // Con la lectura al día (no la vieja tras el menú) cada entrada sale al primer
    // intento: lo que se prueba aquí es el camino de las confirmaciones.
    let mut spec = samsung_32();
    spec.reports = tandem_display::calibrate::sim::Reports::CurrentCode;
    let sim = SimMonitor::new(spec, Input::DisplayPort);
    let req = CalibrationRequest {
        monitor: 0,
        start: Input::DisplayPort,
        known: Vec::new(),
        // Relojes reales: plazos cortos para que la prueba no tarde.
        options: CalibrationOptions {
            confirm_timeout: Duration::from_secs(5),
            user_switch_timeout: Duration::from_secs(5),
            settle: Duration::from_millis(50),
            retry_gap: Duration::from_millis(10),
            ..CalibrationOptions::default()
        },
    };
    let confirmer = tandemd::AgentConfirmer::new(pc.agent.clone(), Input::DisplayPort);
    let mut bus = sim.clone();
    let (cancel, mut on_event) = (CancelToken::new(), |_: &_| {});
    let result = within(async {
        tokio::select! {
            r = tandem_display::calibrate::calibrate(
                &mut bus, &req, &confirmer, &cancel, &mut on_event,
            ) => r,
            () = calibration_user(&mut mac.ui, &sim, Input::Hdmi) => unreachable!(),
            () = calibration_user(&mut pc.ui, &sim, Input::DisplayPort) => unreachable!(),
        }
    })
    .await;
    let profile = result.unwrap();
    assert_eq!(
        profile.ddc_profile(),
        Some(DdcProfile {
            hdmi: 0x05,
            display_port: 0x0F
        })
    );
    // Cada código se vio al primer intento: nada esperó el plazo.
    assert_eq!(profile.attempts, 2);
    assert_eq!(sim.showing(), Showing::Input(Input::DisplayPort));
    pc.stop().await;
    mac.stop().await;
}
