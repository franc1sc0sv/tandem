//! Dos agentes en el mismo proceso, unidos por QUIC real en loopback, con pasos
//! simulados: un cambio de modo pedido por la UI de la Mac se reparte entre las dos
//! máquinas, y un fallo en la PC deshace todo.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_core::{BuiltinDisplay, Machine, Mode, MonitorConfig, State, Step};
use tandem_display::ddc::fake::{FakeDdc, FakeMonitor, SharedFakeDdc};
use tandem_display::ddc::{Edid, MonitorInfo, VCP_INPUT_SOURCE};
use tandem_display::desktop::{DesktopControl, DesktopError};
use tandem_display::switcher::{DdcSwitcher, ManualSwitcher, PerMonitor};
use tandem_link::identity::{Identity, PublicKey};
use tandem_link::transport::{Endpoint, PeerTrust};
use tandem_proto::{AgentToUi, ManualSwitchEvent, Reply, TxnEvent, UiCommand};
use tandemd::ipc::{IpcClient, mem};
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::{Agent, AgentConfig, DisplaySteps, PeerClient, StateStore, daemon, peer};
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

/// Configuración de `machine` con `desk` y su almacén en `dir`, con `initial` guardado.
fn setup(
    dir: &std::path::Path,
    machine: Machine,
    desk: &str,
    initial: &impl Fn(&mut State),
) -> (AgentConfig, StateStore) {
    // Con `[power.wol]`: cualquiera de las dos despierta a la otra (los pasos son simulados).
    let cfg = AgentConfig::from_toml(&format!(
        "machine = \"{machine}\"\n{desk}\n[power.wol]\nmac = \"9c:6b:00:1a:2b:ff\"\n"
    ))
    .unwrap();
    let store = StateStore::new(dir.join(format!("{machine}.json")));
    let mut s = State::initial(&cfg.desk, Machine::Mac);
    initial(&mut s);
    store.save(&s).unwrap();
    (cfg, store)
}

