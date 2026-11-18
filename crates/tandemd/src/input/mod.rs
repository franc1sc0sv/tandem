//! Teclado, mouse y portapapeles compartidos en el agente (DESIGN §7): junta
//! `tandem-input`, `tandem-clipboard` y `tandem-peripherals` con la red.
//!
//! - **Quién manda** (D7, teclado y mouse sin dueño): las dos máquinas capturan siempre y
//!   manda la que recibe entrada física; el arbitraje es [`Follow`], con los
//!   [`ControlMsg::Focus`] del otro agente. Lo declarado (configuración o asistente) solo
//!   dice con quién se arranca. El hub ([`HubWatcher`]) ya no decide: se informa.
//! - **Geometría**: el acomodo del puesto compartido (DESIGN §7): las `[[screens]]`
//!   declaradas o, sin declarar, las que lee esta máquina (de los backends, cada tanto) y
//!   las que manda el otro agente ([`ControlMsg::Screens`]); una máquina apagada según el
//!   [`State`](tandem_core::State) no cuenta. Arman el [`Layout`]; se recalcula en caliente.
//! - **Sesión**: el servidor abre un stream propio sobre el enlace activo
//!   ([`Channel::Input`]) y corre su [`ServerDriver`]; el cliente recibe ese stream del
//!   otro agente y corre su [`ClientDriver`]. Cuando el enlace cambia la sesión termina
//!   y vuelve a empezar sobre el nuevo.
//! - **Camino caliente** (T-093): los dos drivers corren en un runtime propio, en su
//!   propio hilo, sin compartirlo con el portapapeles, la IPC ni la red de control.
//! - **Latencia** (T-093): la mide quien recibe la entrada; las dos máquinas la publican
//!   en [`InputStatus::latency`] con el enlace por el que se midió.
//! - **Portapapeles**: el lado A abre su stream ([`Channel::Clipboard`]) y B lo recibe;
//!   los dos corren un [`ClipDriver`] con el foco del teclado y mouse.
//! - **Modificadores**: Cmd↔Ctrl solo entre macOS y Windows, según el sistema de cada
//!   máquina ([`backends::direction`]); si el de la otra cambia, la captura se rearma.
//! - **Lado**: si al emparejar esta máquina resulta ser el otro lado
//!   ([`Agent::adopt_side`]), el motor entero vuelve a arrancar con el lado nuevo.
//! - **Módulos** (T-098): con «Teclado y mouse» apagado el motor no corre; el portapapeles
//!   se enciende y se apaga solo, sin cortar la sesión de entrada.
//! - **Estado para la UI**: [`InputStatus`] (servidor, hub, sesión, cursor bloqueado
//!   y permisos de macOS que faltan).
//!
//! Los backends de plataforma van detrás de [`InputBackends`]; [`fake`] tiene los
//! dobles para las pruebas con dos agentes.

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

pub use backends::{CaptureSink, InputBackends, PlatformInput};

/// Tiempos del motor de entrada. Lo normal es [`InputOptions::default`]; las pruebas
/// los aceleran.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputOptions {
    pub session: SessionConfig,
    pub clipboard: ClipConfig,
    pub hub: WatchConfig,
    pub layout: LayoutOptions,
    /// El arbitraje del teclado y mouse sin dueño (D7).
    pub follow: FollowConfig,
    /// Cada cuánto se repiten el hub y las pantallas al otro agente, por si un aviso
    /// se perdió en un cambio de enlace.
    pub report_every: Duration,
    /// Cada cuánto se vuelven a leer las pantallas de esta máquina (un monitor que se
    /// enchufa, un cambio de resolución).
    pub screens_every: Duration,
    /// Cada cuánto se miran los permisos de macOS (el usuario los concede en Ajustes).
    pub permissions_every: Duration,
    /// Espera antes de reintentar lo que falló (crear la captura, abrir un stream).
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

/// Los backends y sus tiempos.
#[derive(Clone)]
pub struct InputSetup {
    pub backends: Arc<dyn InputBackends>,
    pub opts: InputOptions,
    /// El detector y el envío de los arrastres de archivos al cruzar (T-121); sin él, el
    /// cruce con botón apretado sigue solo la preferencia «Arrastrar entre máquinas».
    pub file_drag: Option<Arc<dyn FileDragBackend>>,
}

impl InputSetup {
    /// Los de este sistema, con los tiempos normales.
    pub fn platform() -> Self {
        InputSetup {
            backends: Arc::new(PlatformInput::new()),
            opts: InputOptions::default(),
            file_drag: backends::platform_file_drag(),
        }
    }
}

