//! Sesión de entrada entre dos agentes del mismo proceso, por loopback con QUIC
//! real, con captura e inyección simuladas.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tandem_core::{Config, Machine, State};
use tandem_input::event::KeyState;
use tandem_input::geometry::{Layout, LayoutOptions, Point, Rect, Screen, ScreenId};
use tandem_input::session::fake::{CaptureCall, FakeCapture, FakeInjector};
use tandem_input::session::{
    ClientDriver, ClientSession, ServerDriver, ServerOutput, ServerSession, SessionConfig,
    SessionEnd,
};
use tandem_input::{Button, HidUsage, InputEvent, Scroll};
use tandem_link::identity::{Identity, PublicKey};
use tandem_link::transport::{Connection, Endpoint, PeerTrust};
use tandem_proto::{Hello, Peer};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

const LOOPBACK: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
/// Tope para que una prueba rota falle en vez de colgarse.
const TIMEOUT: Duration = Duration::from_secs(10);

struct Only(PublicKey);

impl PeerTrust for Only {
    fn is_trusted(&self, key: &PublicKey) -> bool {
        *key == self.0
    }
}

fn cfg() -> SessionConfig {
    SessionConfig {
        heartbeat_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_millis(400),
        resync_idle: Duration::from_millis(20),
        resync_every: 8,
    }
}

fn desk() -> Config {
    Config::from_toml(
        r#"
        [[monitors]]
        id = "m34"
        label = '34"'
        wiring = { pc = "displayport", mac = "hdmi" }

        [[monitors]]
        id = "m32"
        label = '32"'
        wiring = { pc = "displayport", mac = "hdmi" }
        "#,
    )
    .unwrap()
}

fn mac_screens() -> Vec<Screen> {
    vec![Screen::new(
        ScreenId::Monitor("m32".into()),
        Rect::new(3440, 0, 1920, 1080),
        1.0,
    )]
}

/// 34" en la PC (servidor, izquierda); 32" en la Mac (cliente, derecha).
fn layout() -> Layout {
    let pc = [Screen::new(
        ScreenId::Monitor("m34".into()),
        Rect::new(0, 0, 3440, 1440),
        1.0,
    )];
    let mut st = State::initial(&desk(), Machine::Mac);
    st.owner.insert("m34".into(), Machine::Pc);
    st.on.pc = true;
    let opts = LayoutOptions {
        corner_margin: 0.0,
        ..LayoutOptions::default()
    };
    Layout::compute(&desk(), &st, &mac_screens(), &pc, opts).unwrap()
}

fn start() -> Point {
    Point::new(3400.0, 720.0)
}

fn server_session() -> ServerSession {
    ServerSession::new(Machine::Pc, layout(), start(), cfg().resync_every)
}

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .expect("la prueba superó el tiempo máximo")
}

/// Relé UDP delante del cliente: "desconectar el cable" es que tire los
/// paquetes en los dos sentidos, sin avisar a nadie.
struct Relay {
    addr: SocketAddr,
    plugged: Arc<AtomicBool>,
    task: JoinHandle<()>,
}

impl Relay {
    async fn new(target: SocketAddr) -> Self {
        let front = UdpSocket::bind(LOOPBACK).await.unwrap();
        let back = UdpSocket::bind(LOOPBACK).await.unwrap();
        back.connect(target).await.unwrap();
        let addr = front.local_addr().unwrap();
        let plugged = Arc::new(AtomicBool::new(true));
        let flag = plugged.clone();
        let task = tokio::spawn(async move {
            let mut peer = None;
            let (mut up, mut down) = (vec![0u8; 65536], vec![0u8; 65536]);
            loop {
                tokio::select! {
                    Ok((n, from)) = front.recv_from(&mut up) => {
                        peer = Some(from);
                        if flag.load(Ordering::SeqCst) {
                            let _ = back.send(&up[..n]).await;
                        }
                    }
                    Ok(n) = back.recv(&mut down) => {
                        if let (true, Some(p)) = (flag.load(Ordering::SeqCst), peer) {
                            let _ = front.send_to(&down[..n], p).await;
                        }
                    }
                }
            }
        });
        Self {
            addr,
            plugged,
            task,
        }
    }

