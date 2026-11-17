//! Teclado, mouse y portapapeles entre dos `tandemd` armados con [`daemon::start`]:
//! identidades emparejadas, QUIC real en loopback y backends de entrada simulados
//! ([`FakeInputBackends`]). El cursor cruza el borde y los eventos llegan al otro
//! lado, mover el hub invierte los roles, el portapapeles se sincroniza y los permisos
//! que faltan en la Mac llegan a la UI.
//!
//! La geometría es la del acomodo (DESIGN §7): el de omisión (A a la izquierda, con
//! las pantallas que lee cada una), uno editado que llega por `ControlMsg::Desk` y uno
//! escrito a mano en `tandem.toml`.
//!
//! Salvo que la prueba diga otra cosa, el lado A es una Mac y el B una PC (los nombres de
//! las variables lo reflejan); el código no lo supone: lo dice el `Os` de cada agente.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_clipboard::ClipContent;
use tandem_core::{Machine, Os, Side, Step};
use tandem_input::geometry::{LayoutOptions, Rect, Screen};
use tandem_input::session::SessionConfig;
use tandem_input::{HidUsage, InputEvent, KeyState, ModifierKey};
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_peripherals::{UsbId, WatchConfig};
use tandem_proto::{AgentToUi, InputPermission, InputStatus, Reply, UiCommand};
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

/// Las dos identidades ordenadas por clave: la primera es la del lado A (DECISIONS A1), así
/// cada prueba sabe qué lado le toca a cada agente.
fn sides(x: Identity, y: Identity) -> (Identity, Identity) {
    if x.public_key().as_bytes() < y.public_key().as_bytes() {
        (x, y)
    } else {
        (y, x)
    }
}

const KEYBOARD: UsbId = UsbId::new(0x046D, 0xC33F);
const MOUSE: UsbId = UsbId::new(0x046D, 0xC539);

/// El hub del teclado y el mouse, y el acomodo por omisión: la Mac a la izquierda de la PC.
const DESK: &str = r#"
[input]
server = "b"

[input.hub]
keyboard = "046d:c33f"
mouse = "046d:c539"
"#;

/// Sin hub ni teclado dicho: lo que genera el primer arranque (T-087).
const DESK_BARE: &str = r#"
[input]
"#;

/// Marca para `start_with`: usar [`DESK_BARE`].
const BARE: &str = "<bare>";

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
        layout: LayoutOptions { corner_margin: 0.0 },
        report_every: Duration::from_millis(200),
        screens_every: Duration::from_millis(200),
        permissions_every: Duration::from_millis(50),
        retry: Duration::from_millis(100),
    }
}

fn screen(id: &str, w: u32, h: u32) -> Screen {
    Screen::new(id.into(), Rect::new(0, 0, w, h), 1.0)
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
    start_with(machine, identity, port, peer, fake, "").await
}

/// Como [`start`], con líneas de más en la sección `[input]`.
async fn start_with(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: (PublicKey, u16),
    fake: FakeInputBackends,
    input_extra: &str,
) -> Running {
    start_full(machine, identity, port, peer, fake, input_extra, "").await
}

/// El sistema de cada lado en estas pruebas, salvo que digan otro.
fn os_of(machine: Machine) -> Os {
    match machine {
        Machine::A => Os::MacOs,
        Machine::B => Os::Windows,
    }
}

/// Como [`start_with`], con secciones de más al final de `tandem.toml` (un acomodo
/// escrito a mano).
async fn start_full(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: (PublicKey, u16),
    fake: FakeInputBackends,
    input_extra: &str,
    tail: &str,
) -> Running {
    start_os(
        machine,
        os_of(machine),
        identity,
        port,
        peer,
        fake,
        input_extra,
        tail,
    )
    .await
}

