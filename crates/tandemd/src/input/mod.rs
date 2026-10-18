//! Teclado, mouse y portapapeles compartidos en el agente (DESIGN §7): junta
//! `tandem-input`, `tandem-clipboard` y `tandem-peripherals` con la red.
//!
//! - **Roles**: el [`RoleArbiter`] decide quién es el servidor con el hub de aquí
//!   ([`HubWatcher`]) y el [`ControlMsg::Hub`] del otro agente. Sin enlace, la Mac con
//!   la PC dada por apagada decide sola (T-066).
//! - **Geometría**: las pantallas de esta máquina (de los backends, otra vez con cada
//!   `UpdateInputGeometry`), las que manda el otro agente ([`ControlMsg::Screens`]) y el
//!   [`State`](tandem_core::State) arman el [`Layout`]; se recalcula en caliente.
//! - **Sesión**: el servidor abre un stream propio sobre el enlace activo
//!   ([`Channel::Input`]) y corre su [`ServerDriver`]; el cliente recibe ese stream del
//!   otro agente y corre su [`ClientDriver`]. Cuando el enlace cambia la sesión termina
//!   y vuelve a empezar sobre el nuevo.
//! - **Portapapeles**: la Mac abre su stream ([`Channel::Clipboard`]) y la PC lo
//!   recibe; los dos corren un [`ClipDriver`] con el foco del teclado y mouse.
//! - **Estado para la UI**: [`InputStatus`] (servidor, hub, sesión, cursor bloqueado
//!   y permisos de macOS que faltan).
//!
//! Los backends de plataforma van detrás de [`InputBackends`]; [`fake`] tiene los
//! dobles para las pruebas con dos agentes.

pub mod backends;
pub mod fake;
pub mod screens;

use std::sync::Arc;
use std::time::Duration;

use tandem_clipboard::{ClipConfig, ClipDriver};
use tandem_core::{Machine, Step};
use tandem_input::geometry::{Layout, LayoutOptions};
use tandem_input::session::{
    ClientDriver, ClientSession, ServerDriver, ServerSession, SessionConfig, SessionEnd,
};
use tandem_input::{HidUsage, Point, Screen};
use tandem_peripherals::{HubWatcher, PeerReport, Presence, RoleArbiter, WatchConfig};
use tandem_proto::{Channel, ControlMsg, HubReport, InputPermission, InputStatus};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::agent::{Agent, IncomingChannel};
use crate::network::ActiveLink;
use crate::orchestrator::{LocalSteps, StepFuture, describe};
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
    /// Cada cuánto se repiten el hub y las pantallas al otro agente, por si un aviso
    /// se perdió en un cambio de enlace.
    pub report_every: Duration,
    /// Cada cuánto se vuelven a leer las pantallas aunque no haya cambio de modo (un
    /// monitor que se enchufa).
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
}

impl InputSetup {
    /// Los de este sistema, con los tiempos normales.
    pub fn platform(me: Machine) -> Self {
        InputSetup {
            backends: Arc::new(PlatformInput::new(me)),
            opts: InputOptions::default(),
        }
    }
}

/// El paso `UpdateInputGeometry`: le avisa al motor de entrada que vuelva a leer las
/// pantallas; la geometría nueva sale de ahí y del estado final del cambio de modo.
/// Cualquier otro paso que llegue aquí no le corresponde.
pub struct GeometrySteps {
    geometry: Arc<watch::Sender<u64>>,
}

impl GeometrySteps {
    pub fn new(geometry: Arc<watch::Sender<u64>>) -> Self {
        GeometrySteps { geometry }
    }
}

impl LocalSteps for GeometrySteps {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
        Box::pin(async move {
            match step {
                Step::UpdateInputGeometry => {
                    self.geometry.send_modify(|n| *n = n.wrapping_add(1));
                    Ok(())
                }
                other => Err(format!(
                    "{}: {}",
                    describe(other),
                    crate::orchestrator::STEP_NOT_IMPLEMENTED
                )),
            }
        })
    }
}