    fn unplug(&self) {
        self.plugged.store(false, Ordering::SeqCst);
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Las dos puntas de una conexión QUIC real, a través de un relé: la del
/// servidor (PC) y la del cliente (Mac).
struct Link {
    server_ep: Endpoint,
    client_ep: Endpoint,
    relay: Relay,
    server: Connection,
    client: Connection,
}

async fn link() -> Link {
    let pc = Identity::from_seed([2; 32]);
    let mac = Identity::from_seed([1; 32]);
    let server_ep = Endpoint::bind(LOOPBACK, &pc, Arc::new(Only(mac.public_key()))).unwrap();
    let client_ep = Endpoint::bind(LOOPBACK, &mac, Arc::new(Only(pc.public_key()))).unwrap();
    let relay = Relay::new(client_ep.local_addr().unwrap()).await;
    let (s, c) =
        within(async { tokio::join!(server_ep.connect(relay.addr), client_ep.accept()) }).await;
    Link {
        server_ep,
        client_ep,
        relay,
        server: s.unwrap(),
        client: c.unwrap().unwrap(),
    }
}

struct Running<D> {
    stop: oneshot::Sender<()>,
    task: JoinHandle<(D, SessionEnd)>,
}

impl<D> Running<D> {
    async fn stop(self) -> (D, SessionEnd) {
        let _ = self.stop.send(());
        within(self.task).await.unwrap()
    }

    async fn ended(self) -> (D, SessionEnd) {
        within(self.task).await.unwrap()
    }
}

fn spawn_server(
    conn: Connection,
    events: mpsc::UnboundedReceiver<InputEvent>,
) -> (
    Running<ServerDriver<FakeCapture>>,
    watch::Receiver<Vec<CaptureCall>>,
    watch::Receiver<bool>,
) {
    let capture = FakeCapture::new();
    let calls = capture.calls();
    let mut driver = ServerDriver::new(server_session(), capture, events, cfg());
    let active = driver.active();
    let (stop, rx) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let hello = Hello::new(Peer::Agent(Machine::Pc));
        let end = driver
            .run(&conn, &hello, async {
                let _ = rx.await;
            })
            .await;
        (driver, end)
    });
    (Running { stop, task }, calls, active)
}

/// Espera a que la sesión del servidor esté establecida: lo que se capture
/// antes se descarta.
async fn up(active: &mut watch::Receiver<bool>) {
    within(active.wait_for(|a| *a)).await.unwrap();
}

fn spawn_client(
    conn: Connection,
) -> (
    Running<ClientDriver<FakeInjector>>,
    watch::Receiver<Vec<InputEvent>>,
) {
    let injector = FakeInjector::new();
    let injected = injector.events();
    let mut driver = ClientDriver::new(ClientSession::new(mac_screens()), injector, cfg());
    let (stop, rx) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let hello = Hello::new(Peer::Agent(Machine::Mac));
        let end = driver
            .run(&conn, &hello, async {
                let _ = rx.await;
            })
            .await;
        (driver, end)
    });
    (Running { stop, task }, injected)
}

fn key(id: u16, state: KeyState) -> InputEvent {
    InputEvent::Key {
        usage: HidUsage::key(id),
        state,
    }
}

fn button(button: Button, state: KeyState) -> InputEvent {
    InputEvent::Button { button, state }
}

fn rel(dx: f64, dy: f64) -> InputEvent {
    InputEvent::MotionRelative { dx, dy }
}

fn is_discrete(e: &InputEvent) -> bool {
    matches!(e, InputEvent::Key { .. } | InputEvent::Button { .. })
}

/// Scroll vertical de rueda inyectado en total.
fn wheel(events: &[InputEvent]) -> i32 {
    events
        .iter()
        .filter_map(|e| match e {
            InputEvent::Scroll(Scroll::Wheel { dy_v120, .. }) => Some(*dy_v120),
            _ => None,
        })
        .sum()
}