/// El motor de entrada en marcha. Al soltarlo se detiene todo y la captura devuelve
/// el teclado y el mouse a esta máquina.
pub struct Input {
    tasks: Vec<JoinHandle<()>>,
    hot: Option<HotPath>,
}

impl Drop for Input {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
        // Al soltarse el runtime del camino caliente se sueltan sus drivers: la captura
        // devuelve la entrada.
        self.hot.take();
    }
}

/// Runtime propio de la captura, el envío y la inyección (T-093): un hilo que no comparte
/// cola con el portapapeles, la IPC ni la red de control. Los datagramas y los streams de
/// QUIC se pueden usar desde aquí aunque la conexión viva en el runtime principal.
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
                // Lo que quede corriendo se cancela al soltar el runtime, en este hilo.
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
        // No se espera al hilo: soltar el motor no puede bloquear un runtime de tokio.
        self.thread.take();
    }
}

/// Lo que comparten las tareas del motor.
struct Shared {
    agent: Arc<Agent>,
    me: Machine,
    backends: Arc<dyn InputBackends>,
    opts: InputOptions,
    lock_keys: Vec<HidUsage>,
    /// Adonde van los atajos de escena que se apretaron.
    scene_keys: mpsc::UnboundedSender<String>,
    /// Por dónde sale el cursor en vivo; los drivers solo publican con alguien mirando.
    cursor: CursorTap,
    link: watch::Receiver<Option<ActiveLink>>,
    /// Quién manda el teclado y el mouse según el arbitraje (D7).
    server: watch::Sender<Machine>,
    /// Última entrada física de esta máquina (la captura corre siempre).
    activity: watch::Sender<Option<Instant>>,
    /// Última latencia de la entrada, de la sesión de aquí que la tenga.
    latency: watch::Sender<Option<InputLatency>>,
    hub_here: watch::Sender<Option<bool>>,
    report: watch::Sender<Option<HubReport>>,
    /// Todas las pantallas de esta máquina (lo que se manda al otro agente).
    screens: watch::Sender<Vec<Screen>>,
    /// Las de esta máquina que se están mirando (las del cliente).
    visible: watch::Sender<Vec<Screen>>,
    layout: watch::Sender<Layout>,
    /// Sesiones establecidas, de servidor y de cliente.
    serving: watch::Sender<bool>,
    receiving: watch::Sender<bool>,
    /// El foco está en la otra máquina (servidor) / el cursor está aquí (cliente).
    remote: watch::Sender<bool>,
    here: watch::Sender<bool>,
    locked: watch::Sender<bool>,
    /// Hay un botón del mouse apretado (para vigilar el arrastre de archivos).
    held: watch::Sender<bool>,
    /// Hay un arrastre de archivos en curso: la sesión deja cruzar con el botón apretado.
    file_drag: Arc<AtomicBool>,
    missing: watch::Sender<Vec<InputPermission>>,
    capture_problem: watch::Sender<Option<String>>,
    inject_problem: watch::Sender<Option<String>>,
}

/// Arranca el motor de entrada sobre `link`, el enlace activo de la red. Tiene que
/// llamarse dentro de un runtime de tokio.
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

/// Lo del puesto que el motor toma al arrancar: el atajo de bloqueo y Cmd↔Ctrl (DESIGN
/// §4.4). La geometría no: esa se recalcula en caliente; el portapapeles tampoco: es un
/// módulo que se enciende y apaga solo ([`clipboard_module`]).
fn engine_prefs(desk: &tandem_core::Desk) -> (Vec<String>, bool) {
    (desk.shortcuts.lock.clone(), desk.prefs.cmd_ctrl)
}