/// Como [`start_full`], con el sistema que se le diga.
#[allow(clippy::too_many_arguments)]
async fn start_os(
    machine: Machine,
    os: Os,
    identity: &Identity,
    port: u16,
    peer: (PublicKey, u16),
    fake: FakeInputBackends,
    input_extra: &str,
    tail: &str,
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
    let base = if input_extra == BARE { DESK_BARE } else { DESK };
    let extra = if input_extra == BARE { "" } else { input_extra };
    let desk = base.replacen("[input]\n", &format!("[input]\n{extra}"), 1);
    let text = format!("machine = \"{machine}\"\n{network}{desk}\n{tail}");
    std::fs::write(&paths.config, &text).unwrap();
    let mut cfg = AgentConfig::load(&paths.config).unwrap();
    cfg.os = os;
    let Daemon {
        agent,
        network,
        input,
    } = daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: Arc::new(Steps::default()),
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
/// Devuelve dónde entró.
async fn cross(
    server: &FakeInputBackends,
    client: &FakeInputBackends,
    x: f64,
    dx: f64,
) -> (f64, f64) {
    within(async {
        loop {
            let before = client.injected().borrow().len();
            server.capture(InputEvent::MotionAbsolute { x, y: 500.0 });
            server.capture(InputEvent::MotionRelative { dx, dy: 0.0 });
            tokio::time::sleep(Duration::from_millis(50)).await;
            let entered = client.injected().borrow()[before..]
                .iter()
                .find_map(|e| match e {
                    InputEvent::MotionAbsolute { x, y } => Some((*x, *y)),
                    _ => None,
                });
            if let Some(at) = entered {
                return at;
            }
        }
    })
    .await
}

/// Espera a que cada agente vea a la otra máquina prendida: sin eso sus pantallas no
/// cuentan en la geometría.
async fn both_on(a: &Running, b: &Running) {
    for r in [a, b] {
        let other = r.agent.machine().other();
        let mut rx = r.agent.subscribe();
        within(rx.wait_for(|s| s.is_on(other))).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keyboard_mouse_clipboard_and_roles_between_two_agents() {
    let (mac_id, pc_id) = sides(Identity::from_seed([11; 32]), Identity::from_seed([12; 32]));
    let (mac_port, pc_port) = (free_port(), free_port());
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    // El hub empieza en la PC.
    pc_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let mut mac = start(
        Machine::A,
        &mac_id,
        mac_port,
        (pc_id.public_key(), pc_port),
        mac_fake.clone(),
    )
    .await;
    let pc = start(
        Machine::B,
        &pc_id,
        pc_port,
        (mac_id.public_key(), mac_port),
        pc_fake.clone(),
    )
    .await;

    // Roles por el hub y sesión de entrada sobre el enlace.
    let pc_serving = |s: &InputStatus| s.server == Machine::B && s.session;
    mac.until_input(pc_serving).await;
    pc.until_input(pc_serving).await;
    assert_eq!(pc.agent.input_status().hub_here, Some(true));
    assert_eq!(mac.agent.input_status().hub_here, Some(false));
    assert!(pc_fake.capturing());

    // Acomodo por omisión: la Mac a la izquierda de la PC, con las pantallas que lee cada una.
    both_on(&mac, &pc).await;

    // El cursor sale por el borde izquierdo de la PC y entra a la Mac por su borde derecho.
    let (x, _) = cross(&pc_fake, &mac_fake, 0.0, -5.0).await;
    let calls = pc_fake.capture_calls().borrow().clone();
    assert!(calls.contains(&CaptureCall::Grab(true)), "{calls:?}");
    assert_eq!(x, 1919.0, "entra por el borde derecho de la Mac");

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
    let mac_serving = |s: &InputStatus| s.server == Machine::A && s.session;
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

    // Ahora la Mac captura y el cursor cruza por su borde derecho a la PC.
    let (x, _) = cross(&mac_fake, &pc_fake, 1919.0, 5.0).await;
    assert_eq!(x, 0.0, "entra por el borde izquierdo de la PC");
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
    mac.until_input(|s| !s.session && s.server == Machine::A)
        .await;
    mac.stop().await;
}

/// Un acomodo editado en la PC llega a la Mac por `ControlMsg::Desk` y cambia por dónde
/// cruza el cursor que ella tiene (DESIGN §7).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_arrangement_edited_on_one_machine_reaches_the_other_by_desk() {
    let (mac_id, pc_id) = sides(Identity::from_seed([31; 32]), Identity::from_seed([32; 32]));
    let (mac_port, pc_port) = (free_port(), free_port());
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    // El hub en la Mac: ella captura y decide por dónde cruza.
    mac_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let mac = start(
        Machine::A,
        &mac_id,
        mac_port,
        (pc_id.public_key(), pc_port),
        mac_fake.clone(),
    )
    .await;
    let pc = start(
        Machine::B,
        &pc_id,
        pc_port,
        (mac_id.public_key(), mac_port),
        pc_fake.clone(),
    )
    .await;
    let mac_serving = |s: &InputStatus| s.server == Machine::A && s.session;
    mac.until_input(mac_serving).await;
    both_on(&mac, &pc).await;

    // Por omisión la Mac está a la izquierda: cruza por su borde derecho.
    let (x, _) = cross(&mac_fake, &pc_fake, 1919.0, 5.0).await;
    assert_eq!(x, 0.0);
    // El cursor vuelve a la Mac por donde entró (el borde izquierdo de la PC): la Mac
    // suelta la captura.
    within(async {
        loop {
            mac_fake.capture(InputEvent::MotionRelative { dx: -5.0, dy: 0.0 });
            tokio::time::sleep(Duration::from_millis(50)).await;
            let calls = mac_fake.capture_calls().borrow().clone();
            let last = calls
                .iter()
                .rev()
                .find(|c| matches!(c, CaptureCall::Grab(_)));
            if last == Some(&CaptureCall::Grab(false)) {
                return;
            }
        }
    })
    .await;

    // Antes de editar, las dos ya anotaron a las dos máquinas en el puesto (una versión
    // propia del conocerse); si no, esa versión podría llegar después y pisar la edición.
    for r in [&mac, &pc] {
        let mut d = r.agent.subscribe_desk();
        within(d.wait_for(|d| Machine::ALL.iter().all(|m| d.machines.get(*m).is_some())))
            .await
            .unwrap();
    }
    within(async {
        while mac.agent.desk_sync().rev != pc.agent.desk_sync().rev
            || mac.agent.desk() != pc.agent.desk()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    // En la PC el acomodo pasa a «la Mac a la derecha».
    let mut desk = pc.agent.desk();
    desk.arrangement.a_side = Side::Right;
    pc.agent.set_desk(desk.clone()).unwrap();
    let mut mac_desk = mac.agent.subscribe_desk();
    within(mac_desk.wait_for(|d| *d == desk)).await.unwrap();
    assert_eq!(mac.agent.desk_sync().rev, pc.agent.desk_sync().rev);

    // Ahora la Mac cruza por su borde izquierdo y entra a la PC por el derecho.
    let (x, _) = cross(&mac_fake, &pc_fake, 0.0, -5.0).await;
    assert_eq!(x, 3439.0, "entra por el borde derecho de la PC");
    pc.stop().await;
    mac.stop().await;
}

/// Un `[[screens]]` escrito a mano en `tandem.toml` cambia la geometría: la PC declara su
/// pantalla de 2000×720 aunque lea 3440×1440, y la Mac a su derecha. El cruce sale del
/// acomodo declarado, y el puesto editado llega también a la Mac.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hand_written_screen_in_tandem_toml_changes_the_geometry() {
    let (mac_id, pc_id) = sides(Identity::from_seed([33; 32]), Identity::from_seed([34; 32]));
    let (mac_port, pc_port) = (free_port(), free_port());
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    pc_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let mac = start(
        Machine::A,
        &mac_id,
        mac_port,
        (pc_id.public_key(), pc_port),
        mac_fake.clone(),
    )
    .await;
    let declared = "[[screens]]\nmachine = \"b\"\nid = \"34\"\nx = 0\ny = 0\nwidth = 2000\nheight = 720\n\n[arrangement]\na_side = \"right\"\n";
    let pc = start_full(
        Machine::B,
        &pc_id,
        pc_port,
        (mac_id.public_key(), mac_port),
        pc_fake.clone(),
        "",
        declared,
    )
    .await;
    let pc_serving = |s: &InputStatus| s.server == Machine::B && s.session;
    pc.until_input(pc_serving).await;
    both_on(&mac, &pc).await;

    // A 500 de 720 por el borde derecho declarado (x = 1999) entra a la Mac a 750 de 1080.
    let (x, y) = cross(&pc_fake, &mac_fake, 1999.0, 5.0).await;
    assert_eq!(x, 0.0, "entra por el borde izquierdo de la Mac");
    assert!((y - 750.0).abs() < 1e-6, "{y}");

    // Lo escrito a mano es un cambio de la PC: la Mac lo adopta.
    let want = pc.agent.desk();
    assert_eq!(want.screens.len(), 1);
    let mut mac_desk = mac.agent.subscribe_desk();
    within(mac_desk.wait_for(|d| d.screens == want.screens && d.arrangement == want.arrangement))
        .await
        .unwrap();
    pc.stop().await;
    mac.stop().await;
}

/// Si la captura no puede arrancar (en macOS, sin Accesibilidad), la UI lo ve y el
/// agente reintenta solo.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capture_that_fails_is_reported_and_retried() {
    let (mac_id, pc_id) = sides(Identity::from_seed([13; 32]), Identity::from_seed([14; 32]));
    let fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    fake.devices().set(vec![KEYBOARD]);
    fake.fail_capture(Some("falta el permiso de Accesibilidad".into()));
    let mac = start(
        Machine::A,
        &mac_id,
        free_port(),
        (pc_id.public_key(), free_port()),
        fake.clone(),
    )
    .await;
    // Sin el otro agente y con la PC dada por apagada, la Mac con el hub es el servidor.
    let problem = Some("falta el permiso de Accesibilidad".to_owned());
    mac.until_input(|s| s.server == Machine::A && s.problem == problem)
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

/// El atajo de bloqueo se escribe con las teclas físicas de quien lo usa. La captura
/// cambia Ctrl por Cmd antes de que la sesión vea las teclas: aun así, Ctrl+L en la PC
/// tiene que bloquear el cursor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_lock_shortcut_uses_the_physical_keys_of_the_server() {
    let (mac_id, pc_id) = sides(Identity::from_seed([15; 32]), Identity::from_seed([16; 32]));
    let (mac_port, pc_port) = (free_port(), free_port());
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    pc_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let shortcut = "lock_shortcut = [\"ControlLeft\", \"KeyL\"]\n";
    let mac = start_with(
        Machine::A,
        &mac_id,
        mac_port,
        (pc_id.public_key(), pc_port),
        mac_fake.clone(),
        shortcut,
    )
    .await;
    let pc = start_with(
        Machine::B,
        &pc_id,
        pc_port,
        (mac_id.public_key(), mac_port),
        pc_fake.clone(),
        shortcut,
    )
    .await;
    pc.until_input(|s| s.session && s.server == Machine::B)
        .await;
    assert!(!pc.agent.input_status().locked);

    let l = HidUsage::key(0x0F);
    let ctrl = ModifierKey::ControlLeft.usage();
    assert!(pc_fake.capture(key(ctrl, KeyState::Pressed)));
    assert!(pc_fake.capture(key(l, KeyState::Pressed)));
    pc.until_input(|s| s.locked).await;
    assert!(pc_fake.capture(key(l, KeyState::Released)));
    assert!(pc_fake.capture(key(ctrl, KeyState::Released)));

    // Lo mismo otra vez lo desbloquea.
    assert!(pc_fake.capture(key(ctrl, KeyState::Pressed)));
    assert!(pc_fake.capture(key(l, KeyState::Pressed)));
    pc.until_input(|s| !s.locked).await;
    pc.stop().await;
    mac.stop().await;
}

/// T-087: el estado de entrada se rehace cuando el usuario declara dónde está el teclado,
/// también si declara la máquina que ya era el servidor (no cambian `server` ni el hub, así
/// que solo la declaración puede despertar la tarea de estado).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declaring_the_keyboard_on_the_server_machine_refreshes_defined() {
    let (mac_id, pc_id) = sides(Identity::from_seed([21; 32]), Identity::from_seed([22; 32]));
    let (mac_port, pc_port) = (free_port(), free_port());
    let fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    let mac = start_with(
        Machine::A,
        &mac_id,
        mac_port,
        (pc_id.public_key(), pc_port),
        fake,
        BARE,
    )
    .await;
    // Sin hub ni declaración: sin definir, y el motor arranca como si fuera la Mac.
    mac.until_input(|s| s.server == Machine::A && !s.defined)
        .await;
    // Se declara la Mac: ya era el servidor, así que nada más cambia.
    let reply = mac
        .agent
        .handle(tandem_proto::UiCommand::SetKeyboard(Machine::A));
    assert_eq!(reply, tandem_proto::Reply::Done);
    mac.until_input(|s| s.defined && s.server == Machine::A)
        .await;
    // Y se mantiene: la tarea de estado también ve la declaración.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(mac.agent.input_status().defined);
    mac.stop().await;
}

/// T-090: con permisos que faltan, el agente los pide al sistema una vez al arrancar (para
/// que macOS registre el binario real), no los vuelve a pedir solo por más que sigan
/// faltando, y los vuelve a pedir cuando la UI lo ordena. Las capacidades llevan el proceso.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_permissions_are_requested_once_and_on_the_users_order() {
    let (mac_id, pc_id) = sides(Identity::from_seed([15; 32]), Identity::from_seed([16; 32]));
    let fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    fake.set_missing(vec![
        InputPermission::Accessibility,
        InputPermission::InputMonitoring,
    ]);
    let mut mac = start(
        Machine::A,
        &mac_id,
        free_port(),
        (pc_id.public_key(), free_port()),
        fake.clone(),
    )
    .await;
    mac.until_input(|s| s.missing.len() == 2).await;
    // Varias lecturas de permisos después (cada 50 ms), sigue siendo un solo pedido.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(fake.permission_requests(), 1);

    // Orden explícita de la UI: se pide otra vez, y esta vez el usuario concede.
    fake.grant_on_request(true);
    assert_eq!(
        mac.ui.request(UiCommand::RequestPermissions).await.unwrap(),
        Reply::Done
    );
    mac.until_input(|s| s.missing.is_empty()).await;
    assert_eq!(fake.permission_requests(), 2);

    // El proceso que corre llega a las capacidades para que la UI diga a quién conceder.
    let process = mac.agent.capabilities().here.input.process;
    assert_eq!(process.pid, std::process::id());
    assert!(!process.exe.is_empty());
    assert!(process.started > 0);

    mac.stop().await;
}

