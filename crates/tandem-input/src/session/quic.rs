//! Drivers: conectan las máquinas de estados con una conexión de `tandem-link`.
//!
//! Cada `run` dura lo que dura un enlace. El servidor abre un stream propio sobre
//! la conexión del enlace activo (con su intercambio de `Hello`) y el cliente lo
//! acepta; los datagramas van por la misma conexión. Termina cuando el enlace se
//! cae, cuando el otro lado calla más de `heartbeat_timeout` o cuando `stop` se
//! cumple (el gestor de enlaces avisó de un cambio); en todos los casos el
//! servidor devuelve el foco a su máquina y el cliente suelta todo.
//!
//! Medición (T-093): el servidor marca cada evento al sacarlo de la captura; los latidos
//! llevan marca y eco para estimar el desfase de relojes; el cliente mide al inyectar y
//! cada [`SessionConfig::latency_every`] publica p50/p99/máx, lo registra y se lo manda
//! al servidor, que también lo publica.

use std::future::Future;
use std::time::Duration;

use bytes::Bytes;
use tandem_link::transport::{Connection, ControlStream, TransportError};
use tandem_proto::{Hello, InputLatency};
use tokio::sync::{mpsc, watch};
use tokio::time::{self, Instant, MissedTickBehavior};

use super::latency::{Beats, Captured, Histogram, now_us};
use super::msg::{Datagram, StreamMsg};
use super::{Capture, ClientSession, Injector, ServerOutput, ServerSession, SessionConfig};
use crate::event::InputEvent;
use crate::geometry::{Layout, Screen};

/// Por qué terminó una sesión.
#[derive(Debug, thiserror::Error)]
pub enum SessionEnd {
    #[error("detenida: el enlace cambió")]
    Stopped,
    #[error("el otro lado dejó de dar señales")]
    Silent,
    #[error("enlace: {0}")]
    Link(#[from] TransportError),
    #[error("la captura se cerró")]
    CaptureClosed,
    #[error("no se pudo abrir el stream de entrada: {0}")]
    Open(String),
}

/// Servidor: la captura de esta máquina y su máquina de estados.
pub struct ServerDriver<C> {
    session: ServerSession,
    capture: C,
    events: mpsc::UnboundedReceiver<Captured>,
    layouts: Option<watch::Receiver<Layout>>,
    active: watch::Sender<bool>,
    remote: watch::Sender<bool>,
    locked: watch::Sender<bool>,
    activity: watch::Sender<Option<Instant>>,
    latency: watch::Sender<Option<InputLatency>>,
    cfg: SessionConfig,
}

/// Cada cuánto como máximo se avisa la entrada física: el arbitraje (D7) trabaja en
/// decenas de milisegundos y así el camino caliente no despierta a nadie por evento.
const ACTIVITY_EVERY: Duration = Duration::from_millis(5);

fn touch(activity: &watch::Sender<Option<Instant>>) {
    let now = Instant::now();
    activity.send_if_modified(|last| {
        let due = last.is_none_or(|t| now.saturating_duration_since(t) >= ACTIVITY_EVERY);
        if due {
            *last = Some(now);
        }
        due
    });
}

impl<C: Capture> ServerDriver<C> {
    /// `events` es por donde la captura manda lo que captura; se conserva entre
    /// sesiones (un enlace nuevo sigue con la misma captura). Lo que llega sin
    /// sesión solo pone al día lo presionado y el cursor local
    /// ([`ServerSession::catch_up`]); mientras no hay enlace, [`Self::idle`] lo
    /// consume para que no se acumule.
    pub fn new(
        session: ServerSession,
        capture: C,
        events: mpsc::UnboundedReceiver<Captured>,
        cfg: SessionConfig,
    ) -> Self {
        Self {
            session,
            capture,
            events,
            layouts: None,
            active: watch::Sender::new(false),
            remote: watch::Sender::new(false),
            locked: watch::Sender::new(false),
            activity: watch::Sender::new(None),
            latency: watch::Sender::new(None),
            cfg,
        }
    }

    /// Cuándo llegó la última entrada física de esta máquina (con o sin sesión): la
    /// captura solo ve lo físico, lo inyectado no le llega.
    pub fn activity(&self) -> watch::Receiver<Option<Instant>> {
        self.activity.subscribe()
    }

