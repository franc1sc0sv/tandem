//! Dos agentes en el mismo proceso, unidos por QUIC real en loopback, con pasos
//! simulados: un cambio de modo pedido por la UI de la Mac se reparte entre las dos
//! máquinas, y un fallo en la PC deshace todo.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_core::{Machine, Mode, State, Step};
use tandem_link::identity::{Identity, PublicKey};
use tandem_link::transport::{Endpoint, PeerTrust};
use tandem_proto::{AgentToUi, Reply, TxnEvent, UiCommand};
use tandemd::ipc::{IpcClient, mem};
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::{Agent, AgentConfig, PeerClient, StateStore, daemon, peer};
use tokio::sync::oneshot;

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
                Err("DDC no responde".to_owned())
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
    ui: IpcClient<tokio::io::DuplexStream>,
    _dir: tempfile::TempDir,
    _endpoints: (Endpoint, Endpoint),
    _stop: oneshot::Sender<()>,
}

/// Levanta los dos agentes con `initial` guardado, los une por QUIC y conecta la UI a la Mac.
async fn desk(initial: impl Fn(&mut State), mac_steps: Fake, pc_steps: Fake) -> Desk {
    let dir = tempfile::tempdir().unwrap();
    let agent = |machine: Machine, steps: Arc<Fake>| {
        let cfg = AgentConfig::from_toml(&format!("machine = \"{machine}\"\n{DESK}")).unwrap();
        let store = StateStore::new(dir.path().join(format!("{machine}.json")));
        let mut s = State::initial(&cfg.desk);
        initial(&mut s);
        store.save(&s).unwrap();
        Arc::new(Agent::with_local_steps(cfg, store, steps))
    };
    let (mac_steps, pc_steps) = (Arc::new(mac_steps), Arc::new(pc_steps));
    let mac = agent(Machine::Mac, mac_steps.clone());
    let pc = agent(Machine::Pc, pc_steps.clone());

    // Identidades emparejadas y QUIC por loopback.
    let (mac_id, pc_id) = (Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
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
            PeerClient::open(&to_pc, Machine::Mac),
            PeerClient::open(&to_mac, Machine::Pc)
        )
    })
    .await;
    mac.set_peer(Arc::new(mac_client.unwrap()));
    pc.set_peer(Arc::new(pc_client.unwrap()));

    // La UI de la Mac, por la IPC en memoria.
    let (listener, connector) = mem::pair();
    let (stop, stopped) = oneshot::channel::<()>();
    tokio::spawn(daemon::run(mac.clone(), listener, async {
        let _ = stopped.await;
    }));
    let ui = IpcClient::handshake(connector.connect().unwrap())
        .await
        .unwrap();

    Desk {
        mac,
        pc,
        mac_steps,
        pc_steps,
        ui,
        _dir: dir,
        _endpoints: (mac_ep, pc_ep),
        _stop: stop,
    }
}

/// Pide el modo por la UI y junta los avisos de la transacción hasta su desenlace.
async fn apply(ui: &mut IpcClient<tokio::io::DuplexStream>, mode: Mode) -> Vec<TxnEvent> {
    let reply = within(ui.request(UiCommand::SetMode(mode))).await.unwrap();
    let Reply::Accepted { txn } = reply else {
        panic!("se esperaba Accepted: {reply:?}");
    };
    let mut events = Vec::new();
    loop {
        match within(ui.next_event()).await.unwrap() {
            Some(AgentToUi::Txn(e)) => {
                let end = matches!(
                    e,
                    TxnEvent::Committed { txn: t, .. }
                        | TxnEvent::RolledBack { txn: t, .. }
                        | TxnEvent::Inconsistent { txn: t, .. } if t == txn
                );
                events.push(e);
                if end {
                    return events;
                }
            }
            Some(AgentToUi::State(_)) => {}
            other => panic!("aviso inesperado: {other:?}"),
        }
    }
}

/// Espera a que el agente llegue a `want` (el otro agente recibe el estado final aparte).
async fn reaches(agent: &Agent, want: &State) {
    let mut rx = agent.subscribe();
    within(rx.wait_for(|s| s == want)).await.unwrap();
}

