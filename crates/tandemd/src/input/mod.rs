//! Shared keyboard, mouse and clipboard in the agent (DESIGN §7): brings together
//! `tandem-input`, `tandem-clipboard` and `tandem-peripherals` with the network.
//!
//! - **Who sends** (D7, keyboard and mouse without an owner): both machines always capture and
//!   the one that receives physical input sends; arbitration is [`Follow`], with the other agent's
//!   [`ControlMsg::Focus`]. What is declared (configuration or setup) only
//!   says whom to start with. The hub ([`HubWatcher`]) no longer decides: it informs.
//! - **Geometry**: the shared desk's arrangement (DESIGN §7): the declared
//!   `[[screens]]` or, if undeclared, the ones this machine reads (from the backends, every so often) and
//!   the ones the other agent sends ([`ControlMsg::Screens`]); a machine that is off according to the
//!   [`State`](tandem_core::State) does not count. They build the [`Layout`]; it is recomputed live.
//! - **Session**: the server opens a stream of its own over the active link
//!   ([`Channel::Input`]) and runs its [`ServerDriver`]; the client receives that stream from the
//!   other agent and runs its [`ClientDriver`]. When the link changes the session ends
//!   and starts again over the new one.
//! - **Hot path** (T-093): both drivers run in a runtime of their own, on their
//!   own thread, without sharing it with the clipboard, the IPC or the control network.
//! - **Latency** (T-093): measured by whoever receives the input; both machines publish it
//!   in [`InputStatus::latency`] with the link it was measured over.
//! - **Clipboard**: side A opens its stream ([`Channel::Clipboard`]) and B receives it;
//!   both run a [`ClipDriver`] with the keyboard and mouse focus.
//! - **Modifiers**: Cmd<->Ctrl only between macOS and Windows, according to each machine's
//!   system ([`backends::direction`]); if the other's changes, the capture is rearmed.
//! - **Side**: if when pairing this machine turns out to be the other side
//!   ([`Agent::adopt_side`]), the whole engine restarts with the new side.
//! - **Modules** (T-098): with "Keyboard and mouse" off the engine does not run; the clipboard
//!   turns on and off by itself, without cutting the input session.
//! - **State for the UI**: [`InputStatus`] (server, hub, session, locked cursor
//!   and missing macOS permissions).
//!
//! The platform backends go behind [`InputBackends`]; [`fake`] has the
//! doubles for the tests with two agents.

pub mod backends;
pub mod fake;
pub mod file_drag;
pub mod screens;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use tokio::time::Instant;

use tandem_clipboard::{ClipConfig, ClipDriver};
use tandem_core::{Machine, Os};
use tandem_input::geometry::{Arrangement, Layout, LayoutOptions};
use tandem_input::session::{
    ClientDriver, ClientSession, CursorEvent, CursorTap, Follow, FollowConfig, ServerDriver,
    ServerSession, SessionConfig, SessionEnd,
};
use tandem_input::{HidUsage, Point, Screen};
use tandem_peripherals::{HubWatcher, Presence, WatchConfig};
use tandem_proto::{
    Channel, ControlMsg, FocusMsg, HubReport, InputLatency, InputPermission, InputStatus, ModuleId,
};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::agent::{Agent, IncomingChannel};
use crate::drag::FileDragBackend;
use crate::network::ActiveLink;
use crate::peer::open_channel;

pub use backends::{CaptureSink, InputBackends, PlatformInput, WeakCaptureSink};

/// Input engine timings. The normal one is [`InputOptions::default`]; tests
/// speed them up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputOptions {
    pub session: SessionConfig,
    pub clipboard: ClipConfig,
    pub hub: WatchConfig,
    pub layout: LayoutOptions,
    /// Keyboard and mouse arbitration without an owner (D7).
    pub follow: FollowConfig,
    /// How often the hub and the screens are repeated to the other agent, in case a notice
    /// was lost in a link change.
    pub report_every: Duration,
    /// How often this machine's screens are read again (a monitor that is
    /// plugged in, a resolution change).
    pub screens_every: Duration,
    /// How often the macOS permissions are checked (the user grants them in Settings).
    pub permissions_every: Duration,
    /// Wait before retrying what failed (creating the capture, opening a stream).
    pub retry: Duration,
}

impl Default for InputOptions {
    fn default() -> Self {
        InputOptions {
            session: SessionConfig::default(),
            clipboard: ClipConfig::default(),
            hub: WatchConfig::default(),
            layout: LayoutOptions::default(),
            follow: FollowConfig::default(),
            report_every: Duration::from_secs(2),
            screens_every: Duration::from_secs(5),
            permissions_every: Duration::from_secs(5),
            retry: Duration::from_secs(2),
        }
    }
}

/// The backends and their timings.
#[derive(Clone)]
pub struct InputSetup {
    pub backends: Arc<dyn InputBackends>,
    pub opts: InputOptions,
    /// The detector and sender of file drags when crossing (T-121); without it, crossing
    /// with a button pressed follows only the "Drag between machines" preference.
    pub file_drag: Option<Arc<dyn FileDragBackend>>,
}

impl InputSetup {
    /// The ones of this system, with the normal timings.
    pub fn platform() -> Self {
        InputSetup {
            backends: Arc::new(PlatformInput::new()),
            opts: InputOptions::default(),
            file_drag: backends::platform_file_drag(),
        }
    }
}

/// The running input engine. Dropping it stops everything and the capture gives
/// the keyboard and mouse back to this machine.
pub struct Input {
    tasks: Vec<JoinHandle<()>>,
    hot: Option<HotPath>,
}

impl Drop for Input {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
        // When the hot path's runtime is dropped its drivers are dropped: the capture
        // gives the input back.
        self.hot.take();
    }
}