/// El motor con el lado y las preferencias de ahora; si esta máquina cambia de lado o
/// cambian las preferencias del puesto (desde cualquiera de las dos UIs), se suelta entero
/// (la captura devuelve la entrada) y se arma de nuevo. Con el módulo «Teclado y mouse»
/// apagado no corre.
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
            // Sin motor nadie publica el estado: la sesión y el bloqueo ya no existen.
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
            tracing::info!(lado = %agent.machine(), "el motor de entrada vuelve a arrancar");
        } else {
            tracing::info!(
                encendido = !on,
                "teclado y mouse encendido o apagado por el usuario"
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
    // Lo declarado (configuración o asistente) ya viene con el lado de ahora.
    let keyboard = agent.keyboard_declared().unwrap_or(cfg.keyboard());
    let desk = agent.desk();
    // Un atajo con una tecla que no existe (de una versión más nueva) queda sin atajo.
    let lock_keys = crate::config::shortcut_keys(&desk.shortcuts.lock).unwrap_or_default();
    // Sin pantallas solo falla un margen de esquinas inválido.
    let initial = Layout::compute(&Arrangement::default(), setup.opts.layout);
    let initial = match initial {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("motor de entrada sin arrancar: {e}");
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
    // Captura, envío e inyección en su propio hilo; si no se puede crear, en el runtime
    // de siempre (funciona igual, solo comparte la cola).
    let hot = match HotPath::start() {
        Ok(h) => Some(h),
        Err(e) => {
            tracing::warn!("sin hilo propio para la entrada, se usa el runtime común: {e}");
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

/// Espera el próximo cambio. Si el emisor ya no existe, nunca termina: así un `select!`
/// no gira en vacío por un canal cerrado.
async fn changed<T>(rx: &mut watch::Receiver<T>) {
    if rx.changed().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Lleva al agente lo que publican los drivers del cursor en vivo (T-128): el agente lo
/// reparte a la UI y, si la otra máquina mira, a ella. Corre en el runtime común, no en el
/// del camino caliente.
async fn publish_cursor(agent: Arc<Agent>, mut events: mpsc::UnboundedReceiver<CursorEvent>) {
    while let Some(event) = events.recv().await {
        agent.publish_cursor(event);
    }
}

/// Reparte los streams que abre el otro agente.
async fn route(
    mut incoming: mpsc::UnboundedReceiver<IncomingChannel>,
    input: mpsc::UnboundedSender<IncomingChannel>,
    clipboard: mpsc::UnboundedSender<IncomingChannel>,
) {
    while let Some(inc) = incoming.recv().await {
        let to = match inc.channel {
            Channel::Input => &input,
            Channel::Clipboard => &clipboard,
            // Nunca llega aquí: lo atiende `peer::serve_stream` (los túneles).
            Channel::Tunnel => continue,
            // La prueba de ancho de banda la contesta `peer::serve_stream`; no llega aquí.
            Channel::Measure => continue,
        };
        if to.send(inc).is_err() {
            tracing::debug!("stream del otro agente sin quien lo atienda");
        }
    }
}

/// Lee las pantallas de esta máquina al arrancar y cada tanto.
async fn read_screens(sh: Arc<Shared>) {
    let mut tick = tokio::time::interval(sh.opts.screens_every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut warned = false;
    loop {
        tick.tick().await;
        let backends = sh.backends.clone();
        match tokio::task::spawn_blocking(move || backends.screens()).await {
            Ok(Ok(list)) => {
                warned = false;
                set(&sh.screens, list);
            }
            Ok(Err(e)) => {
                if !warned {
                    tracing::warn!("no se pudieron leer las pantallas de esta máquina: {e}");
                    warned = true;
                }
            }
            Err(e) => tracing::error!("lectura de pantallas: {e}"),
        }
    }
}

/// Recalcula la geometría con cada cambio de estado, del puesto, de pantallas o de lo que
/// muestra cada monitor.
async fn geometry(sh: Arc<Shared>) {
    let mut state = sh.agent.subscribe();
    let mut desk = sh.agent.subscribe_desk();
    let mut mine = sh.screens.subscribe();
    let mut theirs = sh.agent.inbox().screens.subscribe();
    let mut monitors = sh.agent.subscribe_monitors();
    loop {
        let st = state.borrow_and_update().clone();
        let d = desk.borrow_and_update().clone();
        let shows = crate::monitors::shows(&monitors.borrow_and_update(), &d);
        let local = mine.borrow_and_update().clone();
        let remote: Vec<Screen> = theirs
            .borrow_and_update()
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(screens::from_wire)
            .collect();
        match screens::layout(&d, &st, sh.me, (&local, &remote), &shows, sh.opts.layout) {
            Ok(l) => {
                set(&sh.visible, l.screens(sh.me).to_vec());
                set(&sh.layout, l);
            }
            Err(e) => tracing::warn!("geometría inválida, se sigue con la anterior: {e}"),
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

/// Quién manda el teclado y el mouse (D7): la entrada física de aquí, los mensajes del
/// otro agente y lo declarado alimentan el [`Follow`]; lo que devuelve se manda por el
/// enlace activo. Al conectar y cada tanto se manda el estado, por si un mensaje se perdió
/// con un enlace caído.
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
    // Lo que el usuario dijo en el asistente (T-087) manda sobre todo.
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
            // Lo que mandó por el enlace caído ya no vale: el próximo reporte llega por
            // el enlace siguiente.
            sh.agent.inbox().hub.send_replace(None);
        }
        current = now;
        if fresh {
            send(&current, Some(f.state()));
        }
        if *sh.server.borrow() != f.server() {
            tracing::info!(teclado = %f.server(), "el teclado y mouse pasó a otra máquina");
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
                // Otro motor tomó el buzón: este ya no recibe nada.
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

/// Le cuenta al otro agente el hub y las pantallas que lee aquí, cuando cambian y cada
/// tanto.
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

/// Mantiene al día los permisos que faltan y los pide al sistema dos veces como máximo sin
/// que el usuario lo ordene: una al arrancar, si falta alguno (macOS así registra el binario
/// que realmente corre y muestra su diálogo), y después solo por orden explícita de la UI
/// ([`UiCommand::RequestPermissions`]). Nunca en bucle: cada pedido es un diálogo.
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

/// Se sabe dónde está el teclado si lo dijo el usuario o la configuración, si el hub de
/// esta máquina se detecta o si la otra máquina reporta el suyo (T-087).
fn keyboard_defined(
    declared: Option<tandem_core::Machine>,
    hub_here: Option<bool>,
    peer_reports_hub: bool,
) -> bool {
    declared.is_some() || hub_here.is_some() || peer_reports_hub
}

/// Lo que ve la UI.
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
        // Lo que el usuario declara en el asistente (T-087).
        sh.agent.subscribe_keyboard(),
        sh.latency.subscribe(),
    );
    loop {
        let server = *rx.0.borrow_and_update();
        let is_server = server == sh.me;
        // Cada lectura de un `watch` se copia aquí, fuera del literal: dos lecturas del mismo
        // candado dentro de una expresión pueden bloquearse si entre ellas llega una
        // escritura (el candado da prioridad a quien escribe).
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
            // Las dos máquinas capturan e inyectan (D7): cuenta cualquiera de los dos,
            // primero el del papel de ahora.
            problem: {
                let capture = rx.6.borrow_and_update().clone();
                let inject = rx.7.borrow_and_update().clone();
                if is_server {
                    capture.or(inject)
                } else {
                    inject.or(capture)
                }
            },
            // El hub, la configuración o el asistente lo dicen; si no, es «sin definir».
            defined: keyboard_defined(declared, hub_here, peer_hub),
            latency: *rx.10.borrow_and_update(),
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
        }
    }
}

/// Una latencia de la entrada medida en este camino: la usa el guardián de los datos para
/// pausarlos si la entrada pasa su objetivo (DESIGN §6). Se alimenta en las dos máquinas,
/// porque los datos de las dos compiten por el mismo camino: en la que inyecta, con cada
/// muestra; en la que captura, con el p99 de cada ventana que le informa la otra.
/// Va al `Qos` del camino activo (T-101), el mismo para todas las conexiones por él.
fn feed_qos(sh: &Shared, latency: Duration) {
    let kind = sh.link.borrow().as_ref().map(|l| l.kind);
    if let Some(kind) = kind {
        sh.agent.link_qos().get(kind).record_input_latency(latency);
    }
}

/// Copia la latencia que publica un driver en la del motor, con el enlace activo.
/// `feed`: es lo que informa la otra máquina (el servidor no mide): su p99 alimenta el Qos.
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

/// Copia la entrada física que ve la captura en la del motor.
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

/// Copia un `watch` del driver en el del motor; al soltarse el driver queda en `false`.
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

/// El sistema de la otra máquina: lo que anunció o, sin anuncio todavía, lo que guarda el
/// puesto de ella.
fn peer_os(sh: &Shared) -> Option<Os> {
    sh.agent
        .capabilities()
        .peer
        .map(|p| p.info.os)
        .or_else(|| sh.agent.machine_info(sh.me.other()).map(|i| i.os))
}

async fn new_server(sh: &Arc<Shared>, peer: Option<Os>) -> Result<Server, String> {
    let (tx, rx) = mpsc::unbounded_channel();
    // Sin Cmd↔Ctrl en el puesto, ninguna reasignación (como entre dos sistemas iguales).
    let peer = peer.filter(|_| sh.agent.desk().prefs.cmd_ctrl);
    let sink = CaptureSink::new(
        tx,
        &sh.agent.config().input.remap,
        sh.agent.config().os,
        peer,
    )
    .with_scenes(sh.agent.scene_shortcuts(sh.scene_keys.clone()));
    // La sesión ve las teclas ya reasignadas: el atajo, escrito con las físicas, también.
    let lock_keys = sh.lock_keys.iter().map(|&k| sink.remap_key(k)).collect();
    let backends = sh.backends.clone();
    let capture = tokio::task::spawn_blocking(move || backends.capture(sink))
        .await
        .map_err(|e| e.to_string())??;
    // Suscribirse antes de leer: si la geometría cambia entre medio (las pantallas de la
    // otra llegan junto con sus capacidades, justo cuando se rearma la captura), la sesión
    // la recibe; leer primero y suscribirse después dejaba el cambio marcado como visto y
    // la sesión sin la otra máquina hasta el próximo cambio.
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

/// La sesión de servidor, mientras esta máquina lo sea: sobre el enlace activo, o
/// consumiendo lo capturado si no hay enlace.
async fn server(sh: Arc<Shared>) {
    let mut role = sh.server.subscribe();
    let mut link = sh.link.clone();
    let mut caps = sh.agent.subscribe_capabilities();
    let mut driver: Option<Server> = None;
    // El sistema de la otra con que se armó la captura (los modificadores dependen de él).
    let mut built_for: Option<Os> = None;
    loop {
        caps.borrow_and_update();
        let peer = peer_os(&sh);
        if driver.is_some() && built_for != peer {
            tracing::info!(otra = ?peer, "la otra máquina cambió de sistema: se rearma la captura");
            driver = None;
        }
        // Las dos máquinas capturan siempre (D7): la que no manda solo se pone al día y
        // avisa la entrada física, que es lo que decide quién manda.
        let d = match driver.as_mut() {
            Some(d) => d,
            None => match new_server(&sh, peer).await {
                Ok(d) => {
                    set(&sh.capture_problem, None);
                    built_for = peer;
                    driver.insert(d)
                }
                Err(e) => {
                    tracing::warn!("no se pudo arrancar la captura: {e}");
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
            // Sin saber el sistema de la otra no se abre la sesión: al llegar sus
            // capacidades se rearmaría la captura (los modificadores dependen de él) y la
            // sesión nueva empezaría con el cursor en casa, perdiendo un cruce ya hecho.
            // Lo de aquí se sigue poniendo al día.
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
                // Antes de reintentar, lo capturado se consume sin cruzar.
                let pause = tokio::time::sleep(sh.opts.retry);
                if d.idle(pause).await.is_err() {
                    driver = None;
                }
            }
        }
    }
}

/// Termina cuando el sistema que anuncia la otra máquina deja de ser `current`.
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

/// Termina cuando el enlace activo ya no es `current` o esta máquina deja de ser `me`
/// como servidor.
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

/// La sesión de cliente: inyecta lo que llega por cada stream de entrada que abre el
/// servidor. Un stream nuevo reemplaza al anterior (el servidor cambió de enlace).
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
                    tracing::warn!("no se pudo arrancar la inyección: {e}");
                    set(&sh.inject_problem, Some(e));
                    // El stream se cierra: el servidor reintenta y para entonces quizá
                    // ya se pueda.
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
        tracing::debug!("sesión de entrada (recibe el teclado): {end}");
        next = match following {
            Some(f) => Some(f),
            None => streams.recv().await,
        };
    }
}

/// El foco del teclado y mouse está en esta máquina.
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

/// El módulo «Portapapeles»: corre mientras el usuario lo tenga encendido. Apagado, lo que
/// abra el otro agente se cierra enseguida.
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

/// Aborta la tarea al soltarse: el portapapeles se puede apagar a mitad de camino.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// El portapapeles: el lado A abre el stream en el enlace activo y B lo recibe (da igual
/// cuál; solo tiene que ser uno, y los dos saben cuál sin hablarse).
async fn clipboard(sh: Arc<Shared>, streams: &mut mpsc::UnboundedReceiver<IncomingChannel>) {
    let backends = sh.backends.clone();
    let cb = match tokio::task::spawn_blocking(move || backends.clipboard()).await {
        Ok(Ok(cb)) => {
            sh.agent.set_module_problem(ModuleId::Clipboard, None);
            cb
        }
        Ok(Err(e)) => {
            tracing::warn!("sin portapapeles compartido: {e}");
            sh.agent.set_module_problem(
                ModuleId::Clipboard,
                Some(format!("No se pudo abrir el portapapeles del sistema: {e}")),
            );
            // Lo que abra el otro agente se cierra enseguida.
            while streams.recv().await.is_some() {}
            return;
        }
        Err(e) => {
            tracing::error!("portapapeles: {e}");
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
                    tracing::debug!("no se pudo abrir el stream del portapapeles: {e}");
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
        assert!(!f(None, None, false), "sin nada, sin definir");
        assert!(f(Some(Machine::B), None, false));
        assert!(f(None, Some(false), false), "el hub de aquí se detecta");
        assert!(
            f(None, None, true),
            "el hub de la otra máquina también cuenta"
        );
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
        assert!(r.is_err(), "un canal cerrado no despierta a nadie");
    }
}
