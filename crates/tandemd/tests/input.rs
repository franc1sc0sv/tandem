//! Keyboard, mouse and clipboard between two `tandemd`s built with [`daemon::start`]:
//! paired identities, real QUIC over loopback and simulated input backends
//! ([`FakeInputBackends`]). The cursor crosses the edge and the events reach the other
//! side, using the other machine's mouse makes it send (D7: no owner), the
//! clipboard syncs and the permissions missing on the Mac reach the UI.
//!
//! The geometry is the arrangement's (DESIGN §7): the default one (A on the left, with
//! the screens each one reads), an edited one that arrives through `ControlMsg::Desk` and one
//! written by hand in `tandem.toml`.
//!
//! Unless the test says otherwise, side A is a Mac and B a PC (the variable names
//! reflect it); the code does not assume it: each agent's `Os` says so.

mod common;

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

/// The two identities sorted by key: the first is side A's (DECISIONS A1), so
/// each test knows which side each agent gets.
fn sides(x: Identity, y: Identity) -> (Identity, Identity) {
    if x.public_key().as_bytes() < y.public_key().as_bytes() {
        (x, y)
    } else {
        (y, x)
    }
}

const KEYBOARD: UsbId = UsbId::new(0x046D, 0xC33F);
const MOUSE: UsbId = UsbId::new(0x046D, 0xC539);

/// The keyboard and mouse hub, and the default arrangement: the Mac to the left of the PC.
const DESK: &str = r#"
[input]
server = "b"

[input.hub]
keyboard = "046d:c33f"
mouse = "046d:c539"
"#;

/// Without a hub or stated keyboard: what the first start generates (T-087).
const DESK_BARE: &str = r#"
[input]
"#;

/// Marker for `start_with`: use [`DESK_BARE`].
const BARE: &str = "<bare>";

/// Cap so a broken test fails instead of hanging.
const TIMEOUT: Duration = Duration::from_secs(20);