/// El motor de entrada en marcha. Al soltarlo se detiene todo y la captura devuelve
/// el teclado y el mouse a esta máquina.
pub struct Input {
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Input {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// Lo que comparten las tareas del motor.
struct Shared {
    agent: Arc<Agent>,
    me: Machine,
    backends: Arc<dyn InputBackends>,
    opts: InputOptions,
    lock_keys: Vec<HidUsage>,
    link: watch::Receiver<Option<ActiveLink>>,
    /// El servidor según el árbitro.
    server: watch::Sender<Machine>,
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
    let cfg = &agent.config().input;
    agent.set_input_engine();
    let me = agent.machine();
    // La config ya se validó al cargarla.
    let lock_keys = cfg.lock_keys().unwrap_or_default();
    // Sin pantallas solo falla un margen de esquinas inválido.
    let initial = Layout::compute(
        &agent.config().desk,
        &agent.state(),
        &[],
        &[],
        setup.opts.layout,
    );
    let initial = match initial {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("motor de entrada sin arrancar: {e}");
            return Input { tasks: Vec::new() };
        }
    };
    let sh = Arc::new(Shared {
        me,
        backends: setup.backends,
        opts: setup.opts,
        lock_keys,
        link,
        server: watch::Sender::new(cfg.keyboard()),
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
        missing: watch::Sender::new(Vec::new()),
        capture_problem: watch::Sender::new(None),
        inject_problem: watch::Sender::new(None),
        agent,
    });

    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (clip_tx, clip_rx) = mpsc::unbounded_channel();
    let (incoming_tx, incoming) = mpsc::unbounded_channel();
    sh.agent.inbox().set_channels(incoming_tx);

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
        tokio::spawn(roles(sh.clone(), presence)),
        tokio::spawn(report(sh.clone())),
        tokio::spawn(permissions(sh.clone())),
        tokio::spawn(status(sh.clone())),
        tokio::spawn(server(sh.clone())),
        tokio::spawn(client(sh.clone(), input_rx)),
    ]);
    if sh.agent.config().input.clipboard {
        tasks.push(tokio::spawn(clipboard(sh.clone(), clip_rx)));
    }
    Input { tasks }
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
        };
        if to.send(inc).is_err() {
            tracing::debug!("stream del otro agente sin quien lo atienda");
        }
    }
}