/// Sin permisos que pedir, el arranque no abre ningún diálogo.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_is_requested_when_nothing_is_missing() {
    let (mac_id, pc_id) = sides(Identity::from_seed([17; 32]), Identity::from_seed([18; 32]));
    let fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let mac = start(
        Machine::A,
        &mac_id,
        free_port(),
        (pc_id.public_key(), free_port()),
        fake.clone(),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(fake.permission_requests(), 0);
    mac.stop().await;
}

/// Dos máquinas con Windows (DECISIONS D2): el teclado y el mouse cruzan igual y Ctrl sigue
/// siendo Ctrl, porque entre dos sistemas iguales no se reasigna nada.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_windows_machines_share_the_keyboard_without_remapping() {
    let (a_id, b_id) = sides(Identity::from_seed([41; 32]), Identity::from_seed([42; 32]));
    let (a_port, b_port) = (free_port(), free_port());
    let a_fake = FakeInputBackends::new(vec![screen("izq", 1920, 1080)]);
    let b_fake = FakeInputBackends::new(vec![screen("der", 2560, 1440)]);
    b_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let a = start_os(
        Machine::A,
        Os::Windows,
        &a_id,
        a_port,
        (b_id.public_key(), b_port),
        a_fake.clone(),
        "",
        "",
    )
    .await;
    let b = start_os(
        Machine::B,
        Os::Windows,
        &b_id,
        b_port,
        (a_id.public_key(), a_port),
        b_fake.clone(),
        "",
        "",
    )
    .await;
    let b_serving = |s: &InputStatus| s.server == Machine::B && s.session;
    a.until_input(b_serving).await;
    b.until_input(b_serving).await;
    both_on(&a, &b).await;
    // Cada una sabe que la otra es Windows.
    let mut caps = b.agent.subscribe_capabilities();
    within(caps.wait_for(|c| c.peer.as_ref().is_some_and(|p| p.info.os == Os::Windows)))
        .await
        .unwrap();

    let (x, _) = cross(&b_fake, &a_fake, 0.0, -5.0).await;
    assert_eq!(x, 1919.0, "entra por el borde derecho de A");
    let c = HidUsage::key(0x06);
    b_fake.capture(key(ModifierKey::ControlLeft.usage(), KeyState::Pressed));
    b_fake.capture(key(c, KeyState::Pressed));
    b_fake.capture(key(c, KeyState::Released));
    b_fake.capture(key(ModifierKey::ControlLeft.usage(), KeyState::Released));
    until_injected(&a_fake, |e| {
        e.ends_with(&[
            key(ModifierKey::ControlLeft.usage(), KeyState::Pressed),
            key(c, KeyState::Pressed),
            key(c, KeyState::Released),
            key(ModifierKey::ControlLeft.usage(), KeyState::Released),
        ])
    })
    .await;
    // Y las dos quedaron anotadas en el puesto, con su sistema.
    let mut desk = a.agent.subscribe_desk();
    within(desk.wait_for(|d| {
        Machine::ALL
            .iter()
            .all(|m| d.machines.get(*m).is_some_and(|i| i.os == Os::Windows))
    }))
    .await
    .unwrap();
    b.stop().await;
    a.stop().await;
}

