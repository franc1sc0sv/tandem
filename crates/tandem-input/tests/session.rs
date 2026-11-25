//! Input session between two agents in the same process, over loopback with real
//! QUIC, with simulated capture and injection.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tandem_core::Machine;
use tandem_input::event::KeyState;
use tandem_input::geometry::{Arrangement, Layout, LayoutOptions, Point, Rect, Screen, Side};
use tandem_input::session::fake::{CaptureCall, FakeCapture, FakeInjector};
use tandem_input::session::{
    Captured, ClientDriver, ClientSession, CursorEvent, CursorGate, CursorTap, ServerDriver,
    ServerOutput, ServerSession, SessionConfig, SessionEnd,
};
use tandem_input::{Button, HidUsage, InputEvent, Scroll};
use tandem_link::identity::{Identity, PublicKey};
use tandem_link::transport::{Connection, Endpoint, PeerTrust};
use tandem_proto::{CursorSample, Hello, Peer};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

const LOOPBACK: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
/// Cap so that a broken test fails instead of hanging.
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
        latency_every: Duration::from_millis(250),
    }
}

fn mac_screens() -> Vec<Screen> {
    vec![Screen::new(
        "m32".into(),
        Rect::new(3440, 0, 1920, 1080),
        1.0,
    )]
}

/// 34" on the PC (server, left); 32" on the Mac (client, right).
fn layout() -> Layout {
    let pc = vec![Screen::new("m34".into(), Rect::new(0, 0, 3440, 1440), 1.0)];
    let arr = Arrangement {
        a: mac_screens(),
        b: pc,
        a_side: Side::Right,
        align: None,
    };
    Layout::compute(&arr, LayoutOptions { corner_margin: 0.0 }).unwrap()
}

fn start() -> Point {
    Point::new(3400.0, 720.0)
}

fn server_session() -> ServerSession {
    ServerSession::new(Machine::B, layout(), start(), cfg().resync_every)
}

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .expect("the test exceeded the maximum time")
}

/// UDP relay in front of the client: "disconnecting the cable" is it dropping the
/// packets in both directions, without telling anyone.
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

/// The two ends of a real QUIC connection, through a relay: the
/// server's (PC) and the client's (Mac).
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
    events: mpsc::UnboundedReceiver<Captured>,
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
        let hello = Hello::new(Peer::Agent(Machine::B));
        let end = driver
            .run(&conn, &hello, async {
                let _ = rx.await;
            })
            .await;
        (driver, end)
    });
    (Running { stop, task }, calls, active)
}

/// Waits for the server's session to be established: what is captured
/// before it is discarded.
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
        let hello = Hello::new(Peer::Agent(Machine::A));
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