/// Lee las pantallas de esta máquina al arrancar, con cada `UpdateInputGeometry` y
/// cada tanto.
async fn read_screens(sh: Arc<Shared>) {
    let mut geometry = sh.agent.subscribe_geometry();
    let mut tick = tokio::time::interval(sh.opts.screens_every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut warned = false;
    loop {
        let (backends, desk) = (sh.backends.clone(), sh.agent.config().desk.clone());
        match tokio::task::spawn_blocking(move || backends.screens(&desk)).await {
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
        tokio::select! {
            () = changed(&mut geometry) => {}
            _ = tick.tick() => {}
        }
    }
}

/// Recalcula la geometría con cada cambio de estado o de pantallas.
async fn geometry(sh: Arc<Shared>) {
    let mut state = sh.agent.subscribe();
    let mut mine = sh.screens.subscribe();
    let mut theirs = sh.agent.inbox().screens.subscribe();
    loop {
        let st = state.borrow_and_update().clone();
        let local = mine.borrow_and_update().clone();
        let remote: Vec<Screen> = theirs
            .borrow_and_update()
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(screens::from_wire)
            .collect();
        match screens::layout(
            &sh.agent.config().desk,
            &st,
            sh.me,
            &local,
            &remote,
            sh.opts.layout,
        ) {
            Ok(l) => {
                set(&sh.visible, l.screens(sh.me).to_vec());
                set(&sh.layout, l);
            }
            Err(e) => tracing::warn!("geometría inválida, se sigue con la anterior: {e}"),
        }
        tokio::select! {
            () = changed(&mut state) => {}
            () = changed(&mut mine) => {}
            () = changed(&mut theirs) => {}
        }
    }
}

/// Quién es el servidor.
async fn roles(sh: Arc<Shared>, mut presence: Option<watch::Receiver<Presence>>) {
    let mut arbiter = RoleArbiter::new(sh.me, sh.agent.config().input.keyboard());
    let mut peer = sh.agent.inbox().hub.subscribe();
    let mut link = sh.link.clone();
    let mut state = sh.agent.subscribe();
    // Lo que el usuario dijo en el asistente (T-087).
    let mut declared = sh.agent.subscribe_keyboard();
    let mut last_declared = *declared.borrow_and_update();
    loop {
        let now = *declared.borrow_and_update();
        if now != last_declared {
            last_declared = now;
            if let Some(machine) = now {
                arbiter.declare(machine);
            }
        }
        if let Some(p) = presence.as_mut() {
            arbiter.set_local_hub(p.borrow_and_update().hub());
        }
        let linked = link.borrow_and_update().is_some();
        let report = *peer.borrow_and_update();
        let peer_on = state.borrow_and_update().is_on(sh.me.other());
        if linked {
            if let Some(r) = report {
                arbiter.set_peer(Some(PeerReport {
                    hub: r.hub,
                    server: r.server,
                }));
            }
        } else {
            // Lo que mandó por el enlace caído ya no vale: el próximo reporte llega
            // por el enlace siguiente.
            if report.is_some() {
                sh.agent.inbox().hub.send_replace(None);
            }
            if !peer_on {
                arbiter.set_peer_off();
            } else {
                arbiter.set_peer(None);
            }
        }
        if *sh.server.borrow() != arbiter.server() {
            tracing::info!(teclado = %arbiter.server(), "el teclado y mouse pasó a otra máquina");
        }
        set(&sh.server, arbiter.server());
        set(&sh.hub_here, arbiter.local_hub());
        set(
            &sh.report,
            arbiter.report().map(|r| HubReport {
                hub: r.hub,
                server: r.server,
            }),
        );
        tokio::select! {
            () = async {
                match presence.as_mut() {
                    Some(p) => changed(p).await,
                    None => std::future::pending().await,
                }
            } => {}
            () = changed(&mut peer) => {}
            () = changed(&mut link) => {}
            () = changed(&mut state) => {}
            () = changed(&mut declared) => {}
        }
    }
}

/// Le cuenta al otro agente el hub y las pantallas de aquí, cuando cambian y cada tanto.
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

async fn permissions(sh: Arc<Shared>) {
    loop {
        set(&sh.missing, sh.backends.missing_permissions());
        tokio::time::sleep(sh.opts.permissions_every).await;
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
            problem: if is_server {
                rx.6.borrow_and_update().clone()
            } else {
                rx.7.borrow_and_update().clone()
            },
            // El hub, la configuración o el asistente lo dicen; si no, es «sin definir».
            defined: keyboard_defined(declared, hub_here, peer_hub),
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
        }
    }
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

async fn new_server(sh: &Arc<Shared>) -> Result<Server, String> {
    let (tx, rx) = mpsc::unbounded_channel();
    let sink = CaptureSink::new(tx, &sh.agent.config().input.remap, sh.me);
    // La sesión ve las teclas ya reasignadas: el atajo, escrito con las físicas, también.
    let lock_keys = sh.lock_keys.iter().map(|&k| sink.remap_key(k)).collect();
    let backends = sh.backends.clone();
    let capture = tokio::task::spawn_blocking(move || backends.capture(sink))
        .await
        .map_err(|e| e.to_string())??;
    let layout = sh.layout.borrow().clone();
    let cursor = first_cursor(layout.screens(sh.me));
    let session = ServerSession::new(sh.me, layout, cursor, sh.opts.session.resync_every)
        .with_lock_shortcut(lock_keys);
    let driver = ServerDriver::new(session, capture, rx, sh.opts.session)
        .with_layouts(sh.layout.subscribe());
    let s = sh.clone();
    forward(driver.active(), move |v| set(&s.serving, v));
    let s = sh.clone();
    forward(driver.remote(), move |v| set(&s.remote, v));
    let s = sh.clone();
    forward(driver.locked(), move |v| set(&s.locked, v));
    Ok(driver)
}

/// La sesión de servidor, mientras esta máquina lo sea: sobre el enlace activo, o
/// consumiendo lo capturado si no hay enlace.
async fn server(sh: Arc<Shared>) {
    let mut role = sh.server.subscribe();
    let mut link = sh.link.clone();
    let mut driver: Option<Server> = None;
    loop {
        let is_server = *role.borrow_and_update() == sh.me;
        if !is_server {
            match driver.as_mut() {
                // La captura sigue: lo de aquí se pone al día sin cruzar.
                Some(d) => {
                    if d.idle(changed(&mut role)).await.is_err() {
                        driver = None;
                    }
                }
                None => changed(&mut role).await,
            }
            continue;
        }
        let d = match driver.as_mut() {
            Some(d) => d,
            None => match new_server(&sh).await {
                Ok(d) => {
                    set(&sh.capture_problem, None);
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
        let me = sh.me;
        let conn = active.handle.conn.clone();
        let open = async move {
            open_channel(&conn, me, Channel::Input)
                .await
                .map_err(|e| SessionEnd::Open(e.to_string()))
        };
        let stop = until_link_or_role_changes(&mut link, &active, &mut role, me);
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
    let driver =
        ClientDriver::new(session, injector, sh.opts.session).with_screens(sh.visible.subscribe());
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

/// El portapapeles: la Mac abre el stream en el enlace activo y la PC lo recibe.
async fn clipboard(sh: Arc<Shared>, mut streams: mpsc::UnboundedReceiver<IncomingChannel>) {
    let backends = sh.backends.clone();
    let cb = match tokio::task::spawn_blocking(move || backends.clipboard()).await {
        Ok(Ok(cb)) => cb,
        Ok(Err(e)) => {
            tracing::warn!("sin portapapeles compartido: {e}");
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
    let tracker = tokio::spawn(track_focus(sh.clone(), focus_tx));
    let focused = *focus_rx.borrow();
    let mut driver = ClipDriver::new(cb, sh.opts.clipboard, focused).with_focus(focus_rx);
    if sh.me == Machine::Mac {
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
    tracker.abort();
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_keyboard_is_defined_by_the_user_or_any_hub_report() {
        use super::keyboard_defined as f;
        use tandem_core::Machine;
        assert!(!f(None, None, false), "sin nada, sin definir");
        assert!(f(Some(Machine::Pc), None, false));
        assert!(f(None, Some(false), false), "el hub de aquí se detecta");
        assert!(
            f(None, None, true),
            "el hub de la otra máquina también cuenta"
        );
    }

    use super::*;

    #[tokio::test]
    async fn geometry_step_only_bumps_the_counter() {
        let tx = Arc::new(watch::Sender::new(0));
        let mut rx = tx.subscribe();
        let steps = GeometrySteps::new(tx);
        steps.execute(&Step::UpdateInputGeometry).await.unwrap();
        steps.execute(&Step::UpdateInputGeometry).await.unwrap();
        assert_eq!(*rx.borrow_and_update(), 2);
        let e = steps
            .execute(&Step::PowerOn {
                machine: Machine::Pc,
            })
            .await
            .unwrap_err();
        assert!(e.contains(crate::orchestrator::STEP_NOT_IMPLEMENTED), "{e}");
    }

    #[test]
    fn the_cursor_starts_in_the_middle_of_the_first_screen() {
        use tandem_input::geometry::{Rect, ScreenId};
        assert_eq!(first_cursor(&[]), Point::new(0.0, 0.0));
        let s = Screen::new(ScreenId::Builtin, Rect::new(-100, 0, 200, 100), 1.0);
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