/// Los módulos de entrada se apagan de verdad (T-098): sin portapapeles no se comparte lo
/// copiado y la sesión de teclado y mouse sigue; sin teclado y mouse no hay captura ni
/// sesión. Al encenderlos vuelve todo.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn switching_off_the_clipboard_and_the_input_stops_them() {
    use tandem_proto::ModuleId;

    let (mac_id, pc_id) = sides(Identity::from_seed([51; 32]), Identity::from_seed([52; 32]));
    let (mac_port, pc_port) = (free_port(), free_port());
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    pc_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let mac = start(
        Machine::A,
        &mac_id,
        mac_port,
        (pc_id.public_key(), pc_port),
        mac_fake.clone(),
    )
    .await;
    let pc = start(
        Machine::B,
        &pc_id,
        pc_port,
        (mac_id.public_key(), mac_port),
        pc_fake.clone(),
    )
    .await;
    let pc_serving = |s: &InputStatus| s.server == Machine::B && s.session;
    mac.until_input(pc_serving).await;
    pc.until_input(pc_serving).await;

    let text = |s: &str| ClipContent {
        text: Some(s.into()),
        html: None,
        image: None,
    };
    // Copia en la Mac hasta que la PC tenga lo copiado; con `arrives` falso, copia una vez y
    // dice si llegó.
    let copied = |s: &str, arrives: bool| {
        let (mac_fake, pc_fake, want) = (mac_fake.clone(), pc_fake.clone(), text(s));
        async move {
            let mut pc_clip = pc_fake.clipboard().subscribe();
            if !arrives {
                mac_fake.clipboard().copy(Some(want.clone()));
                tokio::time::sleep(Duration::from_millis(400)).await;
                return pc_clip.borrow().content.as_ref() == Some(&want);
            }
            within(async {
                loop {
                    mac_fake.clipboard().copy(Some(want.clone()));
                    let got = tokio::time::timeout(
                        Duration::from_millis(300),
                        pc_clip.wait_for(|c| c.content.as_ref() == Some(&want)),
                    )
                    .await;
                    if matches!(got, Ok(Ok(_))) {
                        return true;
                    }
                }
            })
            .await
        }
    };
    assert!(copied("antes", true).await);

    pc.agent.set_module(ModuleId::Clipboard, false).unwrap();
    assert!(!copied("apagado", false).await, "sin portapapeles no llega");
    // La entrada no se cortó.
    assert!(pc.agent.input_status().session);

    pc.agent.set_module(ModuleId::Clipboard, true).unwrap();
    assert!(copied("encendido", true).await);

    // Sin teclado y mouse en la PC no hay captura ni sesión, en las dos.
    let still = InputEvent::MotionRelative { dx: 0.0, dy: 0.0 };
    pc.agent.set_module(ModuleId::Input, false).unwrap();
    pc.until_input(|s| !s.session).await;
    mac.until_input(|s| !s.session).await;
    // La captura se suelta al terminar sus tareas: nadie escucha lo que el usuario hace.
    within(async {
        while pc_fake.capture(still) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;

    pc.agent.set_module(ModuleId::Input, true).unwrap();
    mac.until_input(pc_serving).await;
    pc.until_input(pc_serving).await;
    assert!(pc_fake.capture(still), "la captura volvió");

    pc.stop().await;
    mac.stop().await;
}