    /// La latencia que informa el cliente cada [`SessionConfig::latency_every`].
    pub fn latency(&self) -> watch::Receiver<Option<InputLatency>> {
        self.latency.subscribe()
    }

    /// `true` mientras el foco está en la otra máquina.
    pub fn remote(&self) -> watch::Receiver<bool> {
        self.remote.subscribe()
    }

    /// `true` mientras el cursor está bloqueado en su máquina ([`ServerSession::is_locked`]).
    pub fn locked(&self) -> watch::Receiver<bool> {
        self.locked.subscribe()
    }

    /// Pone al día [`Self::remote`] y [`Self::locked`] con la sesión.
    fn publish(&self) {
        publish(&self.remote, self.session.is_remote());
        publish(&self.locked, self.session.is_locked());
    }

    /// `true` desde que la sesión está establecida (stream abierto y lo viejo
    /// descartado) hasta que termina: lo capturado desde ahí sí cuenta.
    pub fn active(&self) -> watch::Receiver<bool> {
        self.active.subscribe()
    }

    /// Geometría en caliente: cada valor nuevo se aplica al momento.
    pub fn with_layouts(mut self, layouts: watch::Receiver<Layout>) -> Self {
        self.layouts = Some(layouts);
        self
    }

    pub fn session(&self) -> &ServerSession {
        &self.session
    }

    pub fn capture(&self) -> &C {
        &self.capture
    }

    /// Sin enlace: consume lo capturado hasta que `until` se cumpla, sin cruzar
    /// ni acumular nada. Devuelve `Err` si la captura se cerró.
    pub async fn idle(&mut self, until: impl Future<Output = ()>) -> Result<(), SessionEnd> {
        tokio::pin!(until);
        loop {
            tokio::select! {
                biased;
                () = &mut until => return Ok(()),
                ev = self.events.recv() => match ev {
                    Some(ev) => {
                        touch(&self.activity);
                        self.session.catch_up(ev.event);
                        self.publish();
                    }
                    None => return Err(SessionEnd::CaptureClosed),
                },
            }
        }
    }

    /// Una sesión sobre `conn`: abre el stream de entrada con el intercambio de
    /// [`Hello`] y lo atiende hasta que termine.
    pub async fn run(
        &mut self,
        conn: &Connection,
        hello: &Hello,
        stop: impl Future<Output = ()>,
    ) -> SessionEnd {
        let open = async {
            conn.open_control(hello)
                .await
                .map(|(stream, _)| stream)
                .map_err(SessionEnd::from)
        };
        self.run_with(conn, open, stop).await
    }

    /// Como [`Self::run`], con el stream abierto por `open` (el agente lo abre con su
    /// propio saludo, para distinguirlo de los demás streams de la conexión).
    /// `open` tiene [`SessionConfig::heartbeat_timeout`] para terminar.
    pub async fn run_with(
        &mut self,
        conn: &Connection,
        open: impl Future<Output = Result<ControlStream, SessionEnd>>,
        stop: impl Future<Output = ()>,
    ) -> SessionEnd {
        let end = self.serve(conn, open, stop).await;
        self.active.send_replace(false);
        for out in self.session.link_lost() {
            apply_local(&mut self.capture, out);
        }
        self.publish();
        tracing::info!("sesión de entrada (con el teclado) terminada: {end}");
        end
    }

    async fn serve(
        &mut self,
        conn: &Connection,
        open: impl Future<Output = Result<ControlStream, SessionEnd>>,
        stop: impl Future<Output = ()>,
    ) -> SessionEnd {
        let Self {
            session,
            capture,
            events,
            layouts,
            active,
            remote,
            locked,
            activity,
            latency,
            cfg,
        } = self;
        tokio::pin!(stop);
        // Lo capturado sin sesión no se reproduce: haría cruzar solo.
        catch_up(session, events, activity);
        let mut stream = tokio::select! {
            () = &mut stop => return SessionEnd::Stopped,
            r = time::timeout(cfg.heartbeat_timeout, open) => match r {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) => return e,
                Err(_) => return SessionEnd::Silent,
            },
        };
        catch_up(session, events, activity);
        active.send_replace(true);
        let sync = |session: &ServerSession| {
            publish(remote, session.is_remote());
            publish(locked, session.is_locked());
        };
        sync(session);

