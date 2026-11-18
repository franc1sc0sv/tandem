//! Teclado, mouse y portapapeles entre dos `tandemd` armados con [`daemon::start`]:
//! identidades emparejadas, QUIC real en loopback y backends de entrada simulados
//! ([`FakeInputBackends`]). El cursor cruza el borde y los eventos llegan al otro
//! lado, usar el mouse de la otra máquina la pone a mandar (D7: sin dueño), el
//! portapapeles se sincroniza y los permisos que faltan en la Mac llegan a la UI.
//!
//! La geometría es la del acomodo (DESIGN §7): el de omisión (A a la izquierda, con
//! las pantallas que lee cada una), uno editado que llega por `ControlMsg::Desk` y uno
//! escrito a mano en `tandem.toml`.
//!
//! Salvo que la prueba diga otra cosa, el lado A es una Mac y el B una PC (los nombres de
//! las variables lo reflejan); el código no lo supone: lo dice el `Os` de cada agente.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_clipboard::ClipContent;
use tandem_core::{DeskPatch, InputPrefs, Machine, Os, Shortcuts, Side, Step};
use tandem_input::geometry::{LayoutOptions, Rect, Screen};
use tandem_input::session::{FollowConfig, SessionConfig};
use tandem_input::{HidUsage, InputEvent, KeyState, ModifierKey};
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_peripherals::{UsbId, WatchConfig};
use tandem_proto::{AgentToUi, InputPermission, InputStatus, Reply, UiCommand};
use tandemd::daemon::{self, Backends, Daemon};
use tandemd::drag::FileDragBackend;
use tandemd::input::fake::{CaptureCall, FakeInputBackends};
use tandemd::input::{InputOptions, InputSetup};
use tandemd::ipc::{IpcClient, mem};
use tandemd::network::NetOptions;
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::paths::Paths;
use tandemd::test_ports::{PortTaken, free_port, port_taken};
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

/// Dice dónde se esperó, para saber qué paso se colgó.
#[track_caller]
fn within<F: Future>(f: F) -> impl Future<Output = F::Output> {
    let at = std::panic::Location::caller();
    async move {
        match tokio::time::timeout(TIMEOUT, f).await {
            Ok(v) => v,
            Err(_) => panic!("la prueba superó el tiempo máximo esperando en {at}"),
        }
    }
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
            latency_every: Duration::from_millis(500),
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
        follow: FollowConfig::default(),
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

    #[track_caller]
    fn until_input(&self, f: impl Fn(&InputStatus) -> bool) -> impl Future<Output = ()> {
        let mut rx = self.agent.subscribe_input();
        within(async move {
            rx.wait_for(|s| f(s)).await.map(|_| ()).unwrap();
        })
    }
}

async fn start(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: (PublicKey, u16),
    fake: FakeInputBackends,
) -> Result<Running, PortTaken> {
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
) -> Result<Running, PortTaken> {
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
) -> Result<Running, PortTaken> {
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
) -> Result<Running, PortTaken> {
    start_os_drag(
        machine,
        os,
        identity,
        port,
        peer,
        fake,
        input_extra,
        tail,
        None,
    )
    .await
}

/// Como [`start_os`], con el detector de arrastre de archivos (T-121) que se le dé.
#[allow(clippy::too_many_arguments)]
async fn start_os_drag(
    machine: Machine,
    os: Os,
    identity: &Identity,
    port: u16,
    peer: (PublicKey, u16),
    fake: FakeInputBackends,
    input_extra: &str,
    tail: &str,
    file_drag: Option<Arc<dyn FileDragBackend>>,
) -> Result<Running, PortTaken> {
    // `TANDEM_TEST_LOG=tandemd::input=debug` muestra el registro de los agentes: para
    // entender una espera que vence.
    if let Ok(filter) = std::env::var("TANDEM_TEST_LOG") {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .try_init();
    }
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
    let started = daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: Arc::new(Steps::default()),
            input: Some(Box::new(InputSetup {
                backends: Arc::new(fake.clone()),
                opts: fast(),
                file_drag,
            })),
            env: None,
        },
        NetOptions {
            bind: Some(loopback(port)),
            mdns: false,
            // Sin reserva: un puerto tomado falla enseguida y `start_pair!` elige otro.
            fallback_ports: Vec::new(),
            ..NetOptions::default()
        },
    );
    let Daemon {
        agent,
        network,
        input,
    } = port_taken(started).unwrap()?;
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
    Ok(Running {
        agent,
        ui,
        stop: Some(stop),
        task,
        _dir: dir,
    })
}

fn key(usage: HidUsage, state: KeyState) -> InputEvent {
    InputEvent::Key { usage, state }
}