fn last_position(events: &[InputEvent]) -> Option<Point> {
    events.iter().rev().find_map(|e| match *e {
        InputEvent::MotionAbsolute { x, y } => Some(Point::new(x, y)),
        _ => None,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn una_secuencia_de_eventos_llega_integra_y_en_orden() {
    let l = link().await;
    let (tx, rx) = mpsc::unbounded_channel();
    let (server, mut calls, mut active) = spawn_server(l.server.clone(), rx);
    let (client, mut injected) = spawn_client(l.client.clone());
    up(&mut active).await;

    // Lo que "captura" la PC: cruza a la Mac con Shift apretado, escribe, arrastra
    // con el botón, hace scroll y sigue moviéndose.
    let mut script = vec![key(0xE1, KeyState::Pressed), rel(60.0, 0.0)];
    script.extend([
        key(0x04, KeyState::Pressed),
        key(0x04, KeyState::Released),
        key(0xE1, KeyState::Released),
        key(0x05, KeyState::Pressed),
        key(0x05, KeyState::Released),
    ]);
    script.extend((0..30).map(|i| rel(3.0, if i % 2 == 0 { 1.0 } else { -2.0 })));
    script.push(button(Button::Left, KeyState::Pressed));
    script.extend((0..20).map(|_| rel(-1.5, 4.0)));
    script.push(button(Button::Left, KeyState::Released));
    for _ in 0..3 {
        script.push(InputEvent::Scroll(Scroll::Wheel {
            dx_v120: 0,
            dy_v120: -120,
        }));
    }
    script.extend((0..25).map(|_| rel(7.0, 2.5)));
    script.push(button(Button::Right, KeyState::Pressed));
    script.push(button(Button::Right, KeyState::Released));
    script.push(key(0x2C, KeyState::Pressed));
    script.push(key(0x2C, KeyState::Released));

    // Lo que tiene que inyectar la Mac, calculado con la misma máquina de estados
    // sin red: el Shift entra con el Enter y las teclas y botones, en orden.
    let mut offline = server_session();
    let mut expected_discrete = Vec::new();
    for e in &script {
        for out in offline.handle(*e) {
            match out {
                ServerOutput::Stream(tandem_input::session::StreamMsg::Enter {
                    modifiers, ..
                }) => expected_discrete.extend(modifiers.iter().map(|m| InputEvent::Key {
                    usage: m.usage(),
                    state: KeyState::Pressed,
                })),
                ServerOutput::Stream(_) if is_discrete(e) => expected_discrete.push(*e),
                _ => {}
            }
        }
    }
    let final_pos = offline.remote_cursor().expect("el guion termina en la Mac");
    assert_eq!(expected_discrete.len(), 12, "{expected_discrete:?}");

    // En tandas, como llegarían de la captura.
    for chunk in script.chunks(7) {
        for e in chunk {
            tx.send(*e).unwrap();
        }
        tokio::task::yield_now().await;
    }

    let events = within(injected.wait_for(|ev| {
        ev.iter().filter(|e| is_discrete(e)).count() == expected_discrete.len()
            && last_position(ev) == Some(final_pos)
            && wheel(ev) == -360
    }))
    .await
    .unwrap()
    .clone();

    let discrete: Vec<_> = events.iter().copied().filter(is_discrete).collect();
    assert_eq!(discrete, expected_discrete);
    // El primer movimiento es el de entrada, en el borde de la Mac.
    assert!(matches!(events[0], InputEvent::MotionAbsolute { x, .. } if x == 3440.0));
    // Nunca sale un movimiento fuera de la pantalla de la Mac.
    let mac = mac_screens()[0].rect;
    for e in &events {
        if let InputEvent::MotionAbsolute { x, y } = *e {
            assert!(mac.contains(Point::new(x, y)), "({x}, {y}) fuera de la Mac");
        }
    }
    assert_eq!(calls.borrow_and_update()[0], CaptureCall::Grab(true));

    // Al detener la sesión (cambio de enlace), la PC recupera su entrada.
    let (server, end) = server.stop().await;
    assert!(matches!(end, SessionEnd::Stopped), "{end}");
    assert!(!*active.borrow(), "la sesión ya no está activa");
    assert!(!server.session().is_remote());
    assert!(!server.capture().grabbed());
    let (client, end) = client.ended().await;
    assert!(matches!(end, SessionEnd::Link(_)), "{end}");
    assert!(client.session().pressed().is_empty());

    l.server_ep.close();
    l.client_ep.close();
}

/// Cruza a la Mac y deja la tecla `S` presionada; espera a que el cliente la
/// inyecte.
async fn hold_s(
    tx: &mpsc::UnboundedSender<InputEvent>,
    injected: &mut watch::Receiver<Vec<InputEvent>>,
) {
    for e in [
        rel(60.0, 0.0),
        key(0xE0, KeyState::Pressed),
        key(0x16, KeyState::Pressed),
    ] {
        tx.send(e).unwrap();
    }
    within(injected.wait_for(|ev| ev.contains(&key(0x16, KeyState::Pressed))))
        .await
        .unwrap();
}

/// Lo que quedó presionado en el cliente según lo inyectado.
fn held(events: &[InputEvent]) -> BTreeSet<HidUsage> {
    let mut held = BTreeSet::new();
    for e in events {
        if let InputEvent::Key { usage, state } = *e {
            match state {
                KeyState::Pressed => held.insert(usage),
                KeyState::Released => held.remove(&usage),
            };
        }
    }
    held
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn caida_del_enlace_con_una_tecla_presionada_la_suelta() {
    let l = link().await;
    let (tx, rx) = mpsc::unbounded_channel();
    let (server, calls, mut active) = spawn_server(l.server.clone(), rx);
    let (client, mut injected) = spawn_client(l.client.clone());
    up(&mut active).await;
    hold_s(&tx, &mut injected).await;
    assert_eq!(held(&injected.borrow()).len(), 2);

    // Se corta la conexión a mitad de la tecla.
    l.server.close("cable desconectado");

    let (client, end) = client.ended().await;
    assert!(matches!(end, SessionEnd::Link(_)), "{end}");
    let events = injected.borrow().clone();
    assert!(held(&events).is_empty(), "quedó presionado: {events:?}");
    // Primero la tecla normal, después el Ctrl: soltar no dispara un atajo.
    let n = events.len();
    assert_eq!(
        events[n - 2..],
        [key(0x16, KeyState::Released), key(0xE0, KeyState::Released)]
    );
    assert!(!client.session().is_active());

    // Y la PC deja de tragarse la entrada.
    let (server, end) = server.ended().await;
    assert!(matches!(end, SessionEnd::Link(_)), "{end}");
    assert!(!server.capture().grabbed());
    assert_eq!(calls.borrow().last(), Some(&CaptureCall::Warp(start())));

    l.server_ep.close();
    l.client_ep.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cable_desconectado_sin_aviso_suelta_la_tecla_y_devuelve_el_foco() {
    let l = link().await;
    let (tx, rx) = mpsc::unbounded_channel();
    let (server, calls, mut active) = spawn_server(l.server.clone(), rx);
    let (client, mut injected) = spawn_client(l.client.clone());
    up(&mut active).await;
    hold_s(&tx, &mut injected).await;

    // Nadie avisa: solo dejan de llegar paquetes, y con ellos los latidos.
    l.relay.unplug();

    let (client, end) = client.ended().await;
    assert!(matches!(end, SessionEnd::Silent), "{end}");
    assert!(held(&injected.borrow()).is_empty());
    assert!(client.session().pressed().is_empty());

    let (server, end) = server.ended().await;
    assert!(matches!(end, SessionEnd::Silent), "{end}");
    assert!(!server.capture().grabbed());
    assert!(calls.borrow().contains(&CaptureCall::Grab(false)));

    l.server_ep.close();
    l.client_ep.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cambio_de_enlace_suelta_las_teclas_del_cliente() {
    let l = link().await;
    let (tx, rx) = mpsc::unbounded_channel();
    let (server, _calls, mut active) = spawn_server(l.server.clone(), rx);
    let (client, mut injected) = spawn_client(l.client.clone());
    up(&mut active).await;
    hold_s(&tx, &mut injected).await;

    // El gestor de enlaces avisa de un cambio: la sesión de este enlace termina.
    let (client, end) = client.stop().await;
    assert!(matches!(end, SessionEnd::Stopped), "{end}");
    assert!(held(&injected.borrow()).is_empty());
    assert!(client.session().pressed().is_empty());

    // El servidor ve el stream cerrado y recupera la entrada local.
    let (server, end) = server.ended().await;
    assert!(matches!(end, SessionEnd::Link(_)), "{end}");
    assert!(!server.capture().grabbed());

    l.server_ep.close();
    l.client_ep.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lo_capturado_sin_enlace_no_cruza_al_empezar_la_sesion() {
    let l = link().await;
    let (tx, rx) = mpsc::unbounded_channel();
    // Sin enlace (modo Solo Mac, PC apagada) el usuario aprieta Shift y empuja
    // el mouse contra el borde: eso no puede cruzar cuando el enlace vuelva.
    for e in [
        key(0xE1, KeyState::Pressed),
        rel(60.0, 0.0),
        rel(60.0, 0.0),
        InputEvent::Scroll(Scroll::Wheel {
            dx_v120: 0,
            dy_v120: -120,
        }),
    ] {
        tx.send(e).unwrap();
    }
    let (server, calls, mut active) = spawn_server(l.server.clone(), rx);
    let (client, mut injected) = spawn_client(l.client.clone());
    up(&mut active).await;

    // Ya con enlace: una tecla con el foco todavía local (no viaja), después el
    // cruce de verdad y otra tecla.
    tx.send(key(0x04, KeyState::Pressed)).unwrap();
    tx.send(rel(60.0, 0.0)).unwrap();
    tx.send(key(0x05, KeyState::Pressed)).unwrap();
    let events = within(injected.wait_for(|ev| ev.contains(&key(0x05, KeyState::Pressed))))
        .await
        .unwrap()
        .clone();

    // Cruza una sola vez, desde donde estaba el cursor y con el Shift que el
    // usuario sigue apretando; el movimiento y el scroll viejos no llegan.
    let mut offline = server_session();
    offline.handle(key(0xE1, KeyState::Pressed));
    offline.handle(rel(60.0, 0.0));
    let entry = offline
        .remote_cursor()
        .expect("el cruce en vivo entra a la Mac");
    assert_eq!(
        events,
        [
            InputEvent::MotionAbsolute {
                x: entry.x,
                y: entry.y
            },
            key(0xE1, KeyState::Pressed),
            key(0x05, KeyState::Pressed),
        ]
    );
    let grabs = calls
        .borrow()
        .iter()
        .filter(|c| **c == CaptureCall::Grab(true))
        .count();
    assert_eq!(grabs, 1);

    let (server, _) = server.stop().await;
    assert!(!server.capture().grabbed());
    let _ = client.ended().await;
    l.server_ep.close();
    l.client_ep.close();
}

#[tokio::test]
async fn sin_enlace_lo_capturado_se_consume_sin_cruzar() {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut driver = ServerDriver::new(server_session(), FakeCapture::new(), rx, cfg());
    let edge = Point::new(3439.0, 720.0);
    for e in [
        InputEvent::MotionAbsolute {
            x: edge.x,
            y: edge.y,
        },
        rel(60.0, 0.0),
    ] {
        tx.send(e).unwrap();
    }
    // Lo que ya estaba en el canal se consume antes de que `until` se cumpla.
    let r = within(driver.idle(tokio::time::sleep(Duration::from_millis(50)))).await;
    assert!(r.is_ok());
    assert!(!driver.session().is_remote());
    assert_eq!(driver.session().local_cursor(), edge);
    assert!(driver.capture().calls().borrow().is_empty());

    // Si la captura se cierra, `idle` lo dice.
    drop(tx);
    let r = within(driver.idle(std::future::pending())).await;
    assert!(matches!(r, Err(SessionEnd::CaptureClosed)));
}