        let mut beats = Beats::new();
        let mut heartbeat = time::interval(cfg.heartbeat_interval);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let silence = time::sleep(cfg.heartbeat_timeout);
        let resync = time::sleep(cfg.resync_idle);
        tokio::pin!(silence, resync);

        loop {
            tokio::select! {
                biased;
                () = &mut stop => return SessionEnd::Stopped,
                ev = events.recv() => {
                    let Some(ev) = ev else { return SessionEnd::CaptureClosed };
                    touch(activity);
                    // Se manda enseguida; solo se junta lo que llegó mientras se
                    // mandaba lo anterior (está en el canal): así se coalesce el
                    // movimiento sin esperar a nadie. Cada evento lleva la marca de cuando
                    // se capturó: la espera en el canal cuenta en la latencia.
                    let mut outs = session.handle_at(ev.event, ev.t_us);
                    while let Ok(ev) = events.try_recv() {
                        outs.extend(session.handle_at(ev.event, ev.t_us));
                    }
                    outs.extend(session.flush().map(ServerOutput::Datagram));
                    let sent = apply(capture, &mut stream, conn, outs).await;
                    sync(session);
                    if let Err(e) = sent {
                        return e.into();
                    }
                    resync.as_mut().reset(Instant::now() + cfg.resync_idle);
                }
                msg = stream.recv::<StreamMsg>() => match msg {
                    // Del cliente llegan latidos y la latencia que mide.
                    Ok(Some(msg)) => {
                        silence.as_mut().reset(Instant::now() + cfg.heartbeat_timeout);
                        match msg {
                            StreamMsg::Heartbeat(b) => beats.incoming(b, now_us()),
                            StreamMsg::Latency(l) => {
                                tracing::info!(
                                    n = l.count,
                                    p50_us = l.p50_us,
                                    p99_us = l.p99_us,
                                    max_us = l.max_us,
                                    "latencia de la entrada informada por la otra máquina"
                                );
                                latency.send_replace(Some(l));
                            }
                            _ => {}
                        }
                    }
                    Ok(None) => return TransportError::ControlClosed.into(),
                    Err(e) => return e.into(),
                },
                _ = heartbeat.tick() => {
                    let beat = StreamMsg::Heartbeat(beats.outgoing(now_us()));
                    if let Err(e) = stream.send(&beat).await {
                        return e.into();
                    }
                }
                () = &mut silence => return SessionEnd::Silent,
                () = &mut resync, if session.needs_resync() => {
                    let outs = session.resync();
                    if let Err(e) = apply(capture, &mut stream, conn, outs).await {
                        return e.into();
                    }
                }
                layout = next_layout(layouts) => match layout {
                    Some(l) => {
                        let outs = session.set_layout(l);
                        let sent = apply(capture, &mut stream, conn, outs).await;
                        sync(session);
                        if let Err(e) = sent {
                            return e.into();
                        }
                    }
                    // Nadie más publica geometría: se sigue con la última.
                    None => *layouts = None,
                },
            }
        }
    }
}

fn publish(tx: &watch::Sender<bool>, value: bool) {
    tx.send_if_modified(|v| {
        let changed = *v != value;
        *v = value;
        changed
    });
}

/// Vacía lo que ya está en el canal sin esperar.
fn catch_up(
    session: &mut ServerSession,
    events: &mut mpsc::UnboundedReceiver<Captured>,
    activity: &watch::Sender<Option<Instant>>,
) {
    while let Ok(ev) = events.try_recv() {
        touch(activity);
        session.catch_up(ev.event);
    }
}

async fn next_layout(layouts: &mut Option<watch::Receiver<Layout>>) -> Option<Layout> {
    next_value(layouts).await
}

/// El próximo valor de un `watch` opcional; `None` si se cerró, y sin receptor nunca.
async fn next_value<T: Clone>(rx: &mut Option<watch::Receiver<T>>) -> Option<T> {
    match rx {
        Some(rx) => match rx.changed().await {
            Ok(()) => Some(rx.borrow_and_update().clone()),
            Err(_) => None,
        },
        None => std::future::pending().await,
    }
}

