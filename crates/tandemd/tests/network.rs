//! Dos `tandemd` armados con [`daemon::start`], como en el binario: identidades
//! emparejadas en el directorio de datos, QUIC real en loopback y los enlaces fijos de
//! la config. Solo los pasos de cada máquina son simulados.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_core::DeskPatch;
use tandem_core::{Machine, Side, Step};
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_proto::{AgentToUi, Link, PeerStatus, Reply, TxnEvent, UiCommand};
use tandemd::daemon::{self, Backends, Daemon};
use tandemd::ipc::{IpcClient, mem};
use tandemd::network::NetOptions;
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::paths::Paths;
use tandemd::test_ports::{PortTaken, port_taken};
use tandemd::{Agent, AgentConfig, StoreError};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Las dos identidades ordenadas por clave: la primera es la del lado A (DECISIONS A1), así
/// cada prueba sabe qué lado le toca a cada agente.
fn sides(x: Identity, y: Identity) -> (Identity, Identity) {
    if x.public_key().as_bytes() < y.public_key().as_bytes() {
        (x, y)
    } else {
        (y, x)
    }
}

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
) -> Result<Running, PortTaken> {
    start_with(machine, identity, port, peer, "").await
}

/// Como [`start`], con `extra` justo después de `machine` en `tandem.toml` (claves de arriba
/// y tablas propias, como un archivo de una versión anterior).
async fn start_with(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: Option<(PublicKey, u16)>,
    extra: &str,
) -> Result<Running, PortTaken> {
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
    let text = format!("machine = \"{machine}\"\n{extra}\n{network}\n{WOL}");
    // En el disco, como en el binario: lo que se cambia desde la UI se guarda ahí.
    std::fs::write(&paths.config, &text).unwrap();
    let mut cfg = AgentConfig::from_toml(&text).unwrap();
    // A es una Mac y B una PC en estas pruebas: lo que puede cada una sale de su sistema, no
    // del de la máquina que corre la prueba (en macOS ninguna se apagaría sola).
    cfg.os = match machine {
        Machine::A => tandem_core::Os::MacOs,
        Machine::B => tandem_core::Os::Windows,
    };
    let steps = Arc::new(Fake::default());
    let started = daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: steps.clone(),
            input: None,
            env: None,
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
    Ok(Running {
        agent,
        steps,
        ui,
        stop: Some(stop),
        task,
        _dir: dir,
    })
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
    let (mac_id, pc_id) = sides(Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
    let (mut mac, pc) = tandemd::start_pair!(
        |mac_port, pc_port| start(
            Machine::A,
            &mac_id,
            mac_port,
            Some((pc_id.public_key(), pc_port)),
        ),
        start(
            Machine::B,
            &pc_id,
            pc_port,
            Some((mac_id.public_key(), mac_port)),
        )
    );

    // Cada uno ve al otro por el único enlace fijo, y la UI lo recibe.
    until_peer(&mac.agent, paired("B", Some(Link::Usb4))).await;
    until_peer(&pc.agent, paired("A", Some(Link::Usb4))).await;
    assert_eq!(
        mac.ui.request(UiCommand::GetPeer).await.unwrap(),
        Reply::Peer(paired("B", Some(Link::Usb4)))
    );

    // La Mac ve aparecer a la PC y la da por prendida (peer_watch), y viceversa.
    let mut mac_state = mac.agent.subscribe();
    within(mac_state.wait_for(|s| s.is_on(Machine::B)))
        .await
        .unwrap();
    let mut pc_state = pc.agent.subscribe();
    within(pc_state.wait_for(|s| s.is_on(Machine::A)))
        .await
        .unwrap();

    // Apagar la PC: la Mac le pide el paso por el enlace. Darla por prendida toma el
    // orquestador un instante: si coincide, se reintenta.
    let txn = within(async {
        loop {
            match mac
                .ui
                .request(UiCommand::SetPower {
                    machine: Machine::B,
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
    assert!(!committed.on.b);
    assert!(mac.steps.steps().is_empty());
    assert_eq!(
        pc.steps.steps(),
        [Step::PowerOff {
            machine: Machine::B
        }]
    );

    // Al apagarse la PC, la Mac deja de verla enseguida y ya no tiene a quién pedirle pasos.
    let mac_agent = mac.agent.clone();
    pc.stop().await;
    until_peer(&mac_agent, paired("B", None)).await;
    assert!(!*mac_agent.orchestrator().peer_connected().borrow());
    mac.stop().await;
}

#[tokio::test]
async fn unpaired_agent_works_alone_and_says_so() {
    let identity = Identity::from_seed([3; 32]);
    let mut a = tandemd::start_solo!(|port, _peer| start(Machine::A, &identity, port, None));
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
            machine: Machine::A,
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
    let cfg = AgentConfig::from_toml("machine = \"b\"").unwrap();
    let opts = NetOptions {
        bind: Some(loopback(0)),
        mdns: false,
        ..NetOptions::default()
    };
    let backends = || Backends::Custom {
        local: Arc::new(Fake::default()),
        input: None,
        env: None,
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
    let (mac_id, pc_id) = sides(Identity::from_seed([4; 32]), Identity::from_seed([5; 32]));
    let (mut mac, mac_port, pc_port) = tandemd::start_solo!(|mac_port, pc_port| async {
        let r = start(
            Machine::A,
            &mac_id,
            mac_port,
            Some((pc_id.public_key(), pc_port)),
        )
        .await;
        r.map(|r| (r, mac_port, pc_port))
    });
    assert_eq!(mac.agent.peer_status(), paired("B", None));
    let reply = mac
        .ui
        .request(UiCommand::SetPower {
            machine: Machine::B,
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
    assert!(!mac.agent.state().on.b);

    // La PC arranca después, a propósito: si otra suite tomó su puerto mientras tanto,
    // falla enseguida (sin puertos de reserva) en vez de colgarse.
    let pc = start(
        Machine::B,
        &pc_id,
        pc_port,
        Some((mac_id.public_key(), mac_port)),
    )
    .await
    .expect("el puerto de la PC se tomó mientras tanto");
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
    assert!(state.on.b);
    assert_eq!(
        mac.steps.steps()[0],
        Step::PowerOn {
            machine: Machine::B
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
    let cfg = AgentConfig::from_toml("machine = \"b\"").unwrap();
    daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: Arc::new(Fake::default()),
            input: None,
            env: None,
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

/// Lo mismo, mirando solo el acomodo (las pantallas y los lados): qué máquina es cada lado
/// se anota aparte, cuando las dos se anuncian.
async fn until_layout(r: &Running, want: &tandem_core::Config) {
    let mut rx = r.agent.subscribe_desk();
    within(rx.wait_for(|d| d.screens == want.screens && d.arrangement == want.arrangement))
        .await
        .unwrap();
}

/// Espera a que las dos tengan anotadas a las dos máquinas en el puesto, con la misma
/// versión: a partir de ahí lo que cambie es del usuario.
async fn until_both_known(a: &Running, b: &Running) {
    within(async {
        loop {
            let (da, db) = (a.agent.desk_sync(), b.agent.desk_sync());
            let known =
                |d: &tandem_core::Config| Machine::ALL.iter().all(|m| d.machines.get(*m).is_some());
            if known(&da.desk) && da.stamp.rev == db.stamp.rev && da.desk == db.desk {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
}

/// T-084: el puesto es de las dos. Cambiar el acomodo en la Mac llega a la PC (en caliente
/// y en su `tandem.toml`), y al revés; lo solo-local no viaja.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_desk_changed_on_either_machine_reaches_the_other() {
    let (mac_id, pc_id) = sides(Identity::from_seed([5; 32]), Identity::from_seed([6; 32]));
    let (mut mac, pc) = tandemd::start_pair!(
        |mac_port, pc_port| start(
            Machine::A,
            &mac_id,
            mac_port,
            Some((pc_id.public_key(), pc_port)),
        ),
        start(
            Machine::B,
            &pc_id,
            pc_port,
            Some((mac_id.public_key(), mac_port)),
        )
    );
    until_peer(&mac.agent, paired("B", Some(Link::Usb4))).await;
    until_peer(&pc.agent, paired("A", Some(Link::Usb4))).await;

    // Cada una ve lo que la otra puede: la PC se apaga sola.
    let mut caps = mac.agent.subscribe_capabilities();
    within(caps.wait_for(|c| c.peer.as_ref().is_some_and(|p| p.machine == Machine::B)))
        .await
        .unwrap();
    assert!(mac.agent.capabilities().peer.unwrap().power.power_off);
    // Al conocerse, las dos quedan anotadas en el puesto (en las dos máquinas).
    until_both_known(&mac, &pc).await;
    let base = mac.agent.desk_sync().stamp.rev;
    assert_eq!(
        mac.agent.machine_info(Machine::B),
        Some(mac.agent.capabilities().peer.unwrap().info)
    );

    // En la Mac: la Mac pasa a la derecha de la PC.
    let mut desk = mac.agent.desk();
    desk.arrangement = Some(tandem_core::Arrangement {
        a_side: Side::Right,
        align: tandem_core::Align::Center,
    });
    mac.agent.set_desk(desk.clone()).unwrap();
    until_desk(&pc, &desk).await;
    for r in [&mac, &pc] {
        let cfg = saved(r);
        assert_eq!(cfg.desk, desk, "{}", cfg.machine);
        assert_eq!(cfg.desk_rev, base + 1);
        assert_eq!(cfg.desk_origin, Some(Machine::A));
    }
    // Lo solo-local sigue siendo de cada una.
    assert_eq!(saved(&pc).machine, Machine::B);
    assert_eq!(saved(&pc).network.port, pc.agent.config().network.port);

    // Al revés, desde la PC: vuelve el acomodo de siempre.
    let mut back = desk.clone();
    back.arrangement = None;
    pc.agent.set_desk(back.clone()).unwrap();
    until_desk(&mac, &back).await;
    let cfg = saved(&mac);
    assert_eq!(cfg.desk, back);
    assert_eq!(
        (cfg.desk_rev, cfg.desk_origin),
        (base + 2, Some(Machine::B))
    );
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
    let (mac_id, pc_id) = sides(Identity::from_seed([7; 32]), Identity::from_seed([8; 32]));
    let (pc, mac_port, pc_port) = tandemd::start_solo!(|mac_port, pc_port| async {
        let r = start(
            Machine::B,
            &pc_id,
            pc_port,
            Some((mac_id.public_key(), mac_port)),
        )
        .await;
        r.map(|r| (r, mac_port, pc_port))
    });
    // La Mac todavía no está: el cambio queda solo en la PC.
    let mut desk = pc.agent.desk();
    desk.arrangement = Some(tandem_core::Arrangement {
        a_side: Side::Right,
        align: tandem_core::Align::Center,
    });
    pc.agent.set_desk(desk.clone()).unwrap();

    // La Mac arranca después, a propósito: si otra suite tomó su puerto mientras tanto,
    // falla enseguida (sin puertos de reserva) en vez de colgarse.
    let mac = start(
        Machine::A,
        &mac_id,
        mac_port,
        Some((pc_id.public_key(), pc_port)),
    )
    .await
    .expect("el puerto de la Mac se tomó mientras tanto");
    until_layout(&mac, &desk).await;
    // Además del cambio, al conocerse se anotan las dos máquinas: una versión más.
    until_both_known(&mac, &pc).await;
    let got = saved(&mac);
    assert_eq!(got.desk.arrangement, desk.arrangement);
    assert!(got.desk_rev >= 1);
    assert_eq!(got.desk, pc.agent.desk());

    mac.stop().await;
    pc.stop().await;
}

/// El siguiente aviso «Cambiado desde X» que recibe la UI de `r`: quién, versión, qué y a
/// qué versión volver.
async fn next_desk_changed(r: &mut Running) -> (Machine, u64, String, u64) {
    within(async {
        loop {
            if let Some(AgentToUi::DeskChanged {
                by,
                rev,
                summary,
                undo_to,
            }) = r.ui.next_event().await.unwrap()
            {
                return (by, rev, summary, undo_to);
            }
        }
    })
    .await
}

async fn until_same_desk(a: &Running, b: &Running) {
    within(async {
        while a.agent.desk_sync() != b.agent.desk_sync() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
}

/// T-099 (DESIGN §4.4, D5): el puesto se edita desde la UI de cualquiera de las dos. Lo que
/// se edita en A se ve en B; si se edita en las dos a la vez, gana el último y las dos
/// quedan iguales, B recibe el aviso con «Deshacer», y deshacer en B vuelve exactamente a
/// su versión, también en A.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desk_edits_from_either_ui_converge_and_the_other_gets_the_notice() {
    let (a_id, b_id) = sides(Identity::from_seed([41; 32]), Identity::from_seed([42; 32]));
    let (mut a, mut b) = tandemd::start_pair!(
        |a_port, b_port| start(Machine::A, &a_id, a_port, Some((b_id.public_key(), b_port))),
        start(Machine::B, &b_id, b_port, Some((a_id.public_key(), a_port)))
    );
    until_peer(&a.agent, paired("B", Some(Link::Usb4))).await;
    until_peer(&b.agent, paired("A", Some(Link::Usb4))).await;
    until_both_known(&a, &b).await;

    // Editar en A (por su UI) se ve en B, en caliente y en su archivo, con el aviso.
    let arr = tandem_core::Arrangement {
        a_side: Side::Up,
        align: tandem_core::Align::End,
    };
    let Reply::Desk(edited) =
        a.ui.request(UiCommand::EditDesk {
            patch: DeskPatch::Arrangement(Some(arr)),
        })
        .await
        .unwrap()
    else {
        panic!("la edición no devolvió el puesto");
    };
    assert_eq!(edited.arrangement, Some(arr));
    until_desk(&b, &edited).await;
    assert_eq!(saved(&b).desk.arrangement, Some(arr));
    let (by, rev, summary, _) = next_desk_changed(&mut b).await;
    assert_eq!((by, summary.as_str()), (Machine::A, "Acomodo"));
    assert_eq!(rev, a.agent.desk_sync().stamp.rev);

    // Las dos a la vez: primero B y enseguida A, sin esperar a que se vean. Gana A: o
    // empatan en versión y A tiene la fecha igual o más nueva (y el empate es de A), o A
    // ya había recibido la de B y la suya es la siguiente.
    let prefs = tandem_core::InputPrefs {
        drag_files: false,
        ..tandem_core::InputPrefs::default()
    };
    b.agent.edit_desk(DeskPatch::Prefs(prefs)).unwrap();
    let mine_on_b = b.agent.desk_sync();
    a.agent
        .edit_desk(DeskPatch::Shortcuts(tandem_core::Shortcuts {
            lock: vec!["KeyL".into()],
        }))
        .unwrap();
    until_same_desk(&a, &b).await;
    let last = a.agent.desk_sync();
    assert_eq!(last.stamp.by, Machine::A);
    assert_eq!(last.desk.shortcuts.lock, ["KeyL"]);
    assert_eq!(saved(&b).desk, last.desk);
    // B recibe el aviso, y deshacer lleva a su propia versión.
    let (by, rev, _, undo_to) = next_desk_changed(&mut b).await;
    assert_eq!((by, rev), (Machine::A, last.stamp.rev));
    assert_eq!(undo_to, mine_on_b.stamp.rev);
    let Reply::Desk(back) =
        b.ui.request(UiCommand::UndoDesk { to_rev: undo_to })
            .await
            .unwrap()
    else {
        panic!("no se deshizo");
    };
    assert_eq!(back, mine_on_b.desk);
    until_same_desk(&a, &b).await;
    assert_eq!(a.agent.desk(), mine_on_b.desk);
    assert!(!saved(&a).desk.prefs.drag_files);
    // Ahora el aviso es para A.
    let (by, _, summary, _) = next_desk_changed(&mut a).await;
    assert_eq!(by, Machine::B);
    assert!(!summary.is_empty());
    let Reply::DeskHistory(h) = a.ui.request(UiCommand::GetDeskHistory).await.unwrap() else {
        panic!("sin historial");
    };
    assert_eq!(h[0].stamp, a.agent.desk_sync().stamp);

    a.stop().await;
    b.stop().await;
}

/// Un `tandem.toml` de antes de T-099, con las dos máquinas ya conocidas y la misma versión
/// del puesto. `input` va en su `[input]`.
fn before_t099(input: &str) -> String {
    format!(
        "desk_rev = 3\ndesk_origin = \"a\"\ndesk_sum = \"0123456789abcdef\"\n\n\
         [machines.a]\nos = \"macos\"\nname = \"Mac\"\ncolor = \"#7aa7e6\"\n\n\
         [machines.b]\nos = \"windows\"\nname = \"PC\"\ncolor = \"#a99be8\"\n\n\
         [arrangement]\na_side = \"left\"\noffset = 0\n\n\
         [input]\nlock_shortcut = [\"ScrollLock\"]\n{input}\n"
    )
}

/// Revisión de #74: antes de T-099 el portapapeles era de cada máquina (`[input]
/// clipboard`); ahora es un módulo de cada una, en el puesto (`[modules.b] clipboard`). Si una lo tenía apagado y la otra no, las dos
/// arrancaban con el mismo sello y distinto puesto, y no convergían nunca. Al conectarse
/// tienen que quedar iguales, con el portapapeles como lo dejó quien lo había cambiado.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_local_setting_moved_to_the_desk_does_not_leave_the_machines_apart() {
    let (a_id, b_id) = sides(Identity::from_seed([43; 32]), Identity::from_seed([44; 32]));
    let (a, b) = tandemd::start_pair!(
        |a_port, b_port| start_with(
            Machine::A,
            &a_id,
            a_port,
            Some((b_id.public_key(), b_port)),
            &before_t099(""),
        ),
        start_with(
            Machine::B,
            &b_id,
            b_port,
            Some((a_id.public_key(), a_port)),
            &before_t099("clipboard = false"),
        )
    );
    until_peer(&a.agent, paired("B", Some(Link::Usb4))).await;
    until_peer(&b.agent, paired("A", Some(Link::Usb4))).await;
    until_same_desk(&a, &b).await;
    assert_eq!(
        a.agent.desk().modules.get(Machine::B, "clipboard"),
        Some(false)
    );
    assert_eq!(
        saved(&a).desk.modules.get(Machine::B, "clipboard"),
        Some(false)
    );
    // Cada una aplica lo de su lado: B lo tiene apagado, A no.
    assert!(!b.agent.module_on(tandem_proto::ModuleId::Clipboard));
    assert!(a.agent.module_on(tandem_proto::ModuleId::Clipboard));
    a.stop().await;
    b.stop().await;
}

/// T-101: un cambio de `[data]` llega al `Qos` de todas las conexiones, también a la que
/// cada agente aceptó del otro (comparten el `Qos` de su camino).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_wifi_data_cap_reaches_the_connections_already_open() {
    use tandem_link::manager::LinkKind;

    let (a_id, b_id) = sides(Identity::from_seed([7; 32]), Identity::from_seed([8; 32]));
    let (a, b) = tandemd::start_pair!(
        |a_port, b_port| start(Machine::A, &a_id, a_port, Some((b_id.public_key(), b_port))),
        start(Machine::B, &b_id, b_port, Some((a_id.public_key(), a_port)))
    );
    until_peer(&a.agent, paired("B", Some(Link::Usb4))).await;
    until_peer(&b.agent, paired("A", Some(Link::Usb4))).await;
    until_both_known(&a, &b).await;

    // Las conexiones ya están abiertas (cada una la suya y la que aceptó de la otra) y usan
    // el `Qos` de su camino, el mismo que se pide aquí.
    let b_usb4 = b.agent.link_qos().get(LinkKind::Usb4);
    let b_wifi = b.agent.link_qos().get(LinkKind::Wifi);
    let before = b_wifi.cap().unwrap();

    let data = tandem_core::DataPrefs {
        wifi_cap_percent: 10,
        wifi_cap_mbps: None,
    };
    let desk = a
        .agent
        .edit_desk(tandem_core::DeskPatch::Data(data))
        .unwrap();
    assert_eq!(desk.data, data);
    until_desk(&b, &desk).await;
    within(async {
        while b_usb4.prefs() != desk.data || a.agent.link_qos().prefs() != desk.data {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert_eq!(b_wifi.prefs(), desk.data);
    assert_eq!(b_wifi.cap(), Some(before / 5));
    a.stop().await;
    b.stop().await;
}
