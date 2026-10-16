//! Teclado, mouse y portapapeles entre dos `tandemd` armados con [`daemon::start`]:
//! identidades emparejadas, QUIC real en loopback y backends de entrada simulados
//! ([`FakeInputBackends`]). El cursor cruza el borde y los eventos llegan al otro
//! lado, mover el hub invierte los roles, el portapapeles se sincroniza y los permisos
//! que faltan en la Mac llegan a la UI.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_clipboard::ClipContent;
use tandem_core::{Machine, Mode, Step};
use tandem_display::switcher::ManualSwitcher;
use tandem_input::geometry::{LayoutOptions, Rect, Screen, ScreenId};
use tandem_input::session::SessionConfig;
use tandem_input::{HidUsage, InputEvent, KeyState, ModifierKey};
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_peripherals::{UsbId, WatchConfig};
use tandem_proto::{AgentToUi, InputPermission, InputStatus, Reply, TxnEvent, UiCommand};
use tandemd::daemon::{self, Backends, Daemon};
use tandemd::input::fake::{CaptureCall, FakeInputBackends};
use tandemd::input::{InputOptions, InputSetup};
use tandemd::ipc::{IpcClient, mem};
use tandemd::network::NetOptions;
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::paths::Paths;
use tandemd::{Agent, AgentConfig, StoreError};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const KEYBOARD: UsbId = UsbId::new(0x046D, 0xC33F);
const MOUSE: UsbId = UsbId::new(0x046D, 0xC539);

/// El 34" a la izquierda y el 32" a la derecha: con el 34" en la PC, la Mac queda a su
/// derecha.
const DESK: &str = r#"
[input]
server = "pc"

[input.hub]
keyboard = "046d:c33f"
mouse = "046d:c539"

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

#[derive(Default)]
struct Steps(Mutex<Vec<Step>>);

impl LocalSteps for Steps {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
        Box::pin(async move {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(step.clone());
            Ok(())
        })
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

fn fast() -> InputOptions {
    InputOptions {
        session: SessionConfig {
            heartbeat_interval: Duration::from_millis(100),
            heartbeat_timeout: Duration::from_millis(1500),
            resync_idle: Duration::from_millis(20),
            resync_every: 8,
        },
        clipboard: tandem_clipboard::ClipConfig {
            poll: Duration::from_millis(10),
            ..Default::default()
        },
        hub: WatchConfig {
            poll: Duration::from_millis(20),
            debounce: Duration::from_millis(150),
        },
        layout: LayoutOptions {
            corner_margin: 0.0,
            ..LayoutOptions::default()
        },
        report_every: Duration::from_millis(200),
        screens_every: Duration::from_millis(200),
        permissions_every: Duration::from_millis(50),
        retry: Duration::from_millis(100),
    }
}

fn screen(id: &str, w: u32, h: u32) -> Screen {
    Screen::new(ScreenId::Monitor(id.into()), Rect::new(0, 0, w, h), 1.0)
}

struct Running {
    agent: Arc<Agent>,
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

    async fn until_input(&self, f: impl Fn(&InputStatus) -> bool) {
        let mut rx = self.agent.subscribe_input();
        within(rx.wait_for(|s| f(s))).await.unwrap();
    }
}

async fn start(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: (PublicKey, u16),
    fake: FakeInputBackends,
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
    let network = format!(
        "[network]\nport = {port}\npeers = [{{ kind = \"usb4\", addr = \"{}\" }}]\n",
        loopback(peer.1)
    );
    let cfg = AgentConfig::from_toml(&format!("machine = \"{machine}\"\n{network}{DESK}")).unwrap();
    let Daemon {
        agent,
        network,
        input,
    } = daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: Arc::new(Steps::default()),
            manual: Arc::new(ManualSwitcher::new()),
            input: Some(Box::new(InputSetup {
                backends: Arc::new(fake.clone()),
                opts: fast(),
            })),
        },
        NetOptions {
            bind: Some(loopback(port)),
            mdns: false,
            ..NetOptions::default()
        },
    )
    .unwrap();
    assert!(input.is_some());
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
        stop: Some(stop),
        task,
        _dir: dir,
    }
}