/// Says where it waited, to know which step hung.
#[track_caller]
fn within<F: Future>(f: F) -> impl Future<Output = F::Output> {
    let at = std::panic::Location::caller();
    async move {
        match tokio::time::timeout(TIMEOUT, f).await {
            Ok(v) => v,
            Err(_) => panic!("the test exceeded the maximum time waiting at {at}"),
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
            heartbeat_timeout: common::session().heartbeat_timeout,
            heartbeat_interval: Duration::from_millis(100),
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

/// Like [`start`], with extra lines in the `[input]` section.
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

/// Each side's system in these tests, unless they say otherwise.
fn os_of(machine: Machine) -> Os {
    match machine {
        Machine::A => Os::MacOs,
        Machine::B => Os::Windows,
    }
}

/// Like [`start_with`], with extra sections at the end of `tandem.toml` (an arrangement
/// written by hand).
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

/// Like [`start_full`], with the system it is told.
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

/// Like [`start_os`], with the file drag detector (T-121) it is given.
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
    // `TANDEM_TEST_LOG=tandemd::input=debug` shows the agents' log: to
    // understand a wait that expires.
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
            // Without reservation: a taken port fails right away and `start_pair!` chooses another.
            fallback_ports: Vec::new(),
            health: common::health(),
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

/// Waits for what was injected to satisfy `f`.
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

/// Takes the server's cursor to `x` and pushes it `dx` toward the edge, until the other
/// machine receives the input (the new geometry may take an instant to arrive).
/// Returns where it entered.
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

/// The user uses `who`'s mouse until both machines say it sends and the
/// session is up (D7: the one that receives physical input sends, as soon as the other has
/// 300 ms still).
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

/// Waits for each agent to see the other machine on: without that its screens do not
/// count in the geometry.
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
    // The hub starts on the PC.
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

    // Roles by the hub and input session over the link.
    let pc_serving = |s: &InputStatus| s.server == Machine::B && s.session;
    mac.until_input(pc_serving).await;
    pc.until_input(pc_serving).await;
    assert_eq!(pc.agent.input_status().hub_here, Some(true));
    assert_eq!(mac.agent.input_status().hub_here, Some(false));
    // What each agent announces is what setup shows in step 4 (T-151): the PC's
    // hub is seen from both, and the Mac, without a hub, lists no keyboard or mouse.
    for _ in 0..200 {
        let (p, m) = (pc.agent.capabilities(), mac.agent.capabilities());
        if p.here.hub == Some(true) && m.peer.as_ref().and_then(|c| c.hub) == Some(true) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let (p, m) = (pc.agent.capabilities(), mac.agent.capabilities());
    assert_eq!(p.here.hub, Some(true));
    assert_eq!(m.here.hub, Some(false));
    assert_eq!(m.peer.as_ref().and_then(|c| c.hub), Some(true));
    assert!(pc_fake.capturing());

    // Default arrangement: the Mac to the left of the PC, with the screens each one reads.
    both_on(&mac, &pc).await;

    // The cursor leaves through the PC's left edge and enters the Mac through its right edge.
    let (x, _) = cross(&pc_fake, &mac_fake, 0.0, -5.0).await;
    let calls = pc_fake.capture_calls().borrow().clone();
    assert!(calls.contains(&CaptureCall::Grab(true)), "{calls:?}");
    assert_eq!(x, 1919.0, "enters through the Mac's right edge");

    // The keys arrive, with the PC's Ctrl as Cmd on the Mac.
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

    // The clipboard goes both ways.
    let text = |s: &str| ClipContent {
        text: Some(s.into()),
        html: None,
        image: None,
    };
    mac_fake.clipboard().copy(Some(text("from the Mac")));
    let mut pc_clip = pc_fake.clipboard().subscribe();
    within(pc_clip.wait_for(|s| s.content == Some(text("from the Mac"))))
        .await
        .unwrap();
    // A copy right after the driver writes is taken as its own write
    // (the known gap of `tandem-clipboard`, of microseconds): the user does not copy
    // that fast, the test does.
    tokio::time::sleep(Duration::from_millis(100)).await;
    pc_fake.clipboard().copy(Some(text("from the PC")));
    let mut mac_clip = mac_fake.clipboard().subscribe();
    within(mac_clip.wait_for(|s| s.content == Some(text("from the PC"))))
        .await
        .unwrap();

    // Both always capture (D7).
    assert!(mac_fake.capturing());
    // The user takes the Mac's mouse: as soon as the PC has been still for 300 ms, the
    // Mac sends in both agents. The hub decides nothing.
    take_over(&mac, &mac_fake, &pc).await;
    assert_eq!(pc.agent.input_status().hub_here, Some(true));
    // On ceasing to be the server the PC returned the cursor to its screen.
    let calls = pc_fake.capture_calls().borrow().clone();
    assert_eq!(
        calls
            .iter()
            .rev()
            .find(|c| matches!(c, CaptureCall::Grab(_))),
        Some(&CaptureCall::Grab(false)),
        "{calls:?}"
    );

    // Now the Mac captures and the cursor crosses through its right edge to the PC.
    let (x, _) = cross(&mac_fake, &pc_fake, 1919.0, 5.0).await;
    assert_eq!(x, 0.0, "enters through the PC's left edge");
    let b = HidUsage::key(0x05);
    mac_fake.capture(key(b, KeyState::Pressed));
    until_injected(&pc_fake, |e| e.last() == Some(&key(b, KeyState::Pressed))).await;

    // The Mac is missing a permission: the UI receives it as a notice.
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

    // The PC is used again while the Mac types: it is not taken from it until the Mac
    // has been still for 300 ms.
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
            "the Mac was still in use"
        );
    }
    take_over(&pc, &pc_fake, &mac).await;

    pc.stop().await;
    // Without the other agent there is no session; the Mac keeps the last thing decided until
    // it is used.
    mac.until_input(|s| !s.session && s.server == Machine::B)
        .await;
    // Without a link, the one in use sends. The session drops before the link (which waits for its
    // own health deadline): while the link stays up the Mac asks and nobody answers,
    // so the user keeps moving the mouse until the link drops.
    within(async {
        while mac.agent.input_status().server != Machine::A {
            mac_fake.capture(InputEvent::MotionRelative { dx: 0.0, dy: 1.0 });
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    mac.stop().await;
}

/// An arrangement edited on the PC reaches the Mac through `ControlMsg::Desk` and changes where
/// its cursor crosses (DESIGN §7).
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
    // The Mac is used: it captures and decides where it crosses.
    take_over(&mac, &mac_fake, &pc).await;
    both_on(&mac, &pc).await;

    // By default the Mac is on the left: it crosses through its right edge.
    let (x, _) = cross(&mac_fake, &pc_fake, 1919.0, 5.0).await;
    assert_eq!(x, 0.0);
    // The cursor returns to the Mac where it entered (the PC's left edge): the Mac
    // releases the capture.
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

    // Before editing, both already recorded both machines in the desk (a version
    // of its own from getting to know each other); otherwise, that version could arrive later and trample the edit.
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
    // On the PC the arrangement becomes "the Mac on the right".
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

    // Now the Mac crosses through its left edge and enters the PC through the right one.
    let (x, _) = cross(&mac_fake, &pc_fake, 0.0, -5.0).await;
    assert_eq!(x, 3439.0, "enters through the PC's right edge");
    pc.stop().await;
    mac.stop().await;
}

/// T-150: without `[[screens]]`, the input state carries the screens each engine reads
/// (the ones here and the ones the other machine announced), for the UI to draw them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_input_status_carries_the_real_screens_of_both_machines() {
    let (mac_id, pc_id) = sides(Identity::from_seed([61; 32]), Identity::from_seed([62; 32]));
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
    let both = |s: &InputStatus| !s.screens.is_empty() && !s.peer_screens.is_empty();
    mac.until_input(both).await;
    pc.until_input(both).await;
    let seen = mac.agent.input_status();
    assert_eq!(
        (seen.screens[0].width, seen.screens[0].height),
        (1920, 1080)
    );
    assert_eq!(
        (seen.peer_screens[0].width, seen.peer_screens[0].height),
        (3440, 1440)
    );
    let seen = pc.agent.input_status();
    assert_eq!(
        (seen.screens[0].width, seen.screens[0].height),
        (3440, 1440)
    );
    assert_eq!(seen.peer_screens[0].width, 1920);
}

/// A hand-written `[[screens]]` in `tandem.toml` changes the geometry: the PC declares its
/// 2000x720 screen even though it reads 3440x1440, and the Mac to its right, centered (D18). The
/// crossing comes from the declared arrangement (the shared stretch), and the edited desk also arrives
/// at the Mac.
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

    // Centered, the PC's 720 face 180..900 of the Mac's 1080: at 500 through
    // the declared right edge (x = 1999) it enters the Mac at 180 + 500 = 680.
    let (x, y) = cross(&pc_fake, &mac_fake, 1999.0, 5.0).await;
    assert_eq!(x, 0.0, "enters through the Mac's left edge");
    assert!((y - 680.0).abs() < 1e-6, "{y}");

    // What was written by hand is a change by the PC: the Mac adopts it.
    let want = pc.agent.desk();
    assert_eq!(want.screens.len(), 1);
    let mut mac_desk = mac.agent.subscribe_desk();
    within(mac_desk.wait_for(|d| d.screens == want.screens && d.arrangement == want.arrangement))
        .await
        .unwrap();
    pc.stop().await;
    mac.stop().await;
}

/// If the capture cannot start (on macOS, without Accessibility), the UI sees it and the
/// agent retries on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capture_that_fails_is_reported_and_retried() {
    let (mac_id, pc_id) = sides(Identity::from_seed([13; 32]), Identity::from_seed([14; 32]));
    let fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    fake.devices().set(vec![KEYBOARD]);
    fake.fail_capture(Some("the Accessibility permission is missing".into()));
    let mac = tandemd::start_solo!(|port, peer_port| start(
        Machine::A,
        &mac_id,
        port,
        (pc_id.public_key(), peer_port),
        fake.clone(),
    ));
    // Both machines capture (D7): the failure is seen even if this one does not send.
    let problem = Some("the Accessibility permission is missing".to_owned());
    mac.until_input(|s| s.problem == problem).await;
    assert!(!mac.agent.input_status().session);
    assert!(!fake.capturing());
    // The user grants the permission: the next attempt starts the capture.
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

/// The lock shortcut is written with the physical keys of whoever uses it. The capture
/// swaps Ctrl for Cmd before the session sees the keys: even so, Ctrl+L on the PC
/// has to lock the cursor.
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

    // The same again unlocks it.
    assert!(pc_fake.capture(key(ctrl, KeyState::Pressed)));
    assert!(pc_fake.capture(key(l, KeyState::Pressed)));
    pc.until_input(|s| !s.locked).await;
    pc.stop().await;
    mac.stop().await;
}