async fn apply<C: Capture>(
    capture: &mut C,
    stream: &mut ControlStream,
    conn: &Connection,
    outs: Vec<ServerOutput>,
) -> Result<(), TransportError> {
    for out in outs {
        match out {
            ServerOutput::Stream(msg) => stream.send(&msg).await?,
            ServerOutput::Datagram(d) => {
                let bytes = Datagram::Motion(d).encode()?;
                conn.send_datagram(Bytes::from(bytes))?;
            }
            local => apply_local(capture, local),
        }
    }
    Ok(())
}

/// Un fallo de la captura no corta la sesión: se registra y se sigue, para no
/// dejar al usuario sin teclado por un error del sistema.
fn apply_local<C: Capture>(capture: &mut C, out: ServerOutput) {
    let result = match out {
        ServerOutput::Grab(grab) => capture.set_grab(grab),
        ServerOutput::Warp(pos) => capture.warp(pos),
        ServerOutput::Stream(_) | ServerOutput::Datagram(_) => return,
    };
    if let Err(e) = result {
        tracing::error!("captura: {e}");
    }
}

/// Cliente: la inyección de esta máquina y su máquina de estados.
pub struct ClientDriver<I> {
    session: ClientSession,
    injector: I,
    here: watch::Sender<bool>,
    screens: Option<watch::Receiver<Vec<Screen>>>,
    latency: watch::Sender<Option<InputLatency>>,
    /// Cada latencia medida, además de la ventana (el guardián de los datos, T-101).
    on_sample: Option<Box<dyn FnMut(Duration) + Send>>,
    cfg: SessionConfig,
}

impl<I: Injector> ClientDriver<I> {
    pub fn new(session: ClientSession, injector: I, cfg: SessionConfig) -> Self {
        Self {
            session,
            injector,
            here: watch::Sender::new(false),
            screens: None,
            latency: watch::Sender::new(None),
            on_sample: None,
            cfg,
        }
    }

    /// La latencia medida aquí en la última ventana ([`SessionConfig::latency_every`]).
    pub fn latency(&self) -> watch::Receiver<Option<InputLatency>> {
        self.latency.subscribe()
    }

    /// Llama a `f` con cada latencia medida (captura → inyección).
    pub fn on_latency_sample(mut self, f: impl FnMut(Duration) + Send + 'static) -> Self {
        self.on_sample = Some(Box::new(f));
        self
    }

    /// Las pantallas de esta máquina en caliente: cada valor nuevo pasa a la sesión y
    /// el inyector las vuelve a leer.
    pub fn with_screens(mut self, mut screens: watch::Receiver<Vec<Screen>>) -> Self {
        self.session
            .set_screens(screens.borrow_and_update().clone());
        self.screens = Some(screens);
        self
    }

    /// `true` mientras el cursor está en esta máquina.
    pub fn here(&self) -> watch::Receiver<bool> {
        self.here.subscribe()
    }

    pub fn session(&self) -> &ClientSession {
        &self.session
    }

    pub fn session_mut(&mut self) -> &mut ClientSession {
        &mut self.session
    }

    pub fn injector(&self) -> &I {
        &self.injector
    }

    /// Una sesión sobre `conn`: acepta el stream de entrada que abre el servidor.
    pub async fn run(
        &mut self,
        conn: &Connection,
        hello: &Hello,
        stop: impl Future<Output = ()>,
    ) -> SessionEnd {
        let accept = async {
            conn.accept_control(hello)
                .await
                .map(|(stream, _)| stream)
                .map_err(SessionEnd::from)
        };
        self.run_with(conn, accept, stop).await
    }

    /// Como [`Self::run`], con el stream que entrega `accept` (el agente acepta los
    /// streams de la conexión y le pasa este). `accept` tiene
    /// [`SessionConfig::heartbeat_timeout`] para terminar.
    pub async fn run_with(
        &mut self,
        conn: &Connection,
        accept: impl Future<Output = Result<ControlStream, SessionEnd>>,
        stop: impl Future<Output = ()>,
    ) -> SessionEnd {
        let end = self.serve(conn, accept, stop).await;
        // Pase lo que pase con el enlace, nada queda presionado.
        let release = self.session.link_lost();
        inject(&mut self.injector, release);
        publish(&self.here, false);
        tracing::info!("sesión de entrada (sin el teclado) terminada: {end}");
        end
    }