/// Levanta los dos agentes con `initial` guardado, los une por QUIC y conecta la UI a la Mac.
async fn desk(initial: impl Fn(&mut State), mac_steps: Fake, pc_steps: Fake) -> Desk {
    let dir = tempfile::tempdir().unwrap();
    let agent = |machine: Machine, steps: Arc<Fake>| {
        let (cfg, store) = setup(dir.path(), machine, DESK, &initial);
        Arc::new(Agent::with_local_steps(cfg, store, steps))
    };
    let (mac_steps, pc_steps) = (Arc::new(mac_steps), Arc::new(pc_steps));
    let mac = agent(Machine::Mac, mac_steps.clone());
    let pc = agent(Machine::Pc, pc_steps.clone());
    // Lo guardado vuelve sin confirmar (T-085); aquí se da por sabido, para ver solo los
    // pasos de lo que cambia.
    for a in [&mac, &pc] {
        a.set_state(a.state().confirmed()).unwrap();
    }
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

/// Pide el modo por la UI y junta los avisos de la transacción hasta su desenlace.
async fn apply(ui: &mut Ui, mode: Mode) -> Vec<TxnEvent> {
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
            // El anuncio de capacidades de la otra máquina llega al conectarse (T-084).
            Some(AgentToUi::State(_) | AgentToUi::Capabilities(_)) => {}
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
    assert!(want.on.pc);
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
            Step::PowerOn {
                machine: Machine::Pc
            },
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
        |s| s.on.pc = true,
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
    let d = desk(|s| s.on.pc = true, Fake::default(), Fake::default()).await;
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

/// El puesto con el perfil DDC del 34"; el 32" no tiene perfil y se cambia a mano.
const DESK_DDC: &str = r#"
[[monitors]]
id = "m34"
label = '34"'
edid_model = "LC34G55T"
wiring = { pc = "displayport", mac = "hdmi" }
ddc = { hdmi = 0x06, display_port = 0x0F }

[[monitors]]
id = "m32"
label = '32"'
wiring = { pc = "displayport", mac = "hdmi" }
"#;

/// Escritorio simulado: registra cada llamada.
#[derive(Default)]
struct Escritorio(Mutex<Vec<String>>);

impl Escritorio {
    fn push(&self, s: String) -> Result<(), DesktopError> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(s);
        Ok(())
    }

    fn calls(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl DesktopControl for Escritorio {
    fn attach(&self, m: &MonitorConfig) -> Result<(), DesktopError> {
        self.push(format!("+{}", m.id))
    }
    fn detach(&self, m: &MonitorConfig) -> Result<(), DesktopError> {
        self.push(format!("-{}", m.id))
    }
    fn set_builtin(&self, mode: BuiltinDisplay) -> Result<(), DesktopError> {
        self.push(format!("integrada {mode:?}"))
    }
}

/// El bus DDC de la PC con el 34" (índice 1) y otro monitor que no es.
fn bus_de_la_pc() -> SharedFakeDdc {
    let named = |name: &str| FakeMonitor {
        info: Some(MonitorInfo {
            index: 0,
            description: "Monitor PnP genérico".into(),
            device: String::new(),
            edid: Some(Edid {
                manufacturer: "SAM".into(),
                product_code: 0x7259,
                serial_number: 0,
                name: Some(name.into()),
                serial_text: None,
                size_cm: None,
            }),
        }),
        ..Default::default()
    };
    SharedFakeDdc::new(FakeDdc::new(vec![named("S32BG85"), named("LC34G55T")]))
}

/// La UI de la PC confirma el próximo cambio manual; devuelve el pedido.
async fn confirm_manual(ui: &mut Ui) -> ManualSwitchEvent {
    loop {
        match within(ui.next_event()).await.unwrap() {
            Some(AgentToUi::ManualSwitch(ev @ ManualSwitchEvent::Requested { id, .. })) => {
                let reply = within(ui.request(UiCommand::AnswerManualSwitch { id, done: true }))
                    .await
                    .unwrap();
                assert_eq!(reply, Reply::Done);
                return ev;
            }
            Some(_) => {}
            None => panic!("la PC cerró la IPC"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pasos_de_pantalla_con_backends_simulados() {
    // La UI de la Mac pide Solo Windows: los cambios de entrada se piden desde la Mac pero
    // los hace la PC, el 34" por DDC y el 32" a mano, confirmado en la UI de la PC.
    let dir = tempfile::tempdir().unwrap();
    let pc_on = |s: &mut State| s.on.pc = true;
    let (mac_desktop, pc_desktop) = (
        Arc::new(Escritorio::default()),
        Arc::new(Escritorio::default()),
    );
    let (cfg, store) = setup(dir.path(), Machine::Mac, DESK_DDC, &pc_on);
    let mac_local = DisplaySteps::new(Machine::Mac, cfg.desk.clone())
        .with_desktop(mac_desktop.clone())
        .with_others(Arc::new(Fake::default()));
    let mac = Arc::new(Agent::with_local_steps(cfg, store, Arc::new(mac_local)));

    let bus = bus_de_la_pc();
    let manual = Arc::new(ManualSwitcher::new());
    let (cfg, store) = setup(dir.path(), Machine::Pc, DESK_DDC, &pc_on);
    let pc_local = DisplaySteps::new(Machine::Pc, cfg.desk.clone())
        .with_switcher(Arc::new(PerMonitor {
            ddc: Arc::new(DdcSwitcher::with_retries(bus.factory(), 3, Duration::ZERO)),
            manual: manual.clone(),
        }))
        .with_desktop(pc_desktop.clone())
        .with_others(Arc::new(Fake::default()));
    let pc = Arc::new(Agent::with_backends(cfg, store, Arc::new(pc_local), manual));
    let (mut ui_mac, mut ui_pc, _link) = link(&mac, &pc).await;

    let (events, pedido) =
        tokio::join!(apply(&mut ui_mac, Mode::SoloPc), confirm_manual(&mut ui_pc));
    assert!(
        matches!(events.last(), Some(TxnEvent::Committed { .. })),
        "{events:?}"
    );
    assert!(matches!(
        pedido,
        ManualSwitchEvent::Requested { ref monitor, to: Machine::Pc, input: tandem_core::Input::DisplayPort, .. }
            if monitor.0 == "m32"
    ));
    assert_eq!(bus.writes(), [(1, VCP_INPUT_SOURCE, 0x0F, true)]);
    assert!(mac.state().owner.values().all(|m| *m == Machine::Pc));
    reaches(&pc, &mac.state()).await;
    assert_eq!(pc_desktop.calls(), ["+m34", "+m32"]);
    let mac_calls = mac_desktop.calls();
    assert!(
        mac_calls.contains(&"-m34".to_owned()) && mac_calls.contains(&"-m32".to_owned()),
        "{mac_calls:?}"
    );

    // De vuelta a Solo Mac: el 34" vuelve a HDMI con el código del perfil.
    let (events, _) = tokio::join!(
        apply(&mut ui_mac, Mode::SoloMac),
        confirm_manual(&mut ui_pc)
    );
    assert!(
        matches!(events.last(), Some(TxnEvent::Committed { .. })),
        "{events:?}"
    );
    assert_eq!(bus.writes()[1], (1, VCP_INPUT_SOURCE, 0x06, true));
    assert!(mac.state().owner.values().all(|m| *m == Machine::Mac));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rechazar_el_cambio_manual_vuelve_atras() {
    let dir = tempfile::tempdir().unwrap();
    let pc_on = |s: &mut State| s.on.pc = true;
    let (cfg, store) = setup(dir.path(), Machine::Mac, DESK_DDC, &pc_on);
    let mac_local = DisplaySteps::new(Machine::Mac, cfg.desk.clone())
        .with_desktop(Arc::new(Escritorio::default()))
        .with_others(Arc::new(Fake::default()));
    let mac = Arc::new(Agent::with_local_steps(cfg, store, Arc::new(mac_local)));
    let bus = bus_de_la_pc();
    let manual = Arc::new(ManualSwitcher::new());
    let (cfg, store) = setup(dir.path(), Machine::Pc, DESK_DDC, &pc_on);
    let pc_local = DisplaySteps::new(Machine::Pc, cfg.desk.clone())
        .with_switcher(Arc::new(PerMonitor {
            ddc: Arc::new(DdcSwitcher::with_retries(bus.factory(), 3, Duration::ZERO)),
            manual: manual.clone(),
        }))
        .with_desktop(Arc::new(Escritorio::default()))
        .with_others(Arc::new(Fake::default()));
    let pc = Arc::new(Agent::with_backends(cfg, store, Arc::new(pc_local), manual));
    let (mut ui_mac, mut ui_pc, _link) = link(&mac, &pc).await;
    let initial = mac.state();

    let decline = async {
        loop {
            if let Some(AgentToUi::ManualSwitch(ManualSwitchEvent::Requested { id, .. })) =
                within(ui_pc.next_event()).await.unwrap()
            {
                let r = within(ui_pc.request(UiCommand::AnswerManualSwitch { id, done: false }))
                    .await
                    .unwrap();
                assert_eq!(r, Reply::Done);
                return;
            }
        }
    };
    let (events, ()) = tokio::join!(apply(&mut ui_mac, Mode::SoloPc), decline);
    assert!(events.iter().any(|e| matches!(
        e,
        TxnEvent::StepFailed { error, .. } if error.contains("el usuario no confirmó el cambio manual")
    )), "{events:?}");
    assert!(
        matches!(events.last(), Some(TxnEvent::RolledBack { .. })),
        "{events:?}"
    );
    assert_eq!(mac.state(), initial);
    // El 34" fue a la PC por DDC y la vuelta atrás lo devolvió a la Mac.
    assert_eq!(
        bus.writes(),
        [
            (1, VCP_INPUT_SOURCE, 0x0F, true),
            (1, VCP_INPUT_SOURCE, 0x06, true)
        ]
    );
}