/// T-129: "Lock here" from the UI has the same effect as the shortcut: the cursor stops
/// crossing, the input state says so, and `on: false` releases it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lock_here_from_the_ui_locks_the_input_like_the_shortcut() {
    let (mac_id, pc_id) = sides(Identity::from_seed([17; 32]), Identity::from_seed([18; 32]));
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    pc_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let (mac, mut pc) = tandemd::start_pair!(
        |mac_port, pc_port| start_with(
            Machine::A,
            &mac_id,
            mac_port,
            (pc_id.public_key(), pc_port),
            mac_fake.clone(),
            "",
        ),
        start_with(
            Machine::B,
            &pc_id,
            pc_port,
            (mac_id.public_key(), mac_port),
            pc_fake.clone(),
            "",
        )
    );
    pc.until_input(|s| s.session && s.server == Machine::B)
        .await;
    assert!(!pc.agent.input_status().locked);

    let reply = pc
        .ui
        .request(UiCommand::LockHere { on: true })
        .await
        .unwrap();
    assert_eq!(reply, Reply::Done);
    pc.until_input(|s| s.locked).await;
    // The other machine does not find out: the lock belongs to where the keyboard is.
    assert!(!mac.agent.input_status().locked);

    pc.ui
        .request(UiCommand::LockHere { on: false })
        .await
        .unwrap();
    pc.until_input(|s| !s.locked).await;
    pc.stop().await;
    mac.stop().await;
}