/// Own runtime for capture, sending and injection (T-093): a thread that does not share
/// a queue with the clipboard, the IPC or the control network. QUIC datagrams and streams
/// can be used from here even if the connection lives in the main runtime.
struct HotPath {
    handle: tokio::runtime::Handle,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HotPath {
    fn start() -> std::io::Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let handle = rt.handle().clone();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("tandem-input".into())
            .spawn(move || {
                rt.block_on(async {
                    let _ = stopped.await;
                });
                // What is left running is canceled when the runtime is dropped, on this thread.
                rt.shutdown_background();
            })?;
        Ok(HotPath {
            handle,
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}

impl Drop for HotPath {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        // The thread is not waited for: dropping the engine cannot block a tokio runtime.
        self.thread.take();
    }
}

/// What the engine's tasks share.
struct Shared {
    agent: Arc<Agent>,
    me: Machine,
    backends: Arc<dyn InputBackends>,
    opts: InputOptions,
    lock_keys: Vec<HidUsage>,
    /// Where the pressed scene shortcuts go.
    scene_keys: mpsc::UnboundedSender<String>,
    /// Where the live cursor goes out; the drivers only publish with someone watching.
    cursor: CursorTap,
    link: watch::Receiver<Option<ActiveLink>>,
    /// Who sends the keyboard and mouse according to arbitration (D7).
    server: watch::Sender<Machine>,
    /// Last physical input of this machine (the capture always runs).
    activity: watch::Sender<Option<Instant>>,
    /// Last input latency, from the session here that has it.
    latency: watch::Sender<Option<InputLatency>>,
    hub_here: watch::Sender<Option<bool>>,
    report: watch::Sender<Option<HubReport>>,
    /// All of this machine's screens (what is sent to the other agent).
    screens: watch::Sender<Vec<Screen>>,
    /// This machine's screens that are being watched (the client's).
    visible: watch::Sender<Vec<Screen>>,
    layout: watch::Sender<Layout>,
    /// Established sessions, server and client.
    serving: watch::Sender<bool>,
    receiving: watch::Sender<bool>,
    /// Focus is on the other machine (server) / the cursor is here (client).
    remote: watch::Sender<bool>,
    here: watch::Sender<bool>,
    locked: watch::Sender<bool>,
    /// A mouse button is pressed (to watch the file drag).
    held: watch::Sender<bool>,
    /// A file drag is in progress: the session lets crossing with the button pressed.
    file_drag: Arc<AtomicBool>,
    missing: watch::Sender<Vec<InputPermission>>,
    capture_problem: watch::Sender<Option<String>>,
    inject_problem: watch::Sender<Option<String>>,
}

/// Starts the input engine over `link`, the network's active link. It must be
/// called inside a tokio runtime.
pub fn start(
    agent: Arc<Agent>,
    link: watch::Receiver<Option<ActiveLink>>,
    setup: InputSetup,
) -> Input {
    agent.set_input_engine();
    Input {
        tasks: vec![tokio::spawn(supervise(agent, link, setup))],
        hot: None,
    }
}

/// What of the desk the engine takes on start: the lock shortcut and Cmd<->Ctrl (DESIGN
/// §4.4). Not the geometry: that is recomputed live; nor the clipboard: it is a
/// module that turns on and off by itself ([`clipboard_module`]).
fn engine_prefs(desk: &tandem_core::Desk) -> (Vec<String>, bool) {
    (desk.shortcuts.lock.clone(), desk.prefs.cmd_ctrl)
}

/// The engine with the current side and preferences; if this machine changes side or
/// the desk's preferences change (from either UI), it is dropped whole
/// (the capture gives the input back) and built again. With the "Keyboard and mouse" module
/// off it does not run.
async fn supervise(
    agent: Arc<Agent>,
    link: watch::Receiver<Option<ActiveLink>>,
    setup: InputSetup,
) {
    let mut side = agent.subscribe_machine();
    let mut modules = agent.subscribe_modules();
    let mut desk = agent.subscribe_desk();
    loop {
        side.borrow_and_update();
        let on = modules.borrow_and_update().is_on(ModuleId::Input);
        let prefs = engine_prefs(&desk.borrow_and_update());
        let engine = on.then(|| spawn_engine(agent.clone(), link.clone(), setup.clone()));
        if !on {
            // Without an engine nobody publishes the state: the session and the lock no longer exist.
            let mut status = agent.input_status();
            status.session = false;
            status.locked = false;
            agent.set_input_status(status);
        }
        let restart = loop {
            tokio::select! {
                () = changed(&mut side) => break true,
                () = changed(&mut modules) => {
                    if modules.borrow_and_update().is_on(ModuleId::Input) != on {
                        break false;
                    }
                }
                () = changed(&mut desk) => {
                    if engine_prefs(&desk.borrow_and_update()) != prefs {
                        break true;
                    }
                }
            }
        };
        drop(engine);
        if restart {
            tracing::info!(side = %agent.machine(), "the input engine restarts");
        } else {
            tracing::info!(
                enabled = !on,
                "keyboard and mouse turned on or off by the user"
            );
        }
    }
}

fn spawn_engine(
    agent: Arc<Agent>,
    link: watch::Receiver<Option<ActiveLink>>,
    setup: InputSetup,
) -> Input {
    let cfg = &agent.config().input;
    let me = agent.machine();
    // What is declared (configuration or setup) already comes with the current side.
    let keyboard = agent.keyboard_declared().unwrap_or(cfg.keyboard());
    let desk = agent.desk();
    // A shortcut with a key that does not exist (from a newer version) is left without a shortcut.
    let lock_keys = crate::config::shortcut_keys(&desk.shortcuts.lock).unwrap_or_default();
    // Without screens only an invalid corner margin fails.
    let initial = Layout::compute(&Arrangement::default(), setup.opts.layout);
    let initial = match initial {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("input engine not started: {e}");
            agent.set_module_problem(ModuleId::Files, Some(crate::modules::NO_ENGINE.into()));
            return Input {
                tasks: Vec::new(),
                hot: None,
            };
        }
    };
    let file_drag = setup.file_drag.clone();
    let (scene_keys, scene_fired) = mpsc::unbounded_channel();
    let (cursor, cursor_events) = CursorTap::new(agent.cursor_gate());
    let sh = Arc::new(Shared {
        me,
        backends: setup.backends,
        opts: setup.opts,
        lock_keys,
        scene_keys,
        cursor,
        link,
        server: watch::Sender::new(keyboard),
        activity: watch::Sender::new(None),
        latency: watch::Sender::new(None),
        hub_here: watch::Sender::new(None),
        report: watch::Sender::new(None),
        screens: watch::Sender::new(Vec::new()),
        visible: watch::Sender::new(Vec::new()),
        layout: watch::Sender::new(initial),
        serving: watch::Sender::new(false),
        receiving: watch::Sender::new(false),
        remote: watch::Sender::new(false),
        here: watch::Sender::new(false),
        locked: watch::Sender::new(false),
        held: watch::Sender::new(false),
        file_drag: Arc::new(AtomicBool::new(false)),
        missing: watch::Sender::new(Vec::new()),
        capture_problem: watch::Sender::new(None),
        inject_problem: watch::Sender::new(None),
        agent,
    });

    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (clip_tx, clip_rx) = mpsc::unbounded_channel();
    let (incoming_tx, incoming) = mpsc::unbounded_channel();
    sh.agent.inbox().set_channels(incoming_tx);
    let (focus_tx, focus_rx) = mpsc::unbounded_channel();
    sh.agent.inbox().set_focus(focus_tx);

    let presence = sh.agent.config().input.hub.clone().and_then(|hub| {
        let scanner = sh.backends.scanner()?;
        let w = HubWatcher::new(scanner, hub, sh.opts.hub);
        let rx = w.subscribe();
        Some((w, rx))
    });
    let mut tasks = Vec::new();
    let presence = presence.map(|(w, rx)| {
        tasks.push(tokio::spawn(w.run()));
        rx
    });

    tasks.extend([
        tokio::spawn(route(incoming, input_tx, clip_tx)),
        tokio::spawn(read_screens(sh.clone())),
        tokio::spawn(geometry(sh.clone())),
        tokio::spawn(follow(sh.clone(), presence, focus_rx)),
        tokio::spawn(report(sh.clone())),
        tokio::spawn(permissions(sh.clone())),
        tokio::spawn(status(sh.clone())),
        tokio::spawn(publish_cursor(sh.agent.clone(), cursor_events)),
    ]);
    // Capture, sending and injection on their own thread; if it cannot be created, on the
    // usual runtime (works the same, it only shares the queue).
    let hot = match HotPath::start() {
        Ok(h) => Some(h),
        Err(e) => {
            tracing::warn!("no thread of its own for the input, using the common runtime: {e}");
            None
        }
    };
    let spawn = |f: std::pin::Pin<Box<dyn Future<Output = ()> + Send>>| match &hot {
        Some(h) => h.handle.spawn(f),
        None => tokio::spawn(f),
    };
    tasks.push(spawn(Box::pin(server(sh.clone()))));
    tasks.push(spawn(Box::pin(client(sh.clone(), input_rx))));
    tasks.push(tokio::spawn(clipboard_module(sh.clone(), clip_rx)));
    // Without a detector, "Files" says so with the reason instead of appearing active (T-158).
    sh.agent.set_module_problem(
        ModuleId::Files,
        crate::modules::file_drag_problem(
            cfg!(any(windows, target_os = "macos")),
            file_drag.is_some(),
        )
        .map(str::to_owned),
    );
    if let Some(hook) = file_drag {
        tasks.push(tokio::spawn(file_drag::follow_edge(
            sh.clone(),
            hook.clone(),
        )));
        tasks.push(tokio::spawn(file_drag::follow(sh.clone(), hook)));
    }
    tasks.push(tokio::spawn(
        sh.agent.clone().serve_scene_shortcuts(scene_fired),
    ));
    Input { tasks, hot }
}

fn set<T: PartialEq>(tx: &watch::Sender<T>, value: T) {
    tx.send_if_modified(|v| {
        let changed = *v != value;
        if changed {
            *v = value;
        }
        changed
    });
}

/// Waits for the next change. If the sender no longer exists, it never finishes: this way a `select!`
/// does not spin idle on a closed channel.
async fn changed<T>(rx: &mut watch::Receiver<T>) {
    if rx.changed().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Carries to the agent what the live cursor's drivers publish (T-128): the agent
/// distributes it to the UI and, if the other machine watches, to it. Runs in the common runtime, not in the
/// hot path's.
async fn publish_cursor(agent: Arc<Agent>, mut events: mpsc::UnboundedReceiver<CursorEvent>) {
    while let Some(event) = events.recv().await {
        agent.publish_cursor(event);
    }
}

/// Distributes the streams the other agent opens.
async fn route(
    mut incoming: mpsc::UnboundedReceiver<IncomingChannel>,
    input: mpsc::UnboundedSender<IncomingChannel>,
    clipboard: mpsc::UnboundedSender<IncomingChannel>,
) {
    while let Some(inc) = incoming.recv().await {
        let to = match inc.channel {
            Channel::Input => &input,
            Channel::Clipboard => &clipboard,
            // It never gets here: `peer::serve_stream` handles it (the tunnels).
            Channel::Tunnel => continue,
            // The bandwidth test is answered by `peer::serve_stream`; it does not get here.
            Channel::Measure => continue,
        };
        if to.send(inc).is_err() {
            tracing::debug!("stream from the other agent with nobody to handle it");
        }
    }
}

/// Reads this machine's screens on start and every so often.
async fn read_screens(sh: Arc<Shared>) {
    let mut tick = tokio::time::interval(sh.opts.screens_every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut warned = false;
    loop {
        tick.tick().await;
        let backends = sh.backends.clone();
        let read = move || backends.screens().map(|s| (s, backends.screen_keys()));
        match tokio::task::spawn_blocking(read).await {
            Ok(Ok((list, keys))) => {
                warned = false;
                set(&sh.screens, list);
                // Which monitor each screen is, so the desk learns it (T-340). Without it, the
                // screens still count.
                match keys {
                    Ok(keys) => sh.agent.set_screen_keys(keys),
                    Err(e) => tracing::debug!("could not read which monitor each screen is: {e}"),
                }
            }
            Ok(Err(e)) => {
                if !warned {
                    tracing::warn!("could not read this machine's screens: {e}");
                    warned = true;
                }
            }
            Err(e) => tracing::error!("screen reading: {e}"),
        }
    }
}

/// Recomputes the geometry on every change of state, desk, screens or of what
/// each monitor shows.
async fn geometry(sh: Arc<Shared>) {
    let mut state = sh.agent.subscribe();
    let mut desk = sh.agent.subscribe_desk();
    let mut mine = sh.screens.subscribe();
    let mut theirs = sh.agent.inbox().screens.subscribe();
    let mut monitors = sh.agent.subscribe_monitors();
    loop {
        let st = state.borrow_and_update().clone();
        let d = desk.borrow_and_update().clone();
        // What each monitor shows, and whether it is on and plugged in (T-349).
        let links: screens::Links = monitors
            .borrow_and_update()
            .iter()
            .map(|v| (v.id.clone(), v.link))
            .collect();
        let local = mine.borrow_and_update().clone();
        let remote: Vec<Screen> = theirs
            .borrow_and_update()
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(screens::from_wire)
            .collect();
        match screens::layout(&d, &st, sh.me, (&local, &remote), &links, sh.opts.layout) {
            Ok(l) => {
                set(&sh.visible, l.screens(sh.me).to_vec());
                set(&sh.layout, l);
            }
            Err(e) => tracing::warn!("invalid geometry, continuing with the previous one: {e}"),
        }
        tokio::select! {
            () = changed(&mut state) => {}
            () = changed(&mut desk) => {}
            () = changed(&mut mine) => {}
            () = changed(&mut theirs) => {}
            () = changed(&mut monitors) => {}
        }
    }
}

/// Who sends the keyboard and mouse (D7): the physical input from here, the other
/// agent's messages and what is declared feed the [`Follow`]; what it returns is sent over the
/// active link. On connecting and every so often the state is sent, in case a message was lost
/// with a dropped link.
async fn follow(
    sh: Arc<Shared>,
    mut presence: Option<watch::Receiver<Presence>>,
    mut msgs: mpsc::UnboundedReceiver<FocusMsg>,
) {
    let start = *sh.server.borrow();
    let mut f = Follow::new(sh.me, start, sh.opts.follow);
    let mut link = sh.link.clone();
    let mut activity = sh.activity.subscribe();
    activity.borrow_and_update();
    // What the user said in setup (T-087) overrides everything.
    let mut declared = sh.agent.subscribe_keyboard();
    let mut last_declared = *declared.borrow_and_update();
    let mut current: Option<ActiveLink> = None;
    let mut tick = tokio::time::interval(sh.opts.report_every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let send = |l: &Option<ActiveLink>, m: Option<FocusMsg>| {
        if let (Some(l), Some(m)) = (l, m) {
            l.handle.client.send(ControlMsg::Focus(m));
        }
    };
    loop {
        let now = link.borrow_and_update().clone();
        let fresh = match (&current, &now) {
            (Some(a), Some(b)) => !a.same(b),
            (None, Some(_)) => true,
            _ => false,
        };
        if now.is_none() && current.is_some() {
            // What it sent over the dropped link no longer holds: the next report arrives over
            // the next link.
            sh.agent.inbox().hub.send_replace(None);
        }
        current = now;
        if fresh {
            send(&current, Some(f.state()));
        }
        if *sh.server.borrow() != f.server() {
            tracing::info!(keyboard = %f.server(), "the keyboard and mouse moved to another machine");
        }
        set(&sh.server, f.server());
        let hub = presence.as_mut().map(|p| p.borrow_and_update().hub());
        set(&sh.hub_here, hub);
        set(
            &sh.report,
            hub.map(|hub| HubReport {
                hub,
                server: f.server(),
            }),
        );
        tokio::select! {
            () = changed(&mut link) => {}
            () = changed(&mut activity) => {
                let at = *activity.borrow_and_update();
                if let Some(at) = at {
                    let out = f.local_input(at, current.is_some());
                    send(&current, out);
                }
            }
            msg = msgs.recv() => match msg {
                Some(m) => {
                    let out = f.receive(m, Instant::now());
                    send(&current, out);
                }
                // Another engine took the mailbox: this one no longer receives anything.
                None => std::future::pending::<()>().await,
            },
            () = changed(&mut declared) => {
                let now = *declared.borrow_and_update();
                if now != last_declared {
                    last_declared = now;
                    if let Some(m) = now {
                        let out = f.force(m);
                        send(&current, Some(out));
                    }
                }
            }
            () = async {
                match presence.as_mut() {
                    Some(p) => changed(p).await,
                    None => std::future::pending().await,
                }
            } => {}
            _ = tick.tick() => send(&current, Some(f.state())),
        }
    }
}

/// Tells the other agent the hub and the screens it reads here, when they change and every
/// so often.
async fn report(sh: Arc<Shared>) {
    let mut link = sh.link.clone();
    let mut hub = sh.report.subscribe();
    let mut mine = sh.screens.subscribe();
    loop {
        if let Some(l) = link.borrow_and_update().clone() {
            if let Some(r) = *hub.borrow_and_update() {
                l.handle.client.send(ControlMsg::Hub(r));
            }
            let list = mine
                .borrow_and_update()
                .iter()
                .map(screens::to_wire)
                .collect();
            l.handle.client.send(ControlMsg::Screens(list));
        }
        tokio::select! {
            () = changed(&mut link) => {}
            () = changed(&mut hub) => {}
            () = changed(&mut mine) => {}
            () = tokio::time::sleep(sh.opts.report_every) => {}
        }
    }
}

/// Keeps the missing permissions up to date and asks the system for them at most twice without
/// the user ordering it: once on start, if any is missing (macOS registers the binary
/// that really runs this way and shows its dialog), and afterwards only on the UI's explicit order
/// ([`UiCommand::RequestPermissions`]). Never in a loop: each request is a dialog.
async fn permissions(sh: Arc<Shared>) {
    let mut asks = sh.agent.subscribe_permission_requests();
    asks.borrow_and_update();
    let mut first = true;
    loop {
        let mut now = sh.backends.missing_permissions();
        if first && !now.is_empty() {
            now = sh.backends.request_permissions();
        }
        first = false;
        set(&sh.missing, now);
        let asked = tokio::select! {
            () = tokio::time::sleep(sh.opts.permissions_every) => false,
            r = asks.changed() => r.is_ok(),
        };
        if asked {
            set(&sh.missing, sh.backends.request_permissions());
            first = false;
        }
    }
}

/// It is known where the keyboard is if the user or the configuration said so, if this machine's
/// hub is detected or if the other machine reports its own (T-087).
fn keyboard_defined(
    declared: Option<tandem_core::Machine>,
    hub_here: Option<bool>,
    peer_reports_hub: bool,
) -> bool {
    declared.is_some() || hub_here.is_some() || peer_reports_hub
}

/// What the UI sees.
async fn status(sh: Arc<Shared>) {
    let mut rx = (
        sh.server.subscribe(),
        sh.hub_here.subscribe(),
        sh.serving.subscribe(),
        sh.receiving.subscribe(),
        sh.locked.subscribe(),
        sh.missing.subscribe(),
        sh.capture_problem.subscribe(),
        sh.inject_problem.subscribe(),
        sh.agent.inbox().hub.subscribe(),
        // What the user declares in setup (T-087).
        sh.agent.subscribe_keyboard(),
        sh.latency.subscribe(),
        sh.screens.subscribe(),
        sh.agent.inbox().screens.subscribe(),
    );
    loop {
        let server = *rx.0.borrow_and_update();
        let is_server = server == sh.me;
        // Each read of a `watch` is copied here, outside the literal: two reads of the same
        // lock inside an expression can deadlock if a write arrives between them
        // (the lock gives priority to the writer).
        let hub_here = *rx.1.borrow_and_update();
        let peer_hub = rx.8.borrow_and_update().is_some();
        let declared = *rx.9.borrow_and_update();
        let status = InputStatus {
            server,
            hub_here,
            session: if is_server {
                *rx.2.borrow_and_update()
            } else {
                *rx.3.borrow_and_update()
            },
            locked: is_server && *rx.4.borrow_and_update(),
            missing: rx.5.borrow_and_update().clone(),
            // Both machines capture and inject (D7): either of the two counts,
            // first the one with the current role.
            problem: {
                let capture = rx.6.borrow_and_update().clone();
                let inject = rx.7.borrow_and_update().clone();
                if is_server {
                    capture.or(inject)
                } else {
                    inject.or(capture)
                }
            },
            // The hub, the configuration or setup say so; if not, it is "not set".
            defined: keyboard_defined(declared, hub_here, peer_hub),
            latency: *rx.10.borrow_and_update(),
            screens: rx
                .11
                .borrow_and_update()
                .iter()
                .map(screens::to_wire)
                .collect(),
            peer_screens: rx.12.borrow_and_update().clone().unwrap_or_default(),
        };
        sh.agent.set_input_status(status);
        tokio::select! {
            () = changed(&mut rx.0) => {}
            () = changed(&mut rx.1) => {}
            () = changed(&mut rx.2) => {}
            () = changed(&mut rx.3) => {}
            () = changed(&mut rx.4) => {}
            () = changed(&mut rx.5) => {}
            () = changed(&mut rx.6) => {}
            () = changed(&mut rx.7) => {}
            () = changed(&mut rx.8) => {}
            () = changed(&mut rx.9) => {}
            () = changed(&mut rx.10) => {}
            () = changed(&mut rx.11) => {}
            () = changed(&mut rx.12) => {}
        }
    }
}

/// An input latency measured on this path: the data guardian uses it to
/// pause the data if the input exceeds its target (DESIGN §6). It is fed on both machines,
/// because the data of both compete for the same path: on the one that injects, with each
/// sample; on the one that captures, with the p99 of each window the other reports to it.
/// It goes to the active path's `Qos` (T-101), the same for all the connections over it.
fn feed_qos(sh: &Shared, latency: Duration) {
    let kind = sh.link.borrow().as_ref().map(|l| l.kind);
    if let Some(kind) = kind {
        sh.agent.link_qos().get(kind).record_input_latency(latency);
    }
}

/// Copies the latency a driver publishes into the engine's, with the active link.
/// `feed`: it is what the other machine reports (the server does not measure): its p99 feeds the Qos.
fn forward_latency(sh: Arc<Shared>, mut from: watch::Receiver<Option<InputLatency>>, feed: bool) {
    tokio::spawn(async move {
        loop {
            let value = *from.borrow_and_update();
            if let Some(l) = value {
                if feed {
                    feed_qos(&sh, Duration::from_micros(u64::from(l.p99_us)));
                }
                let link = sh
                    .link
                    .borrow()
                    .as_ref()
                    .map(|a| crate::network::link_of(a.kind));
                set(&sh.latency, Some(InputLatency { link, ..l }));
            }
            if from.changed().await.is_err() {
                return;
            }
        }
    });
}

/// Copies the physical input the capture sees into the engine's.
fn forward_activity(sh: Arc<Shared>, mut from: watch::Receiver<Option<Instant>>) {
    tokio::spawn(async move {
        loop {
            let value = *from.borrow_and_update();
            if value.is_some() {
                sh.activity.send_replace(value);
            }
            if from.changed().await.is_err() {
                return;
            }
        }
    });
}

/// Passes to the driver what the UI asks with "Lock here"; ends when the driver is dropped.
/// What was requested before the driver was built does not count: the counter of that time is taken as seen.
fn forward_lock_requests(mut from: watch::Receiver<(u64, bool)>, to: mpsc::UnboundedSender<bool>) {
    from.borrow_and_update();
    tokio::spawn(async move {
        while from.changed().await.is_ok() {
            let (_, on) = *from.borrow_and_update();
            if to.send(on).is_err() {
                return;
            }
        }
    });
}

/// Copies a driver's `watch` into the engine's; when the driver is dropped it is left at `false`.
fn forward(mut from: watch::Receiver<bool>, to: impl Fn(bool) + Send + 'static) {
    tokio::spawn(async move {
        loop {
            to(*from.borrow_and_update());
            if from.changed().await.is_err() {
                to(false);
                return;
            }
        }
    });
}

fn first_cursor(screens: &[Screen]) -> Point {
    screens.first().map_or(Point::new(0.0, 0.0), |s| {
        Point::new(
            s.rect.left() + f64::from(s.rect.width) / 2.0,
            s.rect.top() + f64::from(s.rect.height) / 2.0,
        )
    })
}

type Server = ServerDriver<backends::DynCapture>;

/// The other machine's system: what it announced or, without an announcement yet, what its
/// desk keeps.
fn peer_os(sh: &Shared) -> Option<Os> {
    sh.agent
        .capabilities()
        .peer
        .map(|p| p.info.os)
        .or_else(|| sh.agent.machine_info(sh.me.other()).map(|i| i.os))
}

async fn new_server(sh: &Arc<Shared>, peer: Option<Os>) -> Result<Server, String> {
    let (tx, rx) = mpsc::unbounded_channel();
    // Without Cmd<->Ctrl in the desk, no remapping (as between two identical systems).
    let peer = peer.filter(|_| sh.agent.desk().prefs.cmd_ctrl);
    let sink = CaptureSink::new(
        tx,
        &sh.agent.config().input.remap,
        sh.agent.config().os,
        peer,
    )
    .with_scenes(sh.agent.scene_shortcuts(sh.scene_keys.clone()));
    if sh.agent.config().input.synthetic_motion {
        sh.agent.set_synthetic_sink(Some(sink.downgrade()));
    }
    // The session sees the keys already remapped: the shortcut, written with the physical ones, too.
    let lock_keys = sh.lock_keys.iter().map(|&k| sink.remap_key(k)).collect();
    let backends = sh.backends.clone();
    let capture = tokio::task::spawn_blocking(move || backends.capture(sink))
        .await
        .map_err(|e| e.to_string())??;
    // Subscribe before reading: if the geometry changes in between (the
    // other's screens arrive together with its capabilities, just when the capture is rearmed), the session
    // receives it; reading first and subscribing after left the change marked as seen and
    // the session without the other machine until the next change.
    let mut layouts = sh.layout.subscribe();
    let layout = layouts.borrow_and_update().clone();
    let cursor = first_cursor(layout.screens(sh.me));
    let session = ServerSession::new(sh.me, layout, cursor, sh.opts.session.resync_every)
        .with_lock_shortcut(lock_keys)
        .with_drag_across(sh.agent.config().input.drag_between_machines)
        .with_file_drag_flag(sh.file_drag.clone());
    let driver = ServerDriver::new(session, capture, rx, sh.opts.session)
        .with_layouts(layouts)
        .with_cursor(sh.cursor.clone());
    forward_lock_requests(sh.agent.subscribe_lock_requests(), driver.lock_control());
    forward_activity(sh.clone(), driver.activity());
    forward_latency(sh.clone(), driver.latency(), true);
    let s = sh.clone();
    forward(driver.active(), move |v| set(&s.serving, v));
    let s = sh.clone();
    forward(driver.remote(), move |v| set(&s.remote, v));
    let s = sh.clone();
    forward(driver.locked(), move |v| set(&s.locked, v));
    let s = sh.clone();
    forward(driver.buttons(), move |v| set(&s.held, v));
    Ok(driver)
}

/// The server session, while this machine is one: over the active link, or
/// consuming what is captured if there is no link.
async fn server(sh: Arc<Shared>) {
    let mut role = sh.server.subscribe();
    let mut link = sh.link.clone();
    let mut caps = sh.agent.subscribe_capabilities();
    let mut driver: Option<Server> = None;
    // The other's system the capture was built with (the modifiers depend on it).
    let mut built_for: Option<Os> = None;
    loop {
        caps.borrow_and_update();
        let peer = peer_os(&sh);
        if driver.is_some() && built_for != peer {
            tracing::info!(other = ?peer, "the other machine changed system: rearming the capture");
            driver = None;
        }
        // Both machines always capture (D7): the one that does not send just catches up and
        // reports the physical input, which is what decides who sends.
        let d = match driver.as_mut() {
            Some(d) => d,
            None => match new_server(&sh, peer).await {
                Ok(d) => {
                    set(&sh.capture_problem, None);
                    built_for = peer;
                    driver.insert(d)
                }
                Err(e) => {
                    tracing::warn!("could not start the capture: {e}");
                    set(&sh.capture_problem, Some(e));
                    tokio::select! {
                        () = changed(&mut role) => {}
                        () = tokio::time::sleep(sh.opts.retry) => {}
                    }
                    continue;
                }
            },
        };
        if *role.borrow_and_update() != sh.me {
            let until = async {
                tokio::select! {
                    () = changed(&mut role) => {}
                    () = until_peer_os_changes(&sh, &mut caps, peer) => {}
                }
            };
            if d.idle(until).await.is_err() {
                driver = None;
            }
            continue;
        }
        let Some(active) = link.borrow_and_update().clone() else {
            let until = async {
                tokio::select! {
                    () = changed(&mut role) => {}
                    () = changed(&mut link) => {}
                }
            };
            if d.idle(until).await.is_err() {
                driver = None;
            }
            continue;
        };
        if peer.is_none() {
            // Without knowing the other's system the session is not opened: when its
            // capabilities arrived the capture would be rearmed (the modifiers depend on it) and the
            // new session would start with the cursor at home, losing a crossing already made.
            // What is here keeps being updated.
            let until = async {
                tokio::select! {
                    () = changed(&mut role) => {}
                    () = changed(&mut link) => {}
                    () = until_peer_os_changes(&sh, &mut caps, peer) => {}
                }
            };
            if d.idle(until).await.is_err() {
                driver = None;
            }
            continue;
        }
        let me = sh.me;
        let conn = active.handle.conn.clone();
        let open = async move {
            open_channel(&conn, me, Channel::Input)
                .await
                .map_err(|e| SessionEnd::Open(e.to_string()))
        };
        let stop = async {
            tokio::select! {
                () = until_link_or_role_changes(&mut link, &active, &mut role, me) => {}
                () = until_peer_os_changes(&sh, &mut caps, peer) => {}
            }
        };
        let end = d.run_with(&active.handle.conn, open, stop).await;
        match end {
            SessionEnd::Stopped => {}
            SessionEnd::CaptureClosed => driver = None,
            _ => {
                // Before retrying, what is captured is consumed without crossing.
                let pause = tokio::time::sleep(sh.opts.retry);
                if d.idle(pause).await.is_err() {
                    driver = None;
                }
            }
        }
    }
}

/// Ends when the system the other machine announces stops being `current`.
async fn until_peer_os_changes(
    sh: &Shared,
    caps: &mut watch::Receiver<tandem_proto::DeskCapabilities>,
    current: Option<Os>,
) {
    loop {
        changed(caps).await;
        if peer_os(sh) != current {
            return;
        }
    }
}

/// Ends when the active link is no longer `current` or this machine stops being `me`
/// as server.
async fn until_link_or_role_changes(
    link: &mut watch::Receiver<Option<ActiveLink>>,
    current: &ActiveLink,
    role: &mut watch::Receiver<Machine>,
    me: Machine,
) {
    let link_changed = async {
        loop {
            changed(link).await;
            if !link.borrow().as_ref().is_some_and(|l| l.same(current)) {
                return;
            }
        }
    };
    let role_changed = async {
        loop {
            changed(role).await;
            if *role.borrow() != me {
                return;
            }
        }
    };
    tokio::select! {
        () = link_changed => {}
        () = role_changed => {}
    }
}

type Client = ClientDriver<backends::DynInjector>;

async fn new_client(sh: &Arc<Shared>) -> Result<Client, String> {
    let backends = sh.backends.clone();
    let injector = tokio::task::spawn_blocking(move || backends.injector())
        .await
        .map_err(|e| e.to_string())??;
    let session = ClientSession::new(sh.visible.borrow().clone());
    let s = sh.clone();
    let driver = ClientDriver::new(session, injector, sh.opts.session)
        .with_screens(sh.visible.subscribe())
        .with_cursor(sh.cursor.clone(), sh.me)
        .on_latency_sample(move |d| feed_qos(&s, d));
    forward_latency(sh.clone(), driver.latency(), false);
    let s = sh.clone();
    forward(driver.here(), move |v| set(&s.here, v));
    Ok(driver)
}

/// The client session: injects what arrives over each input stream the
/// server opens. A new stream replaces the previous one (the server switched link).
async fn client(sh: Arc<Shared>, mut streams: mpsc::UnboundedReceiver<IncomingChannel>) {
    let mut driver: Option<Client> = None;
    let mut next = streams.recv().await;
    while let Some(inc) = next.take() {
        let d = match driver.as_mut() {
            Some(d) => d,
            None => match new_client(&sh).await {
                Ok(d) => {
                    set(&sh.inject_problem, None);
                    driver.insert(d)
                }
                Err(e) => {
                    tracing::warn!("could not start the injection: {e}");
                    set(&sh.inject_problem, Some(e));
                    // The stream is closed: the server retries and by then perhaps
                    // can already.
                    drop(inc);
                    next = streams.recv().await;
                    continue;
                }
            },
        };
        set(&sh.receiving, true);
        let IncomingChannel { conn, stream, .. } = inc;
        let mut following = None;
        let stop = async {
            following = streams.recv().await;
        };
        let end = d
            .run_with(&conn, async { Ok::<_, SessionEnd>(stream) }, stop)
            .await;
        set(&sh.receiving, false);
        tracing::debug!("input session (receives the keyboard): {end}");
        next = match following {
            Some(f) => Some(f),
            None => streams.recv().await,
        };
    }
}

/// The keyboard and mouse focus is on this machine.
fn focus(sh: &Shared, server: Machine) -> bool {
    if server == sh.me {
        !*sh.remote.borrow()
    } else {
        *sh.here.borrow()
    }
}

async fn track_focus(sh: Arc<Shared>, tx: watch::Sender<bool>) {
    let (mut server, mut remote, mut here) = (
        sh.server.subscribe(),
        sh.remote.subscribe(),
        sh.here.subscribe(),
    );
    loop {
        let s = *server.borrow_and_update();
        remote.borrow_and_update();
        here.borrow_and_update();
        set(&tx, focus(&sh, s));
        tokio::select! {
            () = changed(&mut server) => {}
            () = changed(&mut remote) => {}
            () = changed(&mut here) => {}
        }
    }
}

/// The "Clipboard" module: runs while the user has it on. Off, whatever the
/// other agent opens is closed right away.
async fn clipboard_module(sh: Arc<Shared>, mut streams: mpsc::UnboundedReceiver<IncomingChannel>) {
    let mut modules = sh.agent.subscribe_modules();
    let mut open = true;
    loop {
        let on = modules.borrow_and_update().is_on(ModuleId::Clipboard);
        let until_switched = async {
            loop {
                changed(&mut modules).await;
                if modules.borrow_and_update().is_on(ModuleId::Clipboard) != on {
                    return;
                }
            }
        };
        if on {
            tokio::select! {
                () = clipboard(sh.clone(), &mut streams) => return,
                () = until_switched => {}
            }
        } else {
            tokio::pin!(until_switched);
            loop {
                tokio::select! {
                    s = streams.recv(), if open => open = s.is_some(),
                    () = &mut until_switched => break,
                }
            }
        }
    }
}

/// Aborts the task when dropped: the clipboard can be turned off midway.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The clipboard: side A opens the stream on the active link and B receives it (it does not matter
/// which; only one must, and both know which without talking).
async fn clipboard(sh: Arc<Shared>, streams: &mut mpsc::UnboundedReceiver<IncomingChannel>) {
    let backends = sh.backends.clone();
    let cb = match tokio::task::spawn_blocking(move || backends.clipboard()).await {
        Ok(Ok(cb)) => {
            sh.agent.set_module_problem(ModuleId::Clipboard, None);
            cb
        }
        Ok(Err(e)) => {
            tracing::warn!("no shared clipboard: {e}");
            sh.agent.set_module_problem(
                ModuleId::Clipboard,
                Some(format!("Could not open the system clipboard: {e}")),
            );
            // Whatever the other agent opens is closed right away.
            while streams.recv().await.is_some() {}
            return;
        }
        Err(e) => {
            tracing::error!("clipboard: {e}");
            return;
        }
    };
    let (focus_tx, focus_rx) = watch::channel(focus(&sh, *sh.server.borrow()));
    let _tracker = AbortOnDrop(tokio::spawn(track_focus(sh.clone(), focus_tx)));
    let focused = *focus_rx.borrow();
    let mut driver = ClipDriver::new(cb, sh.opts.clipboard, focused).with_focus(focus_rx);
    if sh.me == Machine::A {
        let mut link = sh.link.clone();
        loop {
            let Some(active) = link.borrow_and_update().clone() else {
                changed(&mut link).await;
                continue;
            };
            match open_channel(&active.handle.conn, sh.me, Channel::Clipboard).await {
                Ok(stream) => {
                    let stop = async {
                        loop {
                            changed(&mut link).await;
                            if !link.borrow().as_ref().is_some_and(|l| l.same(&active)) {
                                return;
                            }
                        }
                    };
                    let end = driver.run(stream, stop).await;
                    if !matches!(end, tandem_clipboard::SyncEnd::Stopped) {
                        tokio::time::sleep(sh.opts.retry).await;
                    }
                }
                Err(e) => {
                    tracing::debug!("could not open the clipboard stream: {e}");
                    tokio::time::sleep(sh.opts.retry).await;
                }
            }
        }
    } else {
        let mut next = streams.recv().await;
        while let Some(inc) = next.take() {
            let mut following = None;
            let stop = async {
                following = streams.recv().await;
            };
            driver.run(inc.stream, stop).await;
            next = match following {
                Some(f) => Some(f),
                None => streams.recv().await,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_keyboard_is_defined_by_the_user_or_any_hub_report() {
        use super::keyboard_defined as f;
        use tandem_core::Machine;
        assert!(!f(None, None, false), "nothing, not set");
        assert!(f(Some(Machine::B), None, false));
        assert!(f(None, Some(false), false), "the hub here is detected");
        assert!(f(None, None, true), "the other machine's hub also counts");
    }

    use super::*;

    #[test]
    fn the_cursor_starts_in_the_middle_of_the_first_screen() {
        use tandem_input::geometry::Rect;
        assert_eq!(first_cursor(&[]), Point::new(0.0, 0.0));
        let s = Screen::new("mac-1".into(), Rect::new(-100, 0, 200, 100), 1.0);
        assert_eq!(first_cursor(&[s]), Point::new(0.0, 50.0));
    }

    #[tokio::test]
    async fn closed_watch_never_reports_a_change() {
        let (tx, mut rx) = watch::channel(0);
        drop(tx);
        let r = tokio::time::timeout(Duration::from_millis(20), changed(&mut rx)).await;
        assert!(r.is_err(), "a closed channel wakes nobody");
    }
}