fn shared_34_to_pc() -> Mode {
    Mode::Shared([("m34".into(), Machine::Pc)].into())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn modo_aplicado_de_punta_a_punta() {
    // PC apagada: la Mac la enciende y después la PC cambia la entrada del 34".
    let mut d = desk(|_| {}, Fake::default(), Fake::default()).await;
    let events = apply(&mut d.ui, shared_34_to_pc()).await;

    let mut want = d.mac.state();
    assert!(want.pc_on);
    assert_eq!(want.owner[&"m34".into()], Machine::Pc);
    assert_eq!(want.owner[&"m32".into()], Machine::Mac);
    assert!(matches!(events.last(), Some(TxnEvent::Committed { state, .. }) if *state == want));
    let TxnEvent::Started { steps, .. } = &events[0] else {
        panic!("{events:?}");
    };
    let done = events
        .iter()
        .filter(|e| matches!(e, TxnEvent::StepDone { .. }))
        .count();
    assert_eq!(done, steps.len());

    // Cada paso corrió en su máquina.
    let m34 = || "m34".into();
    assert_eq!(
        d.mac_steps.steps(),
        [
            Step::PowerOnPc,
            Step::Detach {
                machine: Machine::Mac,
                monitor: m34()
            },
            Step::UpdateInputGeometry,
        ]
    );
    assert!(matches!(
        &d.pc_steps.steps()[..],
        [
            Step::SwitchInput {
                to: Machine::Pc,
                ..
            },
            Step::Attach {
                machine: Machine::Pc,
                ..
            },
            Step::UpdateInputGeometry,
        ]
    ));
    // La PC se entera del estado final.
    reaches(&d.pc, &want).await;

    // Y de vuelta a Solo Mac: la PC se queda prendida.
    let events = apply(&mut d.ui, Mode::SoloMac).await;
    assert!(matches!(events.last(), Some(TxnEvent::Committed { .. })));
    want.owner.insert(m34(), Machine::Mac);
    assert_eq!(d.mac.state(), want);
    reaches(&d.pc, &want).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fallo_remoto_vuelve_al_estado_inicial() {
    let attach_pc = Step::Attach {
        machine: Machine::Pc,
        monitor: "m34".into(),
    };
    let mut d = desk(
        |s| s.pc_on = true,
        Fake::default(),
        Fake::failing(attach_pc.clone()),
    )
    .await;
    let initial = d.mac.state();
    let events = apply(&mut d.ui, shared_34_to_pc()).await;

    let n = events.len();
    assert_eq!(
        events[n - 3..],
        [
            TxnEvent::StepFailed {
                txn: 1,
                index: 1,
                error: "pc: DDC no responde".into()
            },
            TxnEvent::RollingBack { txn: 1 },
            TxnEvent::RolledBack {
                txn: 1,
                state: initial.clone()
            },
        ]
    );
    assert_eq!(d.mac.state(), initial);
    reaches(&d.pc, &initial).await;
    // La PC cambió la entrada, falló al agregar el monitor y devolvió la entrada a la Mac.
    let pc = d.pc_steps.steps();
    assert_eq!(pc.len(), 3, "{pc:?}");
    assert_eq!(pc[1], attach_pc);
    assert!(matches!(
        pc[2],
        Step::SwitchInput {
            to: Machine::Mac,
            ..
        }
    ));
    assert!(d.mac_steps.steps().is_empty(), "la Mac no llegó a su paso");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn la_pc_tambien_orquesta() {
    // El mismo protocolo en el otro sentido: la PC pide a la Mac lo que le toca.
    let d = desk(|s| s.pc_on = true, Fake::default(), Fake::default()).await;
    let mut rx = d.pc.subscribe_txns();
    assert!(matches!(
        d.pc.handle(UiCommand::SetMode(Mode::SoloPc)),
        Reply::Accepted { .. }
    ));
    loop {
        match within(rx.recv()).await.unwrap() {
            TxnEvent::Committed { state, .. } => {
                assert!(state.owner.values().all(|m| *m == Machine::Pc));
                reaches(&d.mac, &state).await;
                break;
            }
            TxnEvent::RolledBack { .. } | TxnEvent::Inconsistent { .. } => panic!("falló"),
            _ => {}
        }
    }
    // La pantalla integrada de la Mac se encendió en la Mac.
    assert!(
        d.mac_steps
            .steps()
            .iter()
            .any(|s| matches!(s, Step::SetBuiltin { .. }))
    );
}