/// T-087: the input state is rebuilt when the user declares where the keyboard is,
/// also if they declare the machine that already was the server (neither `server` nor the hub change, so
/// only the declaration can wake the state task).
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
    // Without a hub or declaration: not set, and the engine starts as if it were the Mac.
    mac.until_input(|s| s.server == Machine::A && !s.defined)
        .await;
    // The Mac is declared: it already was the server, so nothing else changes.
    let reply = mac
        .agent
        .handle(tandem_proto::UiCommand::SetKeyboard(Machine::A));
    assert_eq!(reply, tandem_proto::Reply::Done);
    mac.until_input(|s| s.defined && s.server == Machine::A)
        .await;
    // And it holds: the state task also sees the declaration.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(mac.agent.input_status().defined);
    mac.stop().await;
}

/// T-090: with missing permissions, the agent asks the system for them once on start (so
/// macOS registers the real binary), does not ask again on its own however much they stay
/// missing, and asks again when the UI orders it. The capabilities carry the process.
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
    // Several permission reads later (every 50 ms), it is still a single request.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(fake.permission_requests(), 1);

    // Explicit order from the UI: it is requested again, and this time the user grants.
    fake.grant_on_request(true);
    assert_eq!(
        mac.ui.request(UiCommand::RequestPermissions).await.unwrap(),
        Reply::Done
    );
    mac.until_input(|s| s.missing.is_empty()).await;
    assert_eq!(fake.permission_requests(), 2);

    // The process that runs reaches the capabilities so the UI says whom to grant to.
    let process = mac.agent.capabilities().here.input.process;
    assert_eq!(process.pid, std::process::id());
    assert!(!process.exe.is_empty());
    assert!(process.started > 0);

    mac.stop().await;
}

/// With no permissions to request, startup opens no dialog.
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

/// T-150, the three outcomes of the crossing test with the double engine and `WatchCursor`: without
/// moving anything no `Crossed` arrives (timeout), a movement to an edge that
/// is not the arrangement's neither (wrong edge: the cursor does not leave there), and through the marked edge
/// `Crossed` and the other machine's `Cursor` arrive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cross_test_outcomes_come_from_the_engine_through_watch_cursor() {
    let (mac_id, pc_id) = sides(Identity::from_seed([63; 32]), Identity::from_seed([64; 32]));
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

    // What reaches the PC's UI during `ms`: the crossings (and the Mac's samples).
    async fn crossings(
        ui: &mut IpcClient<tokio::io::DuplexStream>,
        ms: u64,
    ) -> (Vec<(Machine, Machine, Side)>, usize) {
        let (mut crossed, mut from_mac) = (Vec::new(), 0);
        let end = tokio::time::Instant::now() + Duration::from_millis(ms);
        while let Ok(Ok(Some(ev))) = tokio::time::timeout_at(end, ui.next_event()).await {
            match ev {
                AgentToUi::Crossed { from, to, edge } => crossed.push((from, to, edge)),
                AgentToUi::Cursor(s) if s.machine == Machine::A => from_mac += 1,
                _ => {}
            }
        }
        (crossed, from_mac)
    }

    // Timeout: nobody moves the mouse, there is no crossing.
    let (crossed, _) = crossings(&mut pc.ui, 700).await;
    assert!(crossed.is_empty(), "{crossed:?}");

    // Wrong edge: against the PC's right and top edge, the cursor does not leave.
    for (x, y, dx, dy) in [(3439.0, 700.0, 50.0, 0.0), (1000.0, 0.0, 0.0, -50.0)] {
        pc_fake.capture(InputEvent::MotionAbsolute { x, y });
        pc_fake.capture(InputEvent::MotionRelative { dx, dy });
    }
    let (crossed, from_mac) = crossings(&mut pc.ui, 700).await;
    assert!(crossed.is_empty(), "{crossed:?}");
    assert_eq!(from_mac, 0, "the cursor should not have passed to the Mac");

    // Marked edge (the PC's left): Crossed and the sample on the Mac.
    let mut crossed = Vec::new();
    let mut from_mac = 0;
    within(async {
        let mut tries = 0;
        while crossed.is_empty() || from_mac == 0 {
            if tries % 10 == 0 {
                pc_fake.capture(InputEvent::MotionAbsolute { x: 0.0, y: 500.0 });
                pc_fake.capture(InputEvent::MotionRelative { dx: -5.0, dy: 0.0 });
            }
            pc_fake.capture(InputEvent::MotionRelative { dx: -1.0, dy: 0.0 });
            tries += 1;
            let (c, m) = crossings(&mut pc.ui, 30).await;
            crossed.extend(c);
            from_mac += m;
        }
    })
    .await;
    assert_eq!(crossed[0], (Machine::B, Machine::A, Side::Left));
}