fn key(usage: HidUsage, state: KeyState) -> InputEvent {
    InputEvent::Key { usage, state }
}

/// Espera a que lo inyectado cumpla `f`.
async fn until_injected(fake: &FakeInputBackends, f: impl Fn(&[InputEvent]) -> bool) {
    let mut rx = fake.injected();
    within(rx.wait_for(|e| f(e))).await.unwrap();
}

/// Lleva el cursor del servidor a `x` y lo empuja `dx` hacia el borde, hasta que la otra
/// máquina reciba la entrada (la geometría nueva puede tardar un instante en llegar).
async fn cross(server: &FakeInputBackends, client: &FakeInputBackends, x: f64, dx: f64) {
    within(async {
        loop {
            let before = client.injected().borrow().len();
            server.capture(InputEvent::MotionAbsolute { x, y: 500.0 });
            server.capture(InputEvent::MotionRelative { dx, dy: 0.0 });
            tokio::time::sleep(Duration::from_millis(50)).await;
            let entered = client.injected().borrow()[before..]
                .iter()
                .any(|e| matches!(e, InputEvent::MotionAbsolute { .. }));
            if entered {
                return;
            }
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keyboard_mouse_clipboard_and_roles_between_two_agents() {
    let (mac_id, pc_id) = (Identity::from_seed([11; 32]), Identity::from_seed([12; 32]));
    let (mac_port, pc_port) = (free_port(), free_port());
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    // El hub empieza en la PC.
    pc_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let mut mac = start(
        Machine::Mac,
        &mac_id,
        mac_port,
        (pc_id.public_key(), pc_port),
        mac_fake.clone(),
    )
    .await;
    let pc = start(
        Machine::Pc,
        &pc_id,
        pc_port,
        (mac_id.public_key(), mac_port),
        pc_fake.clone(),
    )
    .await;

    // Roles por el hub y sesión de entrada sobre el enlace.
    let pc_serving = |s: &InputStatus| s.server == Machine::Pc && s.session;
    mac.until_input(pc_serving).await;
    pc.until_input(pc_serving).await;
    assert_eq!(pc.agent.input_status().hub_here, Some(true));
    assert_eq!(mac.agent.input_status().hub_here, Some(false));
    assert!(pc_fake.capturing());

    // El 34" pasa a la PC: el cambio de modo recalcula la geometría de los dos lados.
    let mut mac_state = mac.agent.subscribe();
    within(mac_state.wait_for(|s| s.pc_on)).await.unwrap();
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
    within(async {
        loop {
            match mac.ui.next_event().await.unwrap().unwrap() {
                AgentToUi::Txn(TxnEvent::Committed { txn: t, .. }) if t == txn => break,
                AgentToUi::Txn(
                    e @ (TxnEvent::RolledBack { .. } | TxnEvent::Inconsistent { .. }),
                ) => panic!("el cambio falló: {e:?}"),
                _ => {}
            }
        }
    })
    .await;

    // El cursor sale por el borde derecho de la PC y entra a la Mac.
    cross(&pc_fake, &mac_fake, 3439.0, 5.0).await;
    let calls = pc_fake.capture_calls().borrow().clone();
    assert!(calls.contains(&CaptureCall::Grab(true)), "{calls:?}");
    let entered = mac_fake
        .injected()
        .borrow()
        .iter()
        .find_map(|e| match e {
            InputEvent::MotionAbsolute { x, .. } => Some(*x),
            _ => None,
        })
        .unwrap();
    assert_eq!(entered, 0.0, "entra por el borde izquierdo de la Mac");

    // Las teclas llegan, con Ctrl de la PC como Cmd en la Mac.
    let a = HidUsage::key(0x04);
    pc_fake.capture(key(ModifierKey::ControlLeft.usage(), KeyState::Pressed));
    pc_fake.capture(key(a, KeyState::Pressed));
    pc_fake.capture(key(a, KeyState::Released));
    pc_fake.capture(key(ModifierKey::ControlLeft.usage(), KeyState::Released));
    until_injected(&mac_fake, |e| {
        e.ends_with(&[
            key(ModifierKey::MetaLeft.usage(), KeyState::Pressed),
            key(a, KeyState::Pressed),
            key(a, KeyState::Released),
            key(ModifierKey::MetaLeft.usage(), KeyState::Released),
        ])
    })
    .await;

    // El portapapeles va en los dos sentidos.
    let text = |s: &str| ClipContent {
        text: Some(s.into()),
        html: None,
        image: None,
    };
    mac_fake.clipboard().copy(Some(text("desde la Mac")));
    let mut pc_clip = pc_fake.clipboard().subscribe();
    within(pc_clip.wait_for(|s| s.content == Some(text("desde la Mac"))))
        .await
        .unwrap();
    // Una copia justo después de que el driver escribe se toma como su propia escritura
    // (el hueco conocido de `tandem-clipboard`, de microsegundos): el usuario no copia
    // tan rápido, la prueba sí.
    tokio::time::sleep(Duration::from_millis(100)).await;
    pc_fake.clipboard().copy(Some(text("desde la PC")));
    let mut mac_clip = mac_fake.clipboard().subscribe();
    within(mac_clip.wait_for(|s| s.content == Some(text("desde la PC"))))
        .await
        .unwrap();

    // El usuario mueve el hub a la Mac: los roles se invierten en los dos agentes.
    pc_fake.devices().set(vec![]);
    mac_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let mac_serving = |s: &InputStatus| s.server == Machine::Mac && s.session;
    mac.until_input(mac_serving).await;
    pc.until_input(mac_serving).await;
    assert!(mac_fake.capturing());
    // Al dejar de ser servidor la PC devolvió el cursor a su pantalla.
    let calls = pc_fake.capture_calls().borrow().clone();
    assert_eq!(
        calls
            .iter()
            .rev()
            .find(|c| matches!(c, CaptureCall::Grab(_))),
        Some(&CaptureCall::Grab(false)),
        "{calls:?}"
    );

    // Ahora la Mac captura y el cursor cruza por su borde izquierdo a la PC.
    cross(&mac_fake, &pc_fake, 0.0, -5.0).await;
    let b = HidUsage::key(0x05);
    mac_fake.capture(key(b, KeyState::Pressed));
    until_injected(&pc_fake, |e| e.last() == Some(&key(b, KeyState::Pressed))).await;

    // A la Mac le falta un permiso: la UI lo recibe como aviso.
    mac_fake.set_missing(vec![InputPermission::Accessibility]);
    within(async {
        loop {
            if let AgentToUi::Input(s) = mac.ui.next_event().await.unwrap().unwrap()
                && s.missing == [InputPermission::Accessibility]
            {
                break;
            }
        }
    })
    .await;
    assert_eq!(
        mac.ui.request(UiCommand::GetInput).await.unwrap(),
        Reply::Input(mac.agent.input_status())
    );

    pc.stop().await;
    // Sin el otro agente no hay sesión, y la Mac, con el hub, sigue de servidor.
    mac.until_input(|s| !s.session && s.server == Machine::Mac)
        .await;
    mac.stop().await;
}

/// Si la captura no puede arrancar (en macOS, sin Accesibilidad), la UI lo ve y el
/// agente reintenta solo.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capture_that_fails_is_reported_and_retried() {
    let (mac_id, pc_id) = (Identity::from_seed([13; 32]), Identity::from_seed([14; 32]));
    let fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    fake.devices().set(vec![KEYBOARD]);
    fake.fail_capture(Some("falta el permiso de Accesibilidad".into()));
    let mac = start(
        Machine::Mac,
        &mac_id,
        free_port(),
        (pc_id.public_key(), free_port()),
        fake.clone(),
    )
    .await;
    // Sin el otro agente y con la PC dada por apagada, la Mac con el hub es el servidor.
    let problem = Some("falta el permiso de Accesibilidad".to_owned());
    mac.until_input(|s| s.server == Machine::Mac && s.problem == problem)
        .await;
    assert!(!mac.agent.input_status().session);
    assert!(!fake.capturing());
    // El usuario concede el permiso: el siguiente intento arranca la captura.
    fake.fail_capture(None);
    within(async {
        while !fake.capturing() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    mac.until_input(|s| s.problem.is_none()).await;
    mac.stop().await;
}
