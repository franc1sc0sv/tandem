//! Dos agentes en el mismo proceso, unidos por QUIC real en loopback, con pasos
//! simulados: encender y apagar pedidos por la UI se reparten entre las dos máquinas, y un
//! fallo en la otra deja el estado como estaba.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_core::{Machine, State, Step};
use tandem_link::identity::{Identity, PublicKey};
use tandem_link::transport::{Endpoint, PeerTrust};
use tandem_proto::{AgentToUi, Reply, TxnEvent, UiCommand};
use tandemd::ipc::{IpcClient, mem};
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::{Agent, AgentConfig, PeerClient, StateStore, daemon, peer};
use tokio::sync::oneshot;

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

/// Pasos simulados de una máquina: registran lo que se les pide y fallan donde se diga.
#[derive(Default)]
struct Fake {
    log: Mutex<Vec<Step>>,
    fail_on: Vec<Step>,
}

impl Fake {
    fn failing(step: Step) -> Self {
        Fake {
            fail_on: vec![step],
            ..Default::default()
        }
    }

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
            if self.fail_on.contains(step) {
                Err("acceso denegado".to_owned())
            } else {
                Ok(())
            }
        })
    }
}

struct Keys(BTreeSet<PublicKey>);

impl PeerTrust for Keys {
    fn is_trusted(&self, key: &PublicKey) -> bool {
        self.0.contains(key)
    }
}

struct Desk {
    mac: Arc<Agent>,
    pc: Arc<Agent>,
    mac_steps: Arc<Fake>,
    pc_steps: Arc<Fake>,
    ui: Ui,
    _dir: tempfile::TempDir,
    _link: Link,
}

type Ui = IpcClient<tokio::io::DuplexStream>;

/// Mantiene vivos los extremos QUIC y los daemons de IPC.
struct Link {
    _endpoints: (Endpoint, Endpoint),
    _stops: Vec<oneshot::Sender<()>>,
}

/// Configuración de `machine` y su almacén en `dir`, con `initial` guardado.
fn setup(
    dir: &std::path::Path,
    machine: Machine,
    initial: &impl Fn(&mut State),
) -> (AgentConfig, StateStore) {
    // Con `[power.wol]`: cualquiera de las dos despierta a la otra (los pasos son simulados).
    let cfg = AgentConfig::from_toml(&format!(
        "machine = \"{machine}\"\n[power.wol]\nmac = \"aa:bb:cc:dd:ee:ff\"\n"
    ))
    .unwrap();
    let store = StateStore::new(dir.join(format!("{machine}.json")));
    let mut s = State::initial(machine);
    initial(&mut s);
    store.save(&s).unwrap();
    (cfg, store)
}

/// Levanta los dos agentes con `initial` guardado, los une por QUIC y conecta la UI a la Mac.
async fn desk(initial: impl Fn(&mut State), mac_steps: Fake, pc_steps: Fake) -> Desk {
    let dir = tempfile::tempdir().unwrap();
    let agent = |machine: Machine, steps: Arc<Fake>| {
        let (cfg, store) = setup(dir.path(), machine, &initial);
        Arc::new(Agent::with_local_steps(cfg, store, steps))
    };
    let (mac_steps, pc_steps) = (Arc::new(mac_steps), Arc::new(pc_steps));
    let mac = agent(Machine::A, mac_steps.clone());
    let pc = agent(Machine::B, pc_steps.clone());
    let (ui, _, link) = link(&mac, &pc).await;
    Desk {
        mac,
        pc,
        mac_steps,
        pc_steps,
        ui,
        _dir: dir,
        _link: link,
    }
}