    async fn serve(
        &mut self,
        conn: &Connection,
        accept: impl Future<Output = Result<ControlStream, SessionEnd>>,
        stop: impl Future<Output = ()>,
    ) -> SessionEnd {
        let Self {
            session,
            injector,
            here,
            screens,
            latency,
            on_sample,
            cfg,
        } = self;
        tokio::pin!(stop);
        // Lo que cambió entre sesiones.
        if let Some(rx) = screens.as_mut()
            && rx.has_changed().unwrap_or(false)
        {
            set_screens(session, injector, rx.borrow_and_update().clone());
        }
        let mut stream = tokio::select! {
            () = &mut stop => return SessionEnd::Stopped,
            r = time::timeout(cfg.heartbeat_timeout, accept) => match r {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) => return e,
                Err(_) => return SessionEnd::Silent,
            },
        };

        let mut beats = Beats::new();
        let mut hist = Histogram::new();
        let mut measure = |t_us: u64, beats: &Beats, hist: &mut Histogram| {
            if let Some(us) = beats.sync().elapsed_since(t_us, now_us()) {
                hist.record(us);
                if let Some(f) = on_sample.as_mut() {
                    f(Duration::from_micros(us));
                }
            }
        };
        let mut heartbeat = time::interval(cfg.heartbeat_interval);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut report = time::interval_at(Instant::now() + cfg.latency_every, cfg.latency_every);
        report.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let silence = time::sleep(cfg.heartbeat_timeout);
        tokio::pin!(silence);

        loop {
            tokio::select! {
                biased;
                () = &mut stop => return SessionEnd::Stopped,
                msg = stream.recv::<StreamMsg>() => match msg {
                    Ok(Some(msg)) => {
                        silence.as_mut().reset(Instant::now() + cfg.heartbeat_timeout);
                        if let StreamMsg::Heartbeat(b) = msg {
                            beats.incoming(b, now_us());
                        }
                        let events = session.on_stream(msg);
                        let injected = !events.is_empty();
                        inject(injector, events);
                        if let (true, StreamMsg::Key { t_us, .. } | StreamMsg::Button { t_us, .. }) = (injected, msg) {
                            measure(t_us, &beats, &mut hist);
                        }
                        publish(here, session.is_active());
                    }
                    Ok(None) => return TransportError::ControlClosed.into(),
                    Err(e) => return e.into(),
                },
                dg = conn.read_datagram() => match dg {
                    Ok(bytes) => {
                        silence.as_mut().reset(Instant::now() + cfg.heartbeat_timeout);
                        match Datagram::decode(&bytes) {
                            Ok(Datagram::Motion(d)) => {
                                let events = session.on_datagram(d);
                                if !events.is_empty() {
                                    inject(injector, events);
                                    measure(d.t_us, &beats, &mut hist);
                                }
                            }
                            // Uno roto se pierde como cualquier datagrama.
                            Err(e) => tracing::warn!("datagrama descartado: {e}"),
                        }
                    }
                    Err(e) => return e.into(),
                },
                _ = heartbeat.tick() => {
                    let beat = StreamMsg::Heartbeat(beats.outgoing(now_us()));
                    if let Err(e) = stream.send(&beat).await {
                        return e.into();
                    }
                }
                _ = report.tick() => {
                    if let Some(l) = hist.take() {
                        tracing::info!(
                            n = l.count,
                            p50_us = l.p50_us,
                            p99_us = l.p99_us,
                            max_us = l.max_us,
                            "latencia de la entrada (captura → inyección)"
                        );
                        latency.send_replace(Some(l));
                        if let Err(e) = stream.send(&StreamMsg::Latency(l)).await {
                            return e.into();
                        }
                    }
                }
                () = &mut silence => return SessionEnd::Silent,
                list = next_value(screens) => match list {
                    Some(list) => set_screens(session, injector, list),
                    None => *screens = None,
                },
            }
        }
    }
}

fn set_screens<I: Injector>(session: &mut ClientSession, injector: &mut I, screens: Vec<Screen>) {
    session.set_screens(screens);
    if let Err(e) = injector.refresh() {
        tracing::warn!("inyección: no se pudieron releer las pantallas: {e}");
    }
}

fn inject<I: Injector>(injector: &mut I, events: Vec<InputEvent>) {
    for e in events {
        if let Err(err) = injector.inject(&e) {
            tracing::error!(event = ?e, "inyección: {err}");
        }
    }
}
