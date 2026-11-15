//! Dos `tandemd` armados con [`daemon::start`], como en el binario: identidades
//! emparejadas en el directorio de datos, QUIC real en loopback y los enlaces fijos de
//! la config. Solo los pasos de cada máquina son simulados.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_core::{Machine, Side, Step};
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
    // Las dos con `[power.wol]` para despertar a la otra (los pasos son simulados).
    let text = format!("machine = \"{machine}\"\n{network}\n{WOL}");
    // En el disco, como en el binario: lo que se cambia desde la UI se guarda ahí.
    std::fs::write(&paths.config, &text).unwrap();
    let cfg = AgentConfig::from_toml(&text).unwrap();
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
async fn paired_agents_find_each_other_and_shut_down_the_pc() {
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

    // La Mac ve aparecer a la PC y la da por prendida (peer_watch), y viceversa.
    let mut mac_state = mac.agent.subscribe();
    within(mac_state.wait_for(|s| s.is_on(Machine::Pc)))
        .await
        .unwrap();
    let mut pc_state = pc.agent.subscribe();
    within(pc_state.wait_for(|s| s.is_on(Machine::Mac)))
        .await
        .unwrap();

    // Apagar la PC: la Mac le pide el paso por el enlace. Darla por prendida toma el
    // orquestador un instante: si coincide, se reintenta.
    let txn = within(async {
        loop {
            match mac
                .ui
                .request(UiCommand::SetPower {
                    machine: Machine::Pc,
                    on: false,
                })
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
                AgentToUi::Txn(e @ TxnEvent::Failed { .. }) => panic!("el apagado falló: {e:?}"),
                _ => {}
            }
        }
    })
    .await;
    assert!(!committed.on.pc);
    assert!(mac.steps.steps().is_empty());
    assert_eq!(
        pc.steps.steps(),
        [Step::PowerOff {
            machine: Machine::Pc
        }]
    );

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
        a.ui.request(UiCommand::SetPower {
            machine: Machine::Mac,
            on: true,
        })
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
    let cfg = AgentConfig::from_toml("machine = \"pc\"").unwrap();
    let opts = NetOptions {
        bind: Some(loopback(0)),
        mdns: false,
        ..NetOptions::default()
    };
    let backends = || Backends::Custom {
        local: Arc::new(Fake::default()),
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

/// Lo que hace falta para que una máquina despierte a la otra.
const WOL: &str = "[power.wol]\nmac = \"9c:6b:00:1a:2b:ff\"\n";

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
    let reply = mac
        .ui
        .request(UiCommand::SetPower {
            machine: Machine::Pc,
            on: true,
        })
        .await
        .unwrap();
    assert!(matches!(reply, Reply::Accepted { .. }), "{reply:?}");
    // El Wake-on-LAN salió y la transacción espera.
    within(async {
        while mac.steps.steps().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(!mac.agent.state().on.pc);

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
                AgentToUi::Txn(e @ TxnEvent::Failed { .. }) => panic!("no se encendió: {e:?}"),
                _ => {}
            }
        }
    })
    .await;
    assert!(state.on.pc);
    assert_eq!(
        mac.steps.steps()[0],
        Step::PowerOn {
            machine: Machine::Pc
        }
    );
    pc.stop().await;
    mac.stop().await;
}

/// Arranca un agente de la PC sin par en `bind`, con esos puertos de reserva.
fn start_on(
    dir: &std::path::Path,
    bind: SocketAddr,
    fallback_ports: Vec<u16>,
) -> Result<Daemon, String> {
    let paths = Paths {
        config: dir.join("tandem.toml"),
        data_dir: dir.to_owned(),
    };
    let cfg = AgentConfig::from_toml("machine = \"pc\"").unwrap();
    daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: Arc::new(Fake::default()),
            input: None,
        },
        NetOptions {
            bind: Some(bind),
            fallback_ports,
            mdns: false,
            ..NetOptions::default()
        },
    )
    .map_err(|e| e.to_string())
}

/// T-082: el puerto del agente ocupado (en la PC real, reservado por Windows) no impide
/// arrancar: se usa el primero libre de la lista de reserva, y es el que se anunciaría.
#[tokio::test]
async fn an_occupied_port_falls_back_to_a_free_one() {
    let dir = tempfile::tempdir().unwrap();
    let busy = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let taken = busy.local_addr().unwrap().port();
    let other = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let also_taken = other.local_addr().unwrap().port();
    let free = free_port();
    let d = start_on(dir.path(), loopback(taken), vec![also_taken, free]).unwrap();
    assert_eq!(d.network.local_addr(), loopback(free));
    assert_eq!(d.agent.peer_status(), PeerStatus::Unpaired);
    d.network.shutdown().await;
}