/// Une los dos agentes por QUIC en loopback y conecta una UI a cada uno (Mac, PC).
async fn link(mac: &Arc<Agent>, pc: &Arc<Agent>) -> (Ui, Ui, Link) {
    // Identidades emparejadas y QUIC por loopback.
    let (mac_id, pc_id) = sides(Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
    let trust = |id: &Identity| Arc::new(Keys(BTreeSet::from([id.public_key()])));
    let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    let mac_ep = Endpoint::bind(loopback, &mac_id, trust(&pc_id)).unwrap();
    let pc_ep = Endpoint::bind(loopback, &pc_id, trust(&mac_id)).unwrap();
    let (to_pc, to_mac) =
        within(async { tokio::join!(mac_ep.connect(pc_ep.local_addr().unwrap()), pc_ep.accept()) })
            .await;
    let (to_pc, to_mac) = (to_pc.unwrap(), to_mac.unwrap().unwrap());

    // Cada agente atiende los streams del otro y abre el suyo para pedir pasos.
    tokio::spawn(peer::accept_streams(to_pc.clone(), mac.clone()));
    tokio::spawn(peer::accept_streams(to_mac.clone(), pc.clone()));
    let (mac_client, pc_client) = within(async {
        tokio::join!(
            PeerClient::open(&to_pc, Machine::A),
            PeerClient::open(&to_mac, Machine::B)
        )
    })
    .await;
    mac.set_peer(Arc::new(mac_client.unwrap()));
    pc.set_peer(Arc::new(pc_client.unwrap()));

    // Una UI en cada máquina, por la IPC en memoria.
    let mut stops = Vec::new();
    let mut ui = async |agent: &Arc<Agent>| {
        let (listener, connector) = mem::pair();
        let (stop, stopped) = oneshot::channel::<()>();
        stops.push(stop);
        tokio::spawn(daemon::run(agent.clone(), listener, async {
            let _ = stopped.await;
        }));
        IpcClient::handshake(connector.connect().unwrap())
            .await
            .unwrap()
    };
    let ui_mac = ui(mac).await;
    let ui_pc = ui(pc).await;
    (
        ui_mac,
        ui_pc,
        Link {
            _endpoints: (mac_ep, pc_ep),
            _stops: stops,
        },
    )
}

/// Pide encender o apagar `machine` por la UI y junta los avisos de la transacción hasta su
/// desenlace.
async fn power(ui: &mut Ui, machine: Machine, on: bool) -> Vec<TxnEvent> {
    let reply = within(ui.request(UiCommand::SetPower { machine, on }))
        .await
        .unwrap();
    let Reply::Accepted { txn } = reply else {
        panic!("se esperaba Accepted: {reply:?}");
    };
    let mut events = Vec::new();
    loop {
        match within(ui.next_event()).await.unwrap() {
            Some(AgentToUi::Txn(e)) => {
                let end = matches!(
                    e,
                    TxnEvent::Committed { txn: t, .. } | TxnEvent::Failed { txn: t, .. } if t == txn
                );
                events.push(e);
                if end {
                    return events;
                }
            }
            // El anuncio de capacidades de la otra máquina llega al conectarse (T-084), y con
            // él se anotan las dos máquinas en el puesto (T-097). También su versión (T-122).
            Some(
                AgentToUi::State(_)
                | AgentToUi::Machines { .. }
                | AgentToUi::Desk(_)
                | AgentToUi::Update(_),
            ) => {}
            other => panic!("aviso inesperado: {other:?}"),
        }
    }
}

/// Espera a que el agente llegue a `want` (el otro agente recibe el estado final aparte).
async fn reaches(agent: &Agent, want: &State) {
    let mut rx = agent.subscribe();
    within(rx.wait_for(|s| s == want)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn encender_la_pc_de_punta_a_punta() {
    // La Mac la cree apagada: manda el Wake-on-LAN y espera a que su agente conteste (ya
    // está conectado).
    let mut d = desk(|s| s.on.b = false, Fake::default(), Fake::default()).await;
    let events = power(&mut d.ui, Machine::B, true).await;
    let want = d.mac.state();
    assert!(want.on.b);
    assert!(matches!(events.last(), Some(TxnEvent::Committed { state, .. }) if *state == want));
    assert_eq!(
        d.mac_steps.steps(),
        [Step::PowerOn {
            machine: Machine::B
        }]
    );
    assert!(d.pc_steps.steps().is_empty());
    // La PC se entera del estado final.
    reaches(&d.pc, &want).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apagar_la_pc_corre_en_la_pc() {
    let mut d = desk(|s| s.on.b = true, Fake::default(), Fake::default()).await;
    let events = power(&mut d.ui, Machine::B, false).await;
    assert!(matches!(events.last(), Some(TxnEvent::Committed { .. })));
    assert!(!d.mac.state().on.b);
    assert_eq!(
        d.pc_steps.steps(),
        [Step::PowerOff {
            machine: Machine::B
        }]
    );
    assert!(d.mac_steps.steps().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn un_fallo_remoto_deja_el_estado_como_estaba() {
    let off = Step::PowerOff {
        machine: Machine::B,
    };
    let mut d = desk(
        |s| s.on.b = true,
        Fake::default(),
        Fake::failing(off.clone()),
    )
    .await;
    let initial = d.mac.state();
    let events = power(&mut d.ui, Machine::B, false).await;
    let n = events.len();
    assert_eq!(
        events[n - 2..],
        [
            TxnEvent::StepFailed {
                txn: 1,
                index: 0,
                error: "máquina B: acceso denegado".into()
            },
            TxnEvent::Failed {
                txn: 1,
                state: initial.clone()
            },
        ]
    );
    assert_eq!(d.mac.state(), initial);
    assert_eq!(d.pc_steps.steps(), [off]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn la_pc_tambien_orquesta() {
    // El mismo protocolo en el otro sentido: la PC le pide a la Mac que se apague.
    let d = desk(|s| s.on.a = true, Fake::default(), Fake::default()).await;
    let mut rx = d.pc.subscribe_txns();
    assert!(matches!(
        d.pc.handle(UiCommand::SetPower {
            machine: Machine::A,
            on: false
        }),
        Reply::Accepted { .. }
    ));
    loop {
        match within(rx.recv()).await.unwrap() {
            TxnEvent::Committed { state, .. } => {
                assert!(!state.on.a);
                break;
            }
            TxnEvent::Failed { .. } => panic!("falló"),
            _ => {}
        }
    }
    assert_eq!(
        d.mac_steps.steps(),
        [Step::PowerOff {
            machine: Machine::A
        }]
    );
}