/// Vertical wheel scroll injected in total.
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
async fn a_sequence_of_events_arrives_whole_and_in_order() {
    let l = link().await;
    let (tx, rx) = mpsc::unbounded_channel();
    let (server, mut calls, mut active) = spawn_server(l.server.clone(), rx);
    let (client, mut injected) = spawn_client(l.client.clone());
    up(&mut active).await;

    // What the PC "captures": it crosses to the Mac with Shift held, types, drags
    // with the button, scrolls and keeps moving.
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

    // What the Mac has to inject, computed with the same state machine
    // without a network: the Shift enters with the Enter and the keys and buttons, in order.
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
    let final_pos = offline.remote_cursor().expect("the script ends on the Mac");
    assert_eq!(expected_discrete.len(), 12, "{expected_discrete:?}");

    // In batches, as they would arrive from the capture.
    for chunk in script.chunks(7) {
        for e in chunk {
            tx.send(Captured::now(*e)).unwrap();
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
    // The first movement is the entry one, at the Mac's edge.
    assert!(matches!(events[0], InputEvent::MotionAbsolute { x, .. } if x == 3440.0));
    // A movement never comes out outside the Mac's screen.
    let mac = mac_screens()[0].rect;
    for e in &events {
        if let InputEvent::MotionAbsolute { x, y } = *e {
            assert!(mac.contains(Point::new(x, y)), "({x}, {y}) outside the Mac");
        }
    }
    assert_eq!(calls.borrow_and_update()[0], CaptureCall::Grab(true));

    // On stopping the session (link change), the PC recovers its input.
    let (server, end) = server.stop().await;
    assert!(matches!(end, SessionEnd::Stopped), "{end}");
    assert!(!*active.borrow(), "the session is no longer active");
    assert!(!server.session().is_remote());
    assert!(!server.capture().grabbed());
    let (client, end) = client.ended().await;
    assert!(matches!(end, SessionEnd::Link(_)), "{end}");
    assert!(client.session().pressed().is_empty());

    l.server_ep.close();
    l.client_ep.close();
}

/// Crosses to the Mac and leaves the `S` key pressed; waits for the client to
/// inject it.
async fn hold_s(
    tx: &mpsc::UnboundedSender<Captured>,
    injected: &mut watch::Receiver<Vec<InputEvent>>,
) {
    for e in [
        rel(60.0, 0.0),
        key(0xE0, KeyState::Pressed),
        key(0x16, KeyState::Pressed),
    ] {
        tx.send(Captured::now(e)).unwrap();
    }
    within(injected.wait_for(|ev| ev.contains(&key(0x16, KeyState::Pressed))))
        .await
        .unwrap();
}

/// What was left pressed on the client according to what was injected.
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
async fn link_drop_with_a_pressed_key_releases_it() {
    let l = link().await;
    let (tx, rx) = mpsc::unbounded_channel();
    let (server, calls, mut active) = spawn_server(l.server.clone(), rx);
    let (client, mut injected) = spawn_client(l.client.clone());
    up(&mut active).await;
    hold_s(&tx, &mut injected).await;
    assert_eq!(held(&injected.borrow()).len(), 2);

    // The connection is cut in the middle of the key.
    l.server.close("cable disconnected");

    let (client, end) = client.ended().await;
    assert!(matches!(end, SessionEnd::Link(_)), "{end}");
    let events = injected.borrow().clone();
    assert!(held(&events).is_empty(), "left pressed: {events:?}");
    // First the normal key, then the Ctrl: releasing does not trigger a shortcut.
    let n = events.len();
    assert_eq!(
        events[n - 2..],
        [key(0x16, KeyState::Released), key(0xE0, KeyState::Released)]
    );
    assert!(!client.session().is_active());

    // And the PC stops swallowing input.
    let (server, end) = server.ended().await;
    assert!(matches!(end, SessionEnd::Link(_)), "{end}");
    assert!(!server.capture().grabbed());
    assert_eq!(calls.borrow().last(), Some(&CaptureCall::Warp(start())));

    l.server_ep.close();
    l.client_ep.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cable_disconnected_without_notice_releases_the_key_and_returns_the_focus() {
    let l = link().await;
    let (tx, rx) = mpsc::unbounded_channel();
    let (server, calls, mut active) = spawn_server(l.server.clone(), rx);
    let (client, mut injected) = spawn_client(l.client.clone());
    up(&mut active).await;
    hold_s(&tx, &mut injected).await;

    // Nobody notifies: packets just stop arriving, and with them the heartbeats.
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
async fn link_change_releases_the_clients_keys() {
    let l = link().await;
    let (tx, rx) = mpsc::unbounded_channel();
    let (server, _calls, mut active) = spawn_server(l.server.clone(), rx);
    let (client, mut injected) = spawn_client(l.client.clone());
    up(&mut active).await;
    hold_s(&tx, &mut injected).await;

    // The link manager announces a change: this link's session ends.
    let (client, end) = client.stop().await;
    assert!(matches!(end, SessionEnd::Stopped), "{end}");
    assert!(held(&injected.borrow()).is_empty());
    assert!(client.session().pressed().is_empty());

    // The server sees the stream closed and recovers local input.
    let (server, end) = server.ended().await;
    assert!(matches!(end, SessionEnd::Link(_)), "{end}");
    assert!(!server.capture().grabbed());

    l.server_ep.close();
    l.client_ep.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_was_captured_without_a_link_does_not_cross_when_the_session_starts() {
    let l = link().await;
    let (tx, rx) = mpsc::unbounded_channel();
    // Without a link (the PC off) the user presses Shift and pushes
    // the mouse against the edge: that cannot cross when the link comes back.
    for e in [
        key(0xE1, KeyState::Pressed),
        rel(60.0, 0.0),
        rel(60.0, 0.0),
        InputEvent::Scroll(Scroll::Wheel {
            dx_v120: 0,
            dy_v120: -120,
        }),
    ] {
        tx.send(Captured::now(e)).unwrap();
    }
    let (server, calls, mut active) = spawn_server(l.server.clone(), rx);
    let (client, mut injected) = spawn_client(l.client.clone());
    up(&mut active).await;

    // Now with a link: a key with the focus still local (it does not travel), then the
    // real crossing and another key.
    tx.send(Captured::now(key(0x04, KeyState::Pressed)))
        .unwrap();
    tx.send(Captured::now(rel(60.0, 0.0))).unwrap();
    tx.send(Captured::now(key(0x05, KeyState::Pressed)))
        .unwrap();
    let events = within(injected.wait_for(|ev| ev.contains(&key(0x05, KeyState::Pressed))))
        .await
        .unwrap()
        .clone();

    // It crosses only once, from where the cursor was and with the Shift the
    // user is still holding; the old movement and scroll do not arrive.
    let mut offline = server_session();
    offline.handle(key(0xE1, KeyState::Pressed));
    offline.handle(rel(60.0, 0.0));
    let entry = offline
        .remote_cursor()
        .expect("the live crossing enters the Mac");
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
async fn without_a_link_what_is_captured_is_consumed_without_crossing() {
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
        tx.send(Captured::now(e)).unwrap();
    }
    // What was already in the channel is consumed before `until` is met.
    let r = within(driver.idle(tokio::time::sleep(Duration::from_millis(50)))).await;
    assert!(r.is_ok());
    assert!(!driver.session().is_remote());
    assert_eq!(driver.session().local_cursor(), edge);
    assert!(driver.capture().calls().borrow().is_empty());

    // If the capture closes, `idle` says so.
    drop(tx);
    let r = within(driver.idle(std::future::pending())).await;
    assert!(matches!(r, Err(SessionEnd::CaptureClosed)));
}

/// T-093: the client measures latency with the server's stamps and the heartbeat
/// offset, publishes it every window and sends it to the server, which also publishes it.
/// Every movement goes out as soon as it arrives (without waiting for a tick): with spaced events,
/// almost every one is its own datagram.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn latency_is_measured_on_the_client_and_reaches_the_server() {
    let l = link().await;
    let (tx, rx) = mpsc::unbounded_channel();
    let capture = FakeCapture::new();
    let mut server = ServerDriver::new(server_session(), capture, rx, cfg());
    let mut active = server.active();
    let mut server_latency = server.latency();
    let injector = FakeInjector::new();
    let mut injected = injector.events();
    let samples = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = samples.clone();
    let mut client = ClientDriver::new(ClientSession::new(mac_screens()), injector, cfg())
        .on_latency_sample(move |d| seen.lock().unwrap().push(d));
    let mut client_latency = client.latency();
    let (stop_s, stop_rx_s) = oneshot::channel::<()>();
    let (stop_c, stop_rx_c) = oneshot::channel::<()>();
    let conn = l.server.clone();
    let s = tokio::spawn(async move {
        let hello = Hello::new(Peer::Agent(Machine::B));
        server
            .run(&conn, &hello, async {
                let _ = stop_rx_s.await;
            })
            .await
    });
    let conn = l.client.clone();
    let c = tokio::spawn(async move {
        let hello = Hello::new(Peer::Agent(Machine::A));
        client
            .run(&conn, &hello, async {
                let _ = stop_rx_c.await;
            })
            .await
    });
    up(&mut active).await;
    // Two heartbeats with echo before measuring: without an estimated offset nothing is measured.
    tokio::time::sleep(cfg().heartbeat_interval * 3).await;

    tx.send(Captured::now(rel(60.0, 0.0))).unwrap();
    let moves = 100usize;
    for _ in 0..moves {
        tx.send(Captured::now(rel(1.0, 0.0))).unwrap();
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    tx.send(Captured::now(key(0x04, KeyState::Pressed)))
        .unwrap();
    tx.send(Captured::now(key(0x04, KeyState::Released)))
        .unwrap();
    within(injected.wait_for(|e| e.iter().filter(|e| is_discrete(e)).count() == 2))
        .await
        .unwrap();
    // Every movement, its datagram: the injected ones are almost as many as the movements
    // (only what arrives while the previous one is being sent is gathered).
    let positions = injected
        .borrow()
        .iter()
        .filter(|e| matches!(e, InputEvent::MotionAbsolute { .. }))
        .count();
    assert!(
        positions >= moves / 2,
        "only {positions} positions for {moves} movements"
    );

    // The window with the latest is published here and reaches the server too (there may be
    // another before it with the first part).
    let mine = within(client_latency.wait_for(Option::is_some))
        .await
        .unwrap()
        .unwrap();
    within(server_latency.wait_for(|v| v.is_some_and(|v| v == mine)))
        .await
        .unwrap();
    assert!(mine.count >= 1 && mine.p50_us <= mine.p99_us && mine.p99_us <= mine.max_us);
    let mut all = samples.lock().unwrap().clone();
    assert!(all.len() > moves / 2, "only {} samples", all.len());
    all.sort_unstable();
    // Loopback: generous for a loaded runner.
    let p99 = all[all.len() * 99 / 100];
    assert!(p99 < Duration::from_millis(50), "p99 {p99:?}");

    let _ = stop_s.send(());
    let _ = stop_c.send(());
    within(s).await.unwrap();
    within(c).await.unwrap();
    l.server_ep.close();
    l.client_ep.close();
}

/// Capture that takes a while to take the input: meanwhile the driver is busy and what
/// is captured waits in the channel. It does not last a fixed time: it signals it is already busy and
/// waits for the test to release it, so the test decides how much time passed inside.
struct SlowCapture {
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

impl tandem_input::session::Capture for SlowCapture {
    type Error = std::convert::Infallible;

    fn set_grab(&mut self, _grab: bool) -> Result<(), Self::Error> {
        let _ = self.entered.send(());
        // With the sender dropped (on closing) it waits for nobody.
        let _ = self.release.recv();
        Ok(())
    }

    fn warp(&mut self, _pos: Point) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// T-093 (review): latency counts from the capture, not from when the driver takes the
/// event out of the channel. With the driver busy (taking the input on crossing), what was captured
/// in the meantime arrives with that wait in the sample. The test knows when the driver is
/// busy (the capture's notice) and releases it after `busy` from when it captured, so
/// it does not depend on how fast the threads run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn latency_includes_the_wait_with_the_driver_busy() {
    let busy = Duration::from_millis(60);
    let l = link().await;
    let (tx, rx) = mpsc::unbounded_channel();
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    let capture = SlowCapture {
        entered: entered_tx,
        release: release_rx,
    };
    let mut server = ServerDriver::new(server_session(), capture, rx, cfg());
    let mut active = server.active();
    let samples = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = samples.clone();
    let mut client = ClientDriver::new(
        ClientSession::new(mac_screens()),
        FakeInjector::new(),
        cfg(),
    )
    .on_latency_sample(move |d| seen.lock().unwrap().push(d));
    let (stop_s, stop_rx_s) = oneshot::channel::<()>();
    let (stop_c, stop_rx_c) = oneshot::channel::<()>();
    let conn = l.server.clone();
    let s = tokio::spawn(async move {
        let hello = Hello::new(Peer::Agent(Machine::B));
        server
            .run(&conn, &hello, async {
                let _ = stop_rx_s.await;
            })
            .await
    });
    let conn = l.client.clone();
    let c = tokio::spawn(async move {
        let hello = Hello::new(Peer::Agent(Machine::A));
        client
            .run(&conn, &hello, async {
                let _ = stop_rx_c.await;
            })
            .await
    });
    up(&mut active).await;
    // Heartbeats with echo before measuring.
    tokio::time::sleep(cfg().heartbeat_interval * 3).await;

    // Crossing keeps the driver busy taking the input and does not release it until the test wants.
    // What follows is captured from another thread, like the hook's or the tap's, only after
    // the driver signaled it is busy: that way it waits in the channel for sure.
    tx.send(Captured::now(rel(60.0, 0.0))).unwrap();
    let more = tx.clone();
    tokio::task::spawn_blocking(move || {
        entered.recv_timeout(TIMEOUT).unwrap();
        for _ in 0..5 {
            more.send(Captured::now(rel(1.0, 0.0))).unwrap();
        }
        // Floor of the time in the channel: sleeping never lasts less than asked.
        std::thread::sleep(busy);
        release.send(()).unwrap();
        // With the sender dropped, the other input takes do not wait.
    })
    .await
    .unwrap();
    // The wait of the movements arrives in the sample; wait to see it (not the first
    // sample, which may be the crossing's).
    let floor = busy * 2 / 3;
    within(async {
        while !samples.lock().unwrap().iter().any(|d| *d >= floor) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    let worst = samples.lock().unwrap().iter().copied().max().unwrap();
    assert!(
        worst >= floor,
        "the sample does not count the wait in the channel: {worst:?}"
    );

    let _ = stop_s.send(());
    let _ = stop_c.send(());
    within(s).await.unwrap();
    within(c).await.unwrap();
    l.server_ep.close();
    l.client_ep.close();
}

/// Both drivers publish the cursor while someone is watching (T-128): the server its
/// position and the crossing; the client, with the focus, its own. With the switch off nothing
/// comes out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_live_cursor_comes_from_whoever_has_the_focus_and_only_with_the_switch_on() {
    let l = link().await;
    let gate = Arc::new(CursorGate::default());
    let (server_tap, mut server_rx) = CursorTap::new(gate.clone());
    let (client_tap, mut client_rx) = CursorTap::new(gate.clone());

    let (tx, rx) = mpsc::unbounded_channel();
    let capture = FakeCapture::new();
    let mut server =
        ServerDriver::new(server_session(), capture, rx, cfg()).with_cursor(server_tap);
    let active = server.active();
    let (stop_server, stopped) = oneshot::channel::<()>();
    let conn = l.server.clone();
    let server_task = tokio::spawn(async move {
        let hello = Hello::new(Peer::Agent(Machine::B));
        server
            .run(&conn, &hello, async {
                let _ = stopped.await;
            })
            .await
    });
    let injector = FakeInjector::new();
    let mut injected = injector.events();
    let mut client = ClientDriver::new(ClientSession::new(mac_screens()), injector, cfg())
        .with_cursor(client_tap, Machine::A);
    let (stop_client, stopped) = oneshot::channel::<()>();
    let conn = l.client.clone();
    let client_task = tokio::spawn(async move {
        let hello = Hello::new(Peer::Agent(Machine::A));
        client
            .run(&conn, &hello, async {
                let _ = stopped.await;
            })
            .await
    });
    up(&mut { active }).await;

    // Without a subscription: a movement, a crossing and more movement publish nothing.
    for e in [rel(5.0, 0.0), rel(60.0, 0.0), rel(3.0, 1.0)] {
        tx.send(Captured::now(e)).unwrap();
    }
    within(injected.wait_for(|ev| last_position(ev).is_some()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        server_rx.try_recv().is_err(),
        "the server published without a subscription"
    );
    assert!(
        client_rx.try_recv().is_err(),
        "the client published without a subscription"
    );

    // With a subscription: the client, which has the focus, publishes its position with its screen.
    gate.set(true);
    tx.send(Captured::now(rel(2.0, 0.0))).unwrap();
    let sample = within(client_rx.recv()).await.unwrap();
    let CursorEvent::Sample(CursorSample {
        machine,
        screen,
        x,
        y,
    }) = sample
    else {
        panic!("a sample was expected: {sample:?}");
    };
    assert_eq!((machine, screen.as_str()), (Machine::A, "m32"));
    assert!(Rect::new(3440, 0, 1920, 1080).contains(Point::new(f64::from(x), f64::from(y))));
    assert!(
        server_rx.try_recv().is_err(),
        "the server does not have the focus"
    );

    // On coming back through the edge, the server publishes the crossing and its own position.
    tx.send(Captured::now(rel(-100.0, 0.0))).unwrap();
    let mut crossed = None;
    let mut sample = None;
    within(async {
        while crossed.is_none() || sample.is_none() {
            match server_rx.recv().await.unwrap() {
                CursorEvent::Crossed { from, to, edge } => crossed = Some((from, to, edge)),
                CursorEvent::Sample(s) => sample = Some(s),
            }
        }
    })
    .await;
    let (from, to, _) = crossed.unwrap();
    assert_eq!((from, to), (Machine::A, Machine::B));
    let sample = sample.unwrap();
    assert_eq!(
        (sample.machine, sample.screen.as_str()),
        (Machine::B, "m34")
    );

    let _ = stop_server.send(());
    let _ = stop_client.send(());
    let _ = within(server_task).await;
    let _ = within(client_task).await;
    l.server_ep.close();
    l.client_ep.close();
}

/// The injector learns when the cursor arrives and leaves (Windows draws the cursor
/// without a mouse with it), once each, also when the link drops.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_injector_learns_when_the_cursor_arrives_and_leaves() {
    let l = link().await;
    let (tx, rx) = mpsc::unbounded_channel();
    let (server, _calls, mut active) = spawn_server(l.server.clone(), rx);
    let injector = FakeInjector::new();
    let mut focus = injector.focus();
    let mut injected = injector.events();
    let mut driver = ClientDriver::new(ClientSession::new(mac_screens()), injector, cfg());
    let (stop, stop_rx) = oneshot::channel::<()>();
    let client = Running {
        stop,
        task: tokio::spawn(async move {
            let hello = Hello::new(Peer::Agent(Machine::A));
            let end = driver
                .run(&l.client, &hello, async {
                    let _ = stop_rx.await;
                })
                .await;
            (driver, end)
        }),
    };
    up(&mut active).await;
    hold_s(&tx, &mut injected).await;
    tx.send(Captured::now(rel(5.0, 0.0))).unwrap();
    within(focus.wait_for(|f| *f == [true])).await.unwrap();
    l.server.close("cable disconnected");
    let _ = client.ended().await;
    assert_eq!(*focus.borrow(), [true, false]);
    let _ = server.ended().await;
    l.server_ep.close();
    l.client_ep.close();
}

/// Scene "only Mac": every monitor shows the Mac, so the PC (with the keyboard) has no
/// screen and no crossing edge. The cursor goes to the Mac when the session starts;
/// before, it stayed on the invisible PC and the mouse "died".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_every_monitor_on_the_other_machine_the_cursor_goes_there_by_itself() {
    let l = link().await;
    let (tx, rx) = mpsc::unbounded_channel();
    let only_mac = Layout::compute(
        &Arrangement {
            a: mac_screens(),
            b: vec![],
            a_side: Side::Right,
            align: None,
        },
        LayoutOptions { corner_margin: 0.0 },
    )
    .unwrap()
    .with_shown_elsewhere(Machine::B);
    let capture = FakeCapture::new();
    let mut driver = ServerDriver::new(
        ServerSession::new(Machine::B, only_mac, start(), cfg().resync_every),
        capture,
        rx,
        cfg(),
    );
    let mut remote = driver.remote();
    let (stop, stop_rx) = oneshot::channel::<()>();
    let conn = l.server.clone();
    let server = Running {
        stop,
        task: tokio::spawn(async move {
            let hello = Hello::new(Peer::Agent(Machine::B));
            let end = driver
                .run(&conn, &hello, async {
                    let _ = stop_rx.await;
                })
                .await;
            (driver, end)
        }),
    };
    let (client, mut injected) = spawn_client(l.client.clone());
    within(remote.wait_for(|r| *r)).await.unwrap();
    // Moving the mouse moves the Mac's cursor from the centre of its screen.
    tx.send(Captured::now(rel(10.0, 0.0))).unwrap();
    within(injected.wait_for(|ev| last_position(ev) == Some(Point::new(4410.0, 540.0))))
        .await
        .unwrap();
    let (server, _) = server.stop().await;
    assert!(!server.capture().grabbed());
    let _ = client.stop().await;
    l.server_ep.close();
    l.client_ep.close();
}