/// Si ningún puerto abre, el error dice cuáles se probaron, la causa y cómo arreglarlo.
#[tokio::test]
async fn no_free_port_explains_how_to_fix_it() {
    let dir = tempfile::tempdir().unwrap();
    let busy = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let taken = busy.local_addr().unwrap().port();
    let busy2 = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let taken2 = busy2.local_addr().unwrap().port();
    let Err(text) = start_on(dir.path(), loopback(taken), vec![taken2]) else {
        panic!("arrancó con todos los puertos ocupados");
    };
    for want in [
        taken.to_string(),
        taken2.to_string(),
        "excludedportrange".into(),
        "[network] port".into(),
        "tandem.toml".into(),
    ] {
        assert!(text.contains(&want), "falta «{want}» en: {text}");
    }
}

/// Lo que el puesto tiene en `tandem.toml` de `r` (lo que leería al volver a arrancar).
fn saved(r: &Running) -> AgentConfig {
    AgentConfig::load(&r._dir.path().join("tandem.toml")).unwrap()
}

async fn until_desk(r: &Running, want: &tandem_core::Config) {
    let mut rx = r.agent.subscribe_desk();
    within(rx.wait_for(|d| d == want)).await.unwrap();
}

/// T-084: el puesto es de las dos. Cambiar el acomodo en la Mac llega a la PC (en caliente
/// y en su `tandem.toml`), y al revés; lo solo-local no viaja.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_desk_changed_on_either_machine_reaches_the_other() {
    let (mac_id, pc_id) = (Identity::from_seed([5; 32]), Identity::from_seed([6; 32]));
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
    until_peer(&mac.agent, paired("pc", Some(Link::Usb4))).await;
    until_peer(&pc.agent, paired("mac", Some(Link::Usb4))).await;

    // Cada una ve lo que la otra puede: la PC se apaga sola.
    let mut caps = mac.agent.subscribe_capabilities();
    within(caps.wait_for(|c| c.peer.as_ref().is_some_and(|p| p.machine == Machine::Pc)))
        .await
        .unwrap();
    assert!(mac.agent.capabilities().peer.unwrap().power.power_off);

    // En la Mac: la Mac pasa a la derecha de la PC.
    let mut desk = mac.agent.desk();
    desk.arrangement.mac_side = Side::Right;
    mac.agent.set_desk(desk.clone()).unwrap();
    until_desk(&pc, &desk).await;
    for r in [&mac, &pc] {
        let cfg = saved(r);
        assert_eq!(cfg.desk, desk, "{}", cfg.machine);
        assert_eq!(cfg.desk_rev, 1);
        assert_eq!(cfg.desk_origin, Some(Machine::Mac));
    }
    // Lo solo-local sigue siendo de cada una.
    assert_eq!(saved(&pc).machine, Machine::Pc);
    assert_eq!(saved(&pc).network.port, pc_port);

    // Al revés, desde la PC: vuelve el acomodo de siempre.
    let mut back = desk.clone();
    back.arrangement.mac_side = Side::Left;
    pc.agent.set_desk(back.clone()).unwrap();
    until_desk(&mac, &back).await;
    let cfg = saved(&mac);
    assert_eq!(cfg.desk, back);
    assert_eq!((cfg.desk_rev, cfg.desk_origin), (2, Some(Machine::Pc)));
    // La UI de la Mac recibe el puesto nuevo empujado.
    within(async {
        loop {
            if let Some(AgentToUi::Desk(d)) = mac.ui.next_event().await.unwrap()
                && d == back
            {
                break;
            }
        }
    })
    .await;

    mac.stop().await;
    pc.stop().await;
}

/// T-084: la Mac se reconecta tras perderse un cambio (estaba apagada): al volver, se
/// pone al día con la versión más nueva de la PC.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_made_while_the_other_was_away_arrives_on_reconnect() {
    let (mac_id, pc_id) = (Identity::from_seed([7; 32]), Identity::from_seed([8; 32]));
    let (mac_port, pc_port) = (free_port(), free_port());
    let pc = start(
        Machine::Pc,
        &pc_id,
        pc_port,
        Some((mac_id.public_key(), mac_port)),
    )
    .await;
    // La Mac todavía no está: el cambio queda solo en la PC.
    let mut desk = pc.agent.desk();
    desk.arrangement.mac_side = Side::Right;
    pc.agent.set_desk(desk.clone()).unwrap();

    let mac = start(
        Machine::Mac,
        &mac_id,
        mac_port,
        Some((pc_id.public_key(), pc_port)),
    )
    .await;
    until_desk(&mac, &desk).await;
    assert_eq!(saved(&mac).desk, desk);
    assert_eq!(saved(&mac).desk_rev, 1);

    mac.stop().await;
    pc.stop().await;
}