/// Espera a que lo inyectado cumpla `f`.
#[track_caller]
fn until_injected(
    fake: &FakeInputBackends,
    f: impl Fn(&[InputEvent]) -> bool,
) -> impl Future<Output = ()> {
    let mut rx = fake.injected();
    within(async move {
        rx.wait_for(|e| f(e)).await.map(|_| ()).unwrap();
    })
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

/// El usuario usa el mouse de `who` hasta que las dos máquinas dicen que manda ella y la
/// sesión está arriba (D7: manda la que recibe entrada física, en cuanto la otra lleve
/// 300 ms quieta).
async fn take_over(who: &Running, fake: &FakeInputBackends, other: &Running) {
    let me = who.agent.machine();
    within(async {
        loop {
            fake.capture(InputEvent::MotionRelative { dx: 0.0, dy: 1.0 });
            let serving = |r: &Running| {
                let s = r.agent.input_status();
                s.server == me && s.session
            };
            if serving(who) && serving(other) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
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
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    // El hub empieza en la PC.
    pc_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let (mut mac, pc) = tandemd::start_pair!(
        |mac_port, pc_port| start(
            Machine::A,
            &mac_id,
            mac_port,
            (pc_id.public_key(), pc_port),
            mac_fake.clone(),
        ),
        start(
            Machine::B,
            &pc_id,
            pc_port,
            (mac_id.public_key(), mac_port),
            pc_fake.clone(),
        )
    );

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

    // Las dos capturan siempre (D7).
    assert!(mac_fake.capturing());
    // El usuario toma el mouse de la Mac: en cuanto la PC lleva 300 ms quieta, manda la
    // Mac en los dos agentes. El hub no decide nada.
    take_over(&mac, &mac_fake, &pc).await;
    assert_eq!(pc.agent.input_status().hub_here, Some(true));
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

    // La PC vuelve a usarse mientras la Mac escribe: no se la quita hasta que la Mac
    // lleva 300 ms quieta.
    let started = std::time::Instant::now();
    for _ in 0..10 {
        mac_fake.capture(key(b, KeyState::Released));
        pc_fake.capture(InputEvent::MotionRelative { dx: 0.0, dy: 1.0 });
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    if started.elapsed() < Duration::from_millis(250) {
        assert_eq!(
            pc.agent.input_status().server,
            Machine::A,
            "la Mac seguía en uso"
        );
    }
    take_over(&pc, &pc_fake, &mac).await;

    pc.stop().await;
    // Sin el otro agente no hay sesión; la Mac sigue con lo último que se decidió hasta
    // que se use.
    mac.until_input(|s| !s.session && s.server == Machine::B)
        .await;
    // Sin enlace, manda la que se usa. La sesión cae antes que el enlace (que espera su
    // propio plazo de salud): mientras el enlace siga arriba la Mac pide y nadie contesta,
    // así que el usuario sigue moviendo el mouse hasta que el enlace cae.
    within(async {
        while mac.agent.input_status().server != Machine::A {
            mac_fake.capture(InputEvent::MotionRelative { dx: 0.0, dy: 1.0 });
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    mac.stop().await;
}

/// Un acomodo editado en la PC llega a la Mac por `ControlMsg::Desk` y cambia por dónde
/// cruza el cursor que ella tiene (DESIGN §7).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_arrangement_edited_on_one_machine_reaches_the_other_by_desk() {
    let (mac_id, pc_id) = sides(Identity::from_seed([31; 32]), Identity::from_seed([32; 32]));
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    let (mac, pc) = tandemd::start_pair!(
        |mac_port, pc_port| start(
            Machine::A,
            &mac_id,
            mac_port,
            (pc_id.public_key(), pc_port),
            mac_fake.clone(),
        ),
        start(
            Machine::B,
            &pc_id,
            pc_port,
            (mac_id.public_key(), mac_port),
            pc_fake.clone(),
        )
    );
    // La Mac se usa: ella captura y decide por dónde cruza.
    take_over(&mac, &mac_fake, &pc).await;
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
        while mac.agent.desk_sync().stamp.rev != pc.agent.desk_sync().stamp.rev
            || mac.agent.desk() != pc.agent.desk()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    // En la PC el acomodo pasa a «la Mac a la derecha».
    let mut desk = pc.agent.desk();
    desk.arrangement = Some(tandem_core::Arrangement {
        a_side: Side::Right,
        align: tandem_core::Align::Center,
    });
    pc.agent.set_desk(desk.clone()).unwrap();
    let mut mac_desk = mac.agent.subscribe_desk();
    within(mac_desk.wait_for(|d| *d == desk)).await.unwrap();
    assert_eq!(
        mac.agent.desk_sync().stamp.rev,
        pc.agent.desk_sync().stamp.rev
    );

    // Ahora la Mac cruza por su borde izquierdo y entra a la PC por el derecho.
    let (x, _) = cross(&mac_fake, &pc_fake, 0.0, -5.0).await;
    assert_eq!(x, 3439.0, "entra por el borde derecho de la PC");
    pc.stop().await;
    mac.stop().await;
}

/// Un `[[screens]]` escrito a mano en `tandem.toml` cambia la geometría: la PC declara su
/// pantalla de 2000×720 aunque lea 3440×1440, y la Mac a su derecha, centradas (D18). El
/// cruce sale del acomodo declarado (el tramo compartido), y el puesto editado llega también
/// a la Mac.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hand_written_screen_in_tandem_toml_changes_the_geometry() {
    let (mac_id, pc_id) = sides(Identity::from_seed([33; 32]), Identity::from_seed([34; 32]));
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    pc_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let declared = "[[screens]]\nmachine = \"b\"\nid = \"34\"\nx = 0\ny = 0\nwidth = 2000\nheight = 720\n\n[arrangement]\na_side = \"right\"\nalign = \"center\"\n";
    let (mac, pc) = tandemd::start_pair!(
        |mac_port, pc_port| start(
            Machine::A,
            &mac_id,
            mac_port,
            (pc_id.public_key(), pc_port),
            mac_fake.clone(),
        ),
        start_full(
            Machine::B,
            &pc_id,
            pc_port,
            (mac_id.public_key(), mac_port),
            pc_fake.clone(),
            "",
            declared,
        )
    );
    let pc_serving = |s: &InputStatus| s.server == Machine::B && s.session;
    pc.until_input(pc_serving).await;
    both_on(&mac, &pc).await;

    // Centradas, los 720 de la PC quedan frente a 180..900 de los 1080 de la Mac: a 500 por
    // el borde derecho declarado (x = 1999) entra a la Mac a 180 + 500 = 680.
    let (x, y) = cross(&pc_fake, &mac_fake, 1999.0, 5.0).await;
    assert_eq!(x, 0.0, "entra por el borde izquierdo de la Mac");
    assert!((y - 680.0).abs() < 1e-6, "{y}");

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
    let mac = tandemd::start_solo!(|port, peer_port| start(
        Machine::A,
        &mac_id,
        port,
        (pc_id.public_key(), peer_port),
        fake.clone(),
    ));
    // Las dos máquinas capturan (D7): la falla se ve aunque esta no mande.
    let problem = Some("falta el permiso de Accesibilidad".to_owned());
    mac.until_input(|s| s.problem == problem).await;
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
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    pc_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let shortcut = "lock_shortcut = [\"ControlLeft\", \"KeyL\"]\n";
    let (mac, pc) = tandemd::start_pair!(
        |mac_port, pc_port| start_with(
            Machine::A,
            &mac_id,
            mac_port,
            (pc_id.public_key(), pc_port),
            mac_fake.clone(),
            shortcut,
        ),
        start_with(
            Machine::B,
            &pc_id,
            pc_port,
            (mac_id.public_key(), mac_port),
            pc_fake.clone(),
            shortcut,
        )
    );
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
    let fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    let mac = tandemd::start_solo!(|mac_port, pc_port| start_with(
        Machine::A,
        &mac_id,
        mac_port,
        (pc_id.public_key(), pc_port),
        fake.clone(),
        BARE,
    ));
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
    let mut mac = tandemd::start_solo!(|port, peer_port| start(
        Machine::A,
        &mac_id,
        port,
        (pc_id.public_key(), peer_port),
        fake.clone(),
    ));
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
    let mac = tandemd::start_solo!(|port, peer_port| start(
        Machine::A,
        &mac_id,
        port,
        (pc_id.public_key(), peer_port),
        fake.clone(),
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(fake.permission_requests(), 0);
    mac.stop().await;
}

/// El cursor en vivo (T-128) entre dos agentes: sin suscripción no se emite nada; con la UI
/// de la PC mirando, llegan las muestras de la Mac (que tiene el foco, por el canal de
/// control) y el cruce, a como mucho 30 Hz; al dejar de mirar, la Mac deja de muestrear.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_live_cursor_flows_only_while_the_ui_watches_and_crosses_between_agents() {
    let (mac_id, pc_id) = sides(Identity::from_seed([41; 32]), Identity::from_seed([42; 32]));
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    pc_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let (mac, mut pc) = tandemd::start_pair!(
        |mac_port, pc_port| start(
            Machine::A,
            &mac_id,
            mac_port,
            (pc_id.public_key(), pc_port),
            mac_fake.clone(),
        ),
        start(
            Machine::B,
            &pc_id,
            pc_port,
            (mac_id.public_key(), mac_port),
            pc_fake.clone(),
        )
    );
    let pc_serving = |s: &InputStatus| s.server == Machine::B && s.session;
    mac.until_input(pc_serving).await;
    pc.until_input(pc_serving).await;
    both_on(&mac, &pc).await;

    // Sin suscripción: el cursor cruza a la Mac, se mueve y vuelve, y no se emite nada.
    cross(&pc_fake, &mac_fake, 0.0, -5.0).await;
    for _ in 0..20 {
        pc_fake.capture(InputEvent::MotionRelative { dx: -1.0, dy: 1.0 });
    }
    pc_fake.capture(InputEvent::MotionRelative { dx: 50.0, dy: 0.0 });
    pc_fake.capture(InputEvent::MotionAbsolute { x: 100.0, y: 100.0 });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!pc.agent.cursor_gate().is_on() && !mac.agent.cursor_gate().is_on());
    assert_eq!(
        pc.agent.cursor_published(),
        0,
        "la PC emitió sin suscripción"
    );
    assert_eq!(
        mac.agent.cursor_published(),
        0,
        "la Mac emitió sin suscripción"
    );

    // La UI de la PC mira: la Mac se entera por el control y enciende su interruptor.
    assert_eq!(
        pc.ui
            .request(UiCommand::WatchCursor { on: true })
            .await
            .unwrap(),
        Reply::Done
    );
    let mac_gate = mac.agent.cursor_gate();
    within(async {
        while !mac_gate.is_on() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    // El cursor cruza a la Mac: la UI de la PC ve el cruce (de su propio motor) y las
    // muestras de la Mac, que tiene el foco.
    let mut crossed = None;
    let mut from_mac = Vec::new();
    within(async {
        let mut tries = 0;
        while crossed.is_none() || from_mac.is_empty() {
            // Cruce y movimiento dentro de la Mac, repetidos por si la geometría llega tarde.
            if tries % 10 == 0 {
                pc_fake.capture(InputEvent::MotionAbsolute { x: 0.0, y: 500.0 });
                pc_fake.capture(InputEvent::MotionRelative { dx: -5.0, dy: 0.0 });
            }
            pc_fake.capture(InputEvent::MotionRelative { dx: -1.0, dy: 0.0 });
            tries += 1;
            while let Ok(Ok(Some(ev))) =
                tokio::time::timeout(Duration::from_millis(20), pc.ui.next_event()).await
            {
                match ev {
                    AgentToUi::Crossed { from, to, edge } => crossed = Some((from, to, edge)),
                    AgentToUi::Cursor(s) if s.machine == Machine::A => from_mac.push(s),
                    _ => {}
                }
            }
        }
    })
    .await;
    assert_eq!(crossed, Some((Machine::B, Machine::A, Side::Left)));
    let sample = &from_mac[0];
    assert_eq!(sample.screen, "m32");
    assert!(
        (0..1920).contains(&sample.x) && (0..1080).contains(&sample.y),
        "{sample:?}"
    );
    assert!(pc.agent.cursor_published() > 0);

    // Frecuencia: 400 movimientos a 1000 Hz producen muchas menos muestras que eventos,
    // nunca más de una cada 33 ms.
    let started = std::time::Instant::now();
    let before = from_mac.len();
    let mut seen = 0usize;
    for i in 0..400 {
        pc_fake.capture(InputEvent::MotionRelative {
            dx: if i % 2 == 0 { 1.0 } else { -1.0 },
            dy: 0.0,
        });
        tokio::time::sleep(Duration::from_millis(1)).await;
        while let Ok(Ok(Some(ev))) =
            tokio::time::timeout(Duration::from_millis(0), pc.ui.next_event()).await
        {
            if matches!(ev, AgentToUi::Cursor(_)) {
                seen += 1;
            }
        }
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    while let Ok(Ok(Some(ev))) =
        tokio::time::timeout(Duration::from_millis(0), pc.ui.next_event()).await
    {
        if matches!(ev, AgentToUi::Cursor(_)) {
            seen += 1;
        }
    }
    let secs = started.elapsed().as_secs_f64();
    let max = (secs * 30.0).ceil() as usize + 2;
    assert!(seen > 0, "ninguna muestra de 400 movimientos");
    assert!(
        seen <= max,
        "{seen} muestras en {secs:.2} s (máximo {max}, antes {before})"
    );

    // Deja de mirar: el interruptor de la Mac se apaga y no emite más.
    assert_eq!(
        pc.ui
            .request(UiCommand::WatchCursor { on: false })
            .await
            .unwrap(),
        Reply::Done
    );
    within(async {
        while mac_gate.is_on() || pc.agent.cursor_gate().is_on() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (pc_n, mac_n) = (pc.agent.cursor_published(), mac.agent.cursor_published());
    for _ in 0..50 {
        pc_fake.capture(InputEvent::MotionRelative { dx: 1.0, dy: 1.0 });
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(pc.agent.cursor_published(), pc_n);
    assert_eq!(mac.agent.cursor_published(), mac_n);

    pc.stop().await;
    mac.stop().await;
}

/// Dos máquinas con Windows (DECISIONS D2): el teclado y el mouse cruzan igual y Ctrl sigue
/// siendo Ctrl, porque entre dos sistemas iguales no se reasigna nada.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_windows_machines_share_the_keyboard_without_remapping() {
    let (a_id, b_id) = sides(Identity::from_seed([41; 32]), Identity::from_seed([42; 32]));
    let a_fake = FakeInputBackends::new(vec![screen("izq", 1920, 1080)]);
    let b_fake = FakeInputBackends::new(vec![screen("der", 2560, 1440)]);
    b_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let (a, b) = tandemd::start_pair!(
        |a_port, b_port| start_os(
            Machine::A,
            Os::Windows,
            &a_id,
            a_port,
            (b_id.public_key(), b_port),
            a_fake.clone(),
            "",
            "",
        ),
        start_os(
            Machine::B,
            Os::Windows,
            &b_id,
            b_port,
            (a_id.public_key(), a_port),
            b_fake.clone(),
            "",
            "",
        )
    );
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
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    pc_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let (mac, pc) = tandemd::start_pair!(
        |mac_port, pc_port| start(
            Machine::A,
            &mac_id,
            mac_port,
            (pc_id.public_key(), pc_port),
            mac_fake.clone(),
        ),
        start(
            Machine::B,
            &pc_id,
            pc_port,
            (mac_id.public_key(), mac_port),
            pc_fake.clone(),
        )
    );
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

/// Espera a que el puesto de `r` cumpla `f`.
async fn until_desk(r: &Running, f: impl Fn(&tandem_core::Desk) -> bool) {
    let mut rx = r.agent.subscribe_desk();
    within(rx.wait_for(|d| f(d))).await.unwrap();
}

/// T-099 (revisión de #74): Cmd↔Ctrl y el módulo de portapapeles están en el puesto.
/// Apagarlos desde la UI de una máquina cambia el motor de las dos, sin reiniciar los
/// agentes: Ctrl de la PC llega como Ctrl a la Mac y copiar ya no cruza.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn turning_off_cmd_ctrl_and_the_clipboard_in_the_desk_changes_both_engines() {
    let (mac_id, pc_id) = sides(Identity::from_seed([45; 32]), Identity::from_seed([46; 32]));
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    pc_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let (mac, pc) = tandemd::start_pair!(
        |mac_port, pc_port| start(
            Machine::A,
            &mac_id,
            mac_port,
            (pc_id.public_key(), pc_port),
            mac_fake.clone(),
        ),
        start(
            Machine::B,
            &pc_id,
            pc_port,
            (mac_id.public_key(), mac_port),
            pc_fake.clone(),
        )
    );
    let pc_serving = |s: &InputStatus| s.server == Machine::B && s.session;
    mac.until_input(pc_serving).await;
    pc.until_input(pc_serving).await;
    both_on(&mac, &pc).await;

    let ctrl = ModifierKey::ControlLeft.usage();
    let meta = ModifierKey::MetaLeft.usage();
    let a = HidUsage::key(0x04);
    // Cruza a la Mac y teclea Ctrl+A en la PC; devuelve lo que llegó a la Mac.
    let type_ctrl_a = || async {
        let (x, _) = cross(&pc_fake, &mac_fake, 0.0, -5.0).await;
        assert_eq!(x, 1919.0);
        let before = mac_fake.injected().borrow().len();
        pc_fake.capture(key(ctrl, KeyState::Pressed));
        pc_fake.capture(key(a, KeyState::Pressed));
        pc_fake.capture(key(a, KeyState::Released));
        pc_fake.capture(key(ctrl, KeyState::Released));
        tokio::time::sleep(Duration::from_millis(100)).await;
        mac_fake.injected().borrow()[before..]
            .iter()
            .filter(|e| matches!(e, InputEvent::Key { .. }))
            .cloned()
            .collect::<Vec<_>>()
    };
    // Con lo de omisión, Ctrl de la PC llega como Cmd.
    within(async { while !type_ctrl_a().await.contains(&key(meta, KeyState::Pressed)) {} }).await;

    // En la Mac se apagan Cmd↔Ctrl y el portapapeles; el puesto llega a la PC.
    let off = InputPrefs {
        cmd_ctrl: false,
        drag_files: true,
    };
    mac.agent.edit_desk(DeskPatch::Prefs(off)).unwrap();
    // El portapapeles de la Mac, apagado desde la UI de la Mac (va en el puesto, D4).
    assert_eq!(
        mac.agent.handle(UiCommand::SetModule {
            machine: Machine::A,
            module: tandem_proto::ModuleId::Clipboard,
            on: false,
        }),
        Reply::Done
    );
    until_desk(&pc, |d| {
        d.prefs == off && d.modules.get(Machine::A, "clipboard") == Some(false)
    })
    .await;
    // El motor de la PC (el que captura) se rearma: Ctrl llega como Ctrl.
    within(async {
        loop {
            let got = type_ctrl_a().await;
            if got.contains(&key(ctrl, KeyState::Pressed)) {
                assert!(!got.contains(&key(meta, KeyState::Pressed)), "{got:?}");
                break;
            }
        }
    })
    .await;

    // Y lo copiado ya no cruza, en ningún sentido.
    let text = |s: &str| ClipContent {
        text: Some(s.into()),
        html: None,
        image: None,
    };
    mac_fake.clipboard().copy(Some(text("solo en la Mac")));
    pc_fake.clipboard().copy(Some(text("solo en la PC")));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_ne!(
        pc_fake.clipboard().subscribe().borrow().content,
        Some(text("solo en la Mac"))
    );
    assert_ne!(
        mac_fake.clipboard().subscribe().borrow().content,
        Some(text("solo en la PC"))
    );

    pc.stop().await;
    mac.stop().await;
}

/// T-099 (revisión de #74): el atajo de bloqueo es del puesto. Cambiarlo desde la UI de la
/// Mac lo cambia en el motor de la PC (el que captura) sin reiniciar ningún agente.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_lock_shortcut_changed_in_the_desk_applies_without_restarting() {
    let (mac_id, pc_id) = sides(Identity::from_seed([47; 32]), Identity::from_seed([48; 32]));
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    pc_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let (mac, pc) = tandemd::start_pair!(
        |mac_port, pc_port| start(
            Machine::A,
            &mac_id,
            mac_port,
            (pc_id.public_key(), pc_port),
            mac_fake.clone(),
        ),
        start(
            Machine::B,
            &pc_id,
            pc_port,
            (mac_id.public_key(), mac_port),
            pc_fake.clone(),
        )
    );
    pc.until_input(|s| s.session && s.server == Machine::B)
        .await;
    assert!(!pc.agent.input_status().locked);

    mac.agent
        .edit_desk(DeskPatch::Shortcuts(Shortcuts {
            lock: vec!["KeyK".into()],
        }))
        .unwrap();
    until_desk(&pc, |d| d.shortcuts.lock == ["KeyK"]).await;
    // Hasta que el motor se rearme, K es una tecla más; después, bloquea.
    let k = HidUsage::key(0x0E);
    within(async {
        while !pc.agent.input_status().locked {
            pc_fake.capture(key(k, KeyState::Pressed));
            pc_fake.capture(key(k, KeyState::Released));
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    // El atajo de antes ya no hace nada: K otra vez lo desbloquea.
    let scroll = HidUsage::key(0x47);
    pc_fake.capture(key(scroll, KeyState::Pressed));
    pc_fake.capture(key(scroll, KeyState::Released));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(pc.agent.input_status().locked);
    pc_fake.capture(key(k, KeyState::Pressed));
    pc.until_input(|s| !s.locked).await;
    pc.stop().await;
    mac.stop().await;
}

/// Un puerto que otra suite tomó entre que se eligió y se abrió (la carrera de las pruebas
/// que se colgaban con la máquina cargada): el agente no se pasa a un puerto de reserva que
/// el otro no conoce, falla enseguida, y `start_pair!` elige otros con los que el par se conecta.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_port_taken_before_starting_fails_fast_and_the_pair_picks_new_ones() {
    let held = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let taken = held.local_addr().unwrap().port();
    let (a_id, b_id) = sides(Identity::from_seed([51; 32]), Identity::from_seed([52; 32]));
    let a_fake = FakeInputBackends::new(vec![screen("izq", 1920, 1080)]);
    let b_fake = FakeInputBackends::new(vec![screen("der", 2560, 1440)]);

    let alone = within(start(
        Machine::A,
        &a_id,
        taken,
        (b_id.public_key(), free_port().unwrap()),
        a_fake.clone(),
    ))
    .await;
    assert!(
        matches!(alone, Err(PortTaken)),
        "tenía que fallar por el puerto"
    );

    let mut first = Some(taken);
    let (a, b) = tandemd::start_pair!(
        |a_port, b_port| start(
            Machine::A,
            &a_id,
            first.take().unwrap_or(a_port),
            (b_id.public_key(), b_port),
            a_fake.clone(),
        ),
        start(
            Machine::B,
            &b_id,
            b_port,
            (a_id.public_key(), a_port),
            b_fake.clone(),
        )
    );
    assert!(first.is_none(), "el primer intento usó el puerto tomado");
    a.until_input(|s| s.session).await;
    b.until_input(|s| s.session).await;
    drop(held);
    b.stop().await;
    a.stop().await;
}

/// La sesión de entrada no se abre antes de saber el sistema de la otra máquina. Si se
/// abría, al llegar sus capacidades se rearmaba la captura y la sesión nueva empezaba con
/// el cursor en casa: un cruce ya hecho se perdía (otra causa de las esperas que vencían
/// en estas pruebas, encontrada en la revisión de T-101).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_input_session_waits_for_the_other_machines_os() {
    for seed in 0..2u8 {
        let (mac_id, pc_id) = sides(
            Identity::from_seed([60 + seed; 32]),
            Identity::from_seed([70 + seed; 32]),
        );
        let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
        let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
        let (mac, pc) = tandemd::start_pair!(
            |mac_port, pc_port| start(
                Machine::A,
                &mac_id,
                mac_port,
                (pc_id.public_key(), pc_port),
                mac_fake.clone(),
            ),
            start(
                Machine::B,
                &pc_id,
                pc_port,
                (mac_id.public_key(), mac_port),
                pc_fake.clone(),
            )
        );
        // El servidor (la PC, por la config) abre la sesión: en ese instante ya sabe qué
        // sistema tiene la Mac.
        let mut rx = pc.agent.subscribe_input();
        within(rx.wait_for(|s| s.session)).await.unwrap();
        let known = pc.agent.capabilities().peer.map(|p| p.info.os);
        assert_eq!(
            known,
            Some(Os::MacOs),
            "sesión abierta sin saber el sistema de la otra"
        );
        pc.stop().await;
        mac.stop().await;
    }
}

/// La latencia de la entrada alimenta el `Qos` del camino en las dos máquinas (T-101): la
/// que inyecta con cada muestra y la que captura con el p99 que le informa la otra. Así
/// el guardián de los datos de cualquiera de las dos puede pausarlos.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_input_latency_feeds_the_link_qos_on_both_machines() {
    use tandem_link::manager::LinkKind;
    let (mac_id, pc_id) = sides(Identity::from_seed([81; 32]), Identity::from_seed([82; 32]));
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    let (mac, pc) = tandemd::start_pair!(
        |mac_port, pc_port| start(
            Machine::A,
            &mac_id,
            mac_port,
            (pc_id.public_key(), pc_port),
            mac_fake.clone(),
        ),
        start(
            Machine::B,
            &pc_id,
            pc_port,
            (mac_id.public_key(), mac_port),
            pc_fake.clone(),
        )
    );
    pc.until_input(|s| s.server == Machine::B && s.session)
        .await;
    both_on(&mac, &pc).await;
    cross(&pc_fake, &mac_fake, 0.0, -5.0).await;
    let p99 = |r: &Running| r.agent.link_qos().get(LinkKind::Usb4).input_p99();
    within(async {
        while p99(&mac).is_none() || p99(&pc).is_none() {
            pc_fake.capture(InputEvent::MotionRelative { dx: 0.0, dy: 1.0 });
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    pc.stop().await;
    mac.stop().await;
}

// ---- Arrastre de archivos al cruzar (T-121) ----

mod file_drag {
    use std::path::{Path, PathBuf};

    use tandem_files::FilesError;
    use tandem_files::drag::{DragEnd, DragIn, DragStarter};
    use tandem_files::drag_os::{DragProbe, PxRect, StripEdge};
    use tandem_input::Button;
    use tandem_proto::DragStatus;
    use tandem_tunnel::BoxFuture;
    use tokio::sync::{Notify, broadcast};

    use super::*;

    type Seen = Arc<Mutex<Vec<(PathBuf, Vec<u8>)>>>;

    /// El detector es la sonda que la prueba enciende; subir es copiar por el disco local
    /// (las dos «máquinas» comparten sistema de archivos).
    #[derive(Clone, Default)]
    struct Hook {
        probe: DragProbe,
        edge: Arc<Mutex<Option<(PxRect, StripEdge)>>>,
        finished: Arc<Mutex<u32>>,
        /// Retiene la copia del archivo con ese nombre hasta que se abra.
        gate: Option<(String, Arc<Notify>)>,
    }

    impl FileDragBackend for Hook {
        fn file_drag(&self) -> Option<Vec<PathBuf>> {
            tandem_files::drag::DragDetector::file_drag(&self.probe)
        }

        fn set_edge(&self, edge: Option<(PxRect, StripEdge)>) {
            *self.edge.lock().unwrap() = edge;
        }

        fn upload<'a>(
            &'a self,
            _agent: &'a Agent,
            local: &'a Path,
            remote: &'a str,
        ) -> BoxFuture<'a, Result<(), FilesError>> {
            Box::pin(async move {
                if let Some((name, gate)) = &self.gate
                    && local.file_name().is_some_and(|n| n.to_str() == Some(name))
                {
                    gate.notified().await;
                }
                std::fs::copy(local, remote)
                    .map(|_| ())
                    .map_err(|e| FilesError::io(local, e))
            })
        }

        fn finish<'a>(&'a self, _agent: &'a Agent) -> BoxFuture<'a, ()> {
            Box::pin(async move { *self.finished.lock().unwrap() += 1 })
        }
    }

    /// Doble del arrastre sintético del destino: lee lo que quedó y «suelta».
    #[derive(Clone, Default)]
    struct DropStarter {
        seen: Seen,
    }

    impl DragStarter for DropStarter {
        async fn start(&self, staged: Vec<PathBuf>) -> Result<DragEnd, FilesError> {
            for p in staged {
                let bytes = std::fs::read(&p).map_err(|e| FilesError::io(&p, e))?;
                self.seen.lock().unwrap().push((p, bytes));
            }
            Ok(DragEnd::Dropped)
        }
    }

    struct Rig {
        mac: Running,
        pc: Running,
        mac_fake: FakeInputBackends,
        pc_fake: FakeInputBackends,
        hook: Hook,
        seen: Seen,
        staging: tempfile::TempDir,
        src: tempfile::TempDir,
        status: broadcast::Receiver<DragStatus>,
        serve: JoinHandle<()>,
    }

    impl Rig {
        async fn stop(self) {
            self.serve.abort();
            self.pc.stop().await;
            self.mac.stop().await;
        }

        /// Los archivos de origen, en la carpeta de la prueba.
        fn files(&self) -> Vec<PathBuf> {
            ["uno.txt", "dos.bin"]
                .iter()
                .map(|n| {
                    let p = self.src.path().join(n);
                    std::fs::write(&p, format!("contenido de {n}")).unwrap();
                    p
                })
                .collect()
        }

        /// Empuja el cursor de la PC contra el borde izquierdo.
        fn push(&self) {
            self.pc_fake
                .capture(InputEvent::MotionAbsolute { x: 0.0, y: 500.0 });
            self.pc_fake
                .capture(InputEvent::MotionRelative { dx: -5.0, dy: 0.0 });
        }

        fn button(&self, state: KeyState) {
            self.pc_fake.capture(InputEvent::Button {
                button: Button::Left,
                state,
            });
        }

        /// Empuja el cursor mientras recoge los avisos de la UI de la PC hasta que llegue
        /// uno que cumpla `last`.
        async fn pushing_until(&mut self, last: impl Fn(&DragStatus) -> bool) -> Vec<DragStatus> {
            let pc_fake = self.pc_fake.clone();
            let pusher = tokio::spawn(async move {
                loop {
                    pc_fake.capture(InputEvent::MotionAbsolute { x: 0.0, y: 500.0 });
                    pc_fake.capture(InputEvent::MotionRelative { dx: -5.0, dy: 0.0 });
                    tokio::time::sleep(Duration::from_millis(30)).await;
                }
            });
            let out = self.statuses_until(last).await;
            pusher.abort();
            out
        }

        async fn statuses_until(&mut self, last: impl Fn(&DragStatus) -> bool) -> Vec<DragStatus> {
            let mut out = Vec::new();
            loop {
                let s = within(self.status.recv()).await.unwrap();
                let done = last(&s);
                out.push(s);
                if done {
                    return out;
                }
            }
        }
    }

    async fn rig(gate: Option<(String, Arc<Notify>)>) -> Rig {
        let (mac_id, pc_id) = sides(Identity::from_seed([21; 32]), Identity::from_seed([22; 32]));
        let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
        let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
        pc_fake.devices().set(vec![KEYBOARD, MOUSE]);
        let hook = Hook {
            gate,
            ..Hook::default()
        };
        let pc_hook: Arc<dyn FileDragBackend> = Arc::new(hook.clone());
        let (mac, pc) = tandemd::start_pair!(
            |mac_port, pc_port| start(
                Machine::A,
                &mac_id,
                mac_port,
                (pc_id.public_key(), pc_port),
                mac_fake.clone(),
            ),
            start_os_drag(
                Machine::B,
                os_of(Machine::B),
                &pc_id,
                pc_port,
                (mac_id.public_key(), mac_port),
                pc_fake.clone(),
                "",
                "",
                Some(pc_hook.clone()),
            )
        );
        let pc_serving = |s: &InputStatus| s.server == Machine::B && s.session;
        mac.until_input(pc_serving).await;
        pc.until_input(pc_serving).await;
        both_on(&mac, &pc).await;

        let starter = DropStarter::default();
        let seen = starter.seen.clone();
        let staging = tempfile::tempdir().unwrap();
        let (agent, base) = (mac.agent.clone(), staging.path().to_owned());
        let serve = tokio::spawn(async move {
            tandemd::drag::serve(&agent, DragIn::new(starter, base)).await;
        });
        let status = pc.agent.drag().subscribe();
        Rig {
            mac,
            pc,
            mac_fake,
            pc_fake,
            hook,
            seen,
            staging,
            src: tempfile::tempdir().unwrap(),
            status,
            serve,
        }
    }

    fn name(i: u32, n: &str) -> DragStatus {
        DragStatus::Copying {
            name: n.into(),
            index: i,
            total: 2,
        }
    }

    fn ended(s: &DragStatus) -> bool {
        matches!(
            s,
            DragStatus::Dropped | DragStatus::Cancelled | DragStatus::Failed(_)
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn un_arrastre_de_archivos_cruza_copia_y_se_suelta_en_el_destino() {
        let mut r = rig(None).await;
        // La franja del detector queda en el borde que mira a la Mac.
        within(async {
            while *r.hook.edge.lock().unwrap() != Some(((0, 0, 3440, 1440), StripEdge::Left)) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        r.hook.probe.enter(r.files());
        r.button(KeyState::Pressed);
        // «Arrastrar entre máquinas» está apagado: solo cruza por los archivos detectados.
        let before = r.mac_fake.injected().borrow().len();
        let statuses = r.pushing_until(ended).await;
        assert_eq!(
            statuses,
            vec![
                name(0, "uno.txt"),
                name(1, "dos.bin"),
                DragStatus::Copied { total: 2 },
                DragStatus::Dropped,
            ]
        );
        assert!(
            r.mac_fake.injected().borrow().len() > before,
            "el cursor cruzó a la Mac"
        );
        let seen = r.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].1, b"contenido de uno.txt");
        assert_eq!(seen[1].1, b"contenido de dos.bin");
        // Soltado el arrastre se limpia: el destino sin restos y el hook avisado.
        within(async {
            while *r.hook.finished.lock().unwrap() == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert_eq!(std::fs::read_dir(r.staging.path()).unwrap().count(), 0);
        r.button(KeyState::Released);
        r.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn soltar_el_boton_a_media_copia_cancela_y_borra_lo_copiado() {
        let gate = Arc::new(Notify::new());
        let mut r = rig(Some(("dos.bin".into(), gate.clone()))).await;
        r.hook.probe.enter(r.files());
        r.button(KeyState::Pressed);
        // La copia queda detenida en el segundo archivo.
        let first = r.pushing_until(|s| *s == name(1, "dos.bin")).await;
        assert_eq!(first, vec![name(0, "uno.txt"), name(1, "dos.bin")]);
        r.button(KeyState::Released);
        let rest = r.statuses_until(ended).await;
        assert_eq!(rest, vec![DragStatus::Cancelled]);
        assert!(r.seen.lock().unwrap().is_empty(), "no se llegó a arrastrar");
        within(async {
            while std::fs::read_dir(r.staging.path()).unwrap().count() != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        r.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sin_archivos_arrastrados_el_cursor_no_cruza_con_el_boton_apretado() {
        let mut r = rig(None).await;
        r.button(KeyState::Pressed);
        let before = r.mac_fake.injected().borrow().len();
        for _ in 0..20 {
            r.push();
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        assert_eq!(r.mac_fake.injected().borrow().len(), before);
        assert!(r.status.try_recv().is_err(), "ningún aviso de arrastre");
        r.stop().await;
    }
}