/// T-239: the Windows capture drops what `SendInput` injects (its loop guard), so synthetic
/// motion from the hardware suite never reached the edge on the PC. A test agent with
/// `synthetic_motion` takes `SyntheticMotion` as captured motion: it crosses the PC's edge and
/// reaches the Mac. An agent without it rejects the command, so production input is unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn synthetic_motion_through_the_agent_crosses_only_on_a_test_agent() {
    let (mac_id, pc_id) = sides(Identity::from_seed([71; 32]), Identity::from_seed([72; 32]));
    let mac_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
    let pc_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
    pc_fake.devices().set(vec![KEYBOARD, MOUSE]);
    let (mut mac, mut pc) = tandemd::start_pair!(
        |mac_port, pc_port| start(
            Machine::A,
            &mac_id,
            mac_port,
            (pc_id.public_key(), pc_port),
            mac_fake.clone(),
        ),
        start_with(
            Machine::B,
            &pc_id,
            pc_port,
            (mac_id.public_key(), mac_port),
            pc_fake.clone(),
            "synthetic_motion = true\n",
        )
    );
    let pc_serving = |s: &InputStatus| s.server == Machine::B && s.session;
    mac.until_input(pc_serving).await;
    pc.until_input(pc_serving).await;
    both_on(&mac, &pc).await;

    // Without the switch the agent does not take it.
    match mac
        .ui
        .request(UiCommand::SyntheticMotion { dx: -5, dy: 0 })
        .await
        .unwrap()
    {
        Reply::Rejected(why) => assert!(why.contains("synthetic_motion"), "{why}"),
        other => panic!("the Mac should reject synthetic motion: {other:?}"),
    }

    assert_eq!(
        pc.ui
            .request(UiCommand::WatchCursor { on: true })
            .await
            .unwrap(),
        Reply::Done
    );
    // Only the agent's motion: the fake capture gets nothing, like `SendInput` on Windows.
    let mut crossed = None;
    within(async {
        while crossed.is_none() {
            assert_eq!(
                pc.ui
                    .request(UiCommand::SyntheticMotion { dx: -40, dy: 0 })
                    .await
                    .unwrap(),
                Reply::Done
            );
            let end = tokio::time::Instant::now() + Duration::from_millis(5);
            while let Ok(Ok(Some(ev))) = tokio::time::timeout_at(end, pc.ui.next_event()).await {
                if let AgentToUi::Crossed { from, to, edge } = ev {
                    crossed = Some((from, to, edge));
                }
            }
        }
    })
    .await;
    assert_eq!(crossed, Some((Machine::B, Machine::A, Side::Left)));
    // Once over, the motion keeps going to the Mac, as the latency check needs.
    for _ in 0..5 {
        pc.ui
            .request(UiCommand::SyntheticMotion { dx: 3, dy: 0 })
            .await
            .unwrap();
    }
    until_injected(&mac_fake, |e| {
        e.iter()
            .any(|e| matches!(e, InputEvent::MotionAbsolute { .. }))
    })
    .await;
    mac.stop().await;
    pc.stop().await;
}

/// The live cursor (T-128) between two agents: without a subscription nothing is emitted; with the PC's
/// UI watching, the Mac's samples arrive (it has focus, over the control
/// channel) and the crossing, at 30 Hz at most; when it stops watching, the Mac stops sampling.
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

    // Without a subscription: the cursor crosses to the Mac, moves and returns, and nothing is emitted.
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
        "the PC emitted without a subscription"
    );
    assert_eq!(
        mac.agent.cursor_published(),
        0,
        "the Mac emitted without a subscription"
    );

    // The PC's UI watches: the Mac finds out through control and turns on its switch.
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

    // The cursor crosses to the Mac: the PC's UI sees the crossing (from its own engine) and the
    // Mac's samples, which has focus.
    let mut crossed = None;
    let mut from_mac = Vec::new();
    within(async {
        let mut tries = 0;
        while crossed.is_none() || from_mac.is_empty() {
            // Crossing and movement within the Mac, repeated in case the geometry arrives late.
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

    // Frequency: 400 moves at 1000 Hz produce many fewer samples than events,
    // never more than one every 33 ms.
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
    assert!(seen > 0, "no sample from 400 moves");
    assert!(
        seen <= max,
        "{seen} samples in {secs:.2} s (maximum {max}, before {before})"
    );

    // It stops watching: the Mac's switch turns off and it emits no more.
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

/// Two Windows machines (DECISIONS D2): the keyboard and mouse cross the same and Ctrl is still
/// Ctrl, because between two identical systems nothing is remapped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_windows_machines_share_the_keyboard_without_remapping() {
    let (a_id, b_id) = sides(Identity::from_seed([41; 32]), Identity::from_seed([42; 32]));
    let a_fake = FakeInputBackends::new(vec![screen("left", 1920, 1080)]);
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
    // Each knows the other is Windows.
    let mut caps = b.agent.subscribe_capabilities();
    within(caps.wait_for(|c| c.peer.as_ref().is_some_and(|p| p.info.os == Os::Windows)))
        .await
        .unwrap();

    let (x, _) = cross(&b_fake, &a_fake, 0.0, -5.0).await;
    assert_eq!(x, 1919.0, "enters through A's right edge");
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
    // And both were recorded in the desk, with their system.
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

/// The input modules really turn off (T-098): without the clipboard what is copied is not shared
/// and the keyboard and mouse session continues; without keyboard and mouse there is no capture or
/// session. When turned on again everything returns.
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
    // Copy on the Mac until the PC has what was copied; with `arrives` false, copy once and
    // say whether it arrived.
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
    assert!(copied("before", true).await);

    pc.agent.set_module(ModuleId::Clipboard, false).unwrap();
    assert!(
        !copied("off", false).await,
        "without the clipboard it does not arrive"
    );
    // The input was not cut.
    assert!(pc.agent.input_status().session);

    pc.agent.set_module(ModuleId::Clipboard, true).unwrap();
    assert!(copied("on", true).await);

    // Without keyboard and mouse on the PC there is no capture or session, on both.
    let still = InputEvent::MotionRelative { dx: 0.0, dy: 0.0 };
    pc.agent.set_module(ModuleId::Input, false).unwrap();
    pc.until_input(|s| !s.session).await;
    mac.until_input(|s| !s.session).await;
    // The capture is released when its tasks end: nobody listens to what the user does.
    within(async {
        while pc_fake.capture(still) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;

    pc.agent.set_module(ModuleId::Input, true).unwrap();
    mac.until_input(pc_serving).await;
    pc.until_input(pc_serving).await;
    assert!(pc_fake.capture(still), "the capture came back");

    pc.stop().await;
    mac.stop().await;
}

/// Waits for `r`'s desk to satisfy `f`.
async fn until_desk(r: &Running, f: impl Fn(&tandem_core::Desk) -> bool) {
    let mut rx = r.agent.subscribe_desk();
    within(rx.wait_for(|d| f(d))).await.unwrap();
}

/// T-099 (review of #74): Cmd<->Ctrl and the clipboard module are in the desk.
/// Turning them off from one machine's UI changes both's engine, without restarting the
/// agents: the PC's Ctrl arrives as Ctrl on the Mac and copying no longer crosses.
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
    // Crosses to the Mac and types Ctrl+A on the PC; returns what reached the Mac.
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
    // With the defaults, the PC's Ctrl arrives as Cmd.
    within(async { while !type_ctrl_a().await.contains(&key(meta, KeyState::Pressed)) {} }).await;

    // On the Mac Cmd<->Ctrl and the clipboard are turned off; the desk reaches the PC.
    let off = InputPrefs {
        cmd_ctrl: false,
        drag_files: true,
    };
    mac.agent.edit_desk(DeskPatch::Prefs(off)).unwrap();
    // The Mac's clipboard, turned off from the Mac's UI (it goes in the desk, D4).
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
    // The PC's engine (the one that captures) is rearmed: Ctrl arrives as Ctrl.
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

    // And what is copied no longer crosses, in either direction.
    let text = |s: &str| ClipContent {
        text: Some(s.into()),
        html: None,
        image: None,
    };
    mac_fake.clipboard().copy(Some(text("only on the Mac")));
    pc_fake.clipboard().copy(Some(text("only on the PC")));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_ne!(
        pc_fake.clipboard().subscribe().borrow().content,
        Some(text("only on the Mac"))
    );
    assert_ne!(
        mac_fake.clipboard().subscribe().borrow().content,
        Some(text("only on the PC"))
    );

    pc.stop().await;
    mac.stop().await;
}

/// T-099 (review of #74): the lock shortcut belongs to the desk. Changing it from the
/// Mac's UI changes it in the PC's engine (the one that captures) without restarting any agent.
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
    // Until the engine is rearmed, K is just another key; afterwards, it locks.
    let k = HidUsage::key(0x0E);
    within(async {
        while !pc.agent.input_status().locked {
            pc_fake.capture(key(k, KeyState::Pressed));
            pc_fake.capture(key(k, KeyState::Released));
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    // The earlier shortcut does nothing now: K again unlocks it.
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

/// A port another suite took between choosing and opening it (the race of the tests
/// that hung with the machine loaded): the agent does not move to a reserve port that
/// the other does not know, it fails right away, and `start_pair!` chooses others with which the pair connects.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_port_taken_before_starting_fails_fast_and_the_pair_picks_new_ones() {
    let held = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let taken = held.local_addr().unwrap().port();
    let (a_id, b_id) = sides(Identity::from_seed([51; 32]), Identity::from_seed([52; 32]));
    let a_fake = FakeInputBackends::new(vec![screen("left", 1920, 1080)]);
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
        "it had to fail because of the port"
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
    assert!(first.is_none(), "the first attempt used the taken port");
    a.until_input(|s| s.session).await;
    b.until_input(|s| s.session).await;
    drop(held);
    b.stop().await;
    a.stop().await;
}

/// The input session is not opened before knowing the other machine's system. If it
/// opened, when its capabilities arrived the capture was rearmed and the new session started with
/// the cursor at home: an already made crossing was lost (another cause of the waits that expired
/// in these tests, found in the review of T-101).
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
        // The server (the PC, by config) opens the session: at that instant it already knows which
        // system the Mac has.
        let mut rx = pc.agent.subscribe_input();
        within(rx.wait_for(|s| s.session)).await.unwrap();
        let known = pc.agent.capabilities().peer.map(|p| p.info.os);
        assert_eq!(
            known,
            Some(Os::MacOs),
            "session opened without knowing the other's system"
        );
        pc.stop().await;
        mac.stop().await;
    }
}

/// Input latency feeds the path's `Qos` on both machines (T-101): the one
/// that injects with each sample and the one that captures with the p99 the other reports. This way
/// the data guardian of either can pause them.
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

// ---- File drag when crossing (T-121) ----

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

    /// The detector is the probe the test turns on; uploading is copying through the local disk
    /// (both "machines" share a file system).
    #[derive(Clone, Default)]
    struct Hook {
        probe: DragProbe,
        edge: Arc<Mutex<Option<(PxRect, StripEdge)>>>,
        finished: Arc<Mutex<u32>>,
        /// Holds the copy of the file with that name until it is opened.
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

    /// Double of the destination's synthetic drag: reads what was left and "drops".
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

        /// The source files, in the test folder.
        fn files(&self) -> Vec<PathBuf> {
            ["one.txt", "two.bin"]
                .iter()
                .map(|n| {
                    let p = self.src.path().join(n);
                    std::fs::write(&p, format!("content of {n}")).unwrap();
                    p
                })
                .collect()
        }

        /// Pushes the PC's cursor against the left edge.
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

        /// Pushes the cursor while collecting the PC's UI notices until
        /// one that satisfies `last`.
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

    /// T-158: "Files" comes out active with a drag detector and with the true reason without one,
    /// through the real path (`spawn_engine`), not calling `set_problem` by hand.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn files_module_follows_the_drag_backend() {
        use tandem_proto::{ModuleId, ModuleState};
        let (a_id, b_id) = sides(Identity::from_seed([23; 32]), Identity::from_seed([24; 32]));
        let a_fake = FakeInputBackends::new(vec![screen("m32", 1920, 1080)]);
        let b_fake = FakeInputBackends::new(vec![screen("m34", 3440, 1440)]);
        let hook: Arc<dyn FileDragBackend> = Arc::new(Hook::default());
        let (without, with) = tandemd::start_pair!(
            |a_port, b_port| start_os_drag(
                Machine::A,
                os_of(Machine::A),
                &a_id,
                a_port,
                (b_id.public_key(), b_port),
                a_fake.clone(),
                "",
                "",
                None,
            ),
            start_os_drag(
                Machine::B,
                os_of(Machine::B),
                &b_id,
                b_port,
                (a_id.public_key(), a_port),
                b_fake.clone(),
                "",
                "",
                Some(hook.clone()),
            )
        );
        let state_of = |r: &Running| {
            r.agent
                .capabilities()
                .here
                .modules
                .into_iter()
                .find(|m| m.id == ModuleId::Files)
                .map(|m| m.state)
        };
        let expected =
            tandemd::modules::file_drag_problem(cfg!(any(windows, target_os = "macos")), false)
                .unwrap();
        let want = ModuleState::Unavailable {
            reason: expected.into(),
        };
        // The engine reports its problem on start: wait for the announcement to bring it.
        let mut rx = without.agent.subscribe_capabilities();
        within(rx.wait_for(|_| state_of(&without) == Some(want.clone())))
            .await
            .unwrap();
        assert_eq!(state_of(&with), Some(ModuleState::Active));
        with.stop().await;
        without.stop().await;
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
    async fn a_file_drag_crosses_copies_and_drops_at_the_destination() {
        let mut r = rig(None).await;
        // The detector's strip is left on the edge facing the Mac.
        within(async {
            while *r.hook.edge.lock().unwrap() != Some(((0, 0, 3440, 1440), StripEdge::Left)) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        r.hook.probe.enter(r.files());
        r.button(KeyState::Pressed);
        // "Drag between machines" is off: it only crosses for detected files.
        let before = r.mac_fake.injected().borrow().len();
        let statuses = r.pushing_until(ended).await;
        assert_eq!(
            statuses,
            vec![
                name(0, "one.txt"),
                name(1, "two.bin"),
                DragStatus::Copied { total: 2 },
                DragStatus::Dropped,
            ]
        );
        assert!(
            r.mac_fake.injected().borrow().len() > before,
            "the cursor crossed to the Mac"
        );
        let seen = r.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].1, b"content of one.txt");
        assert_eq!(seen[1].1, b"content of two.bin");
        // Once dropped the drag is cleaned up: the destination without leftovers and the hook notified.
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
    async fn releasing_the_button_midway_through_the_copy_cancels_and_deletes_what_was_copied() {
        let gate = Arc::new(Notify::new());
        let mut r = rig(Some(("two.bin".into(), gate.clone()))).await;
        r.hook.probe.enter(r.files());
        r.button(KeyState::Pressed);
        // The copy is left stopped on the second file.
        let first = r.pushing_until(|s| *s == name(1, "two.bin")).await;
        assert_eq!(first, vec![name(0, "one.txt"), name(1, "two.bin")]);
        r.button(KeyState::Released);
        let rest = r.statuses_until(ended).await;
        assert_eq!(rest, vec![DragStatus::Cancelled]);
        assert!(r.seen.lock().unwrap().is_empty(), "it never got to drag");
        within(async {
            while std::fs::read_dir(r.staging.path()).unwrap().count() != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        r.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn without_dragged_files_the_cursor_does_not_cross_with_the_button_pressed() {
        let mut r = rig(None).await;
        r.button(KeyState::Pressed);
        let before = r.mac_fake.injected().borrow().len();
        for _ in 0..20 {
            r.push();
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        assert_eq!(r.mac_fake.injected().borrow().len(), before);
        assert!(r.status.try_recv().is_err(), "no drag notice");
        r.stop().await;
    }
}
