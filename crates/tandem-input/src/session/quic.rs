//! Drivers: conectan las máquinas de estados con una conexión de `tandem-link`.
//!
//! Cada `run` dura lo que dura un enlace. El servidor abre un stream propio sobre
//! la conexión del enlace activo (con su intercambio de `Hello`) y el cliente lo
//! acepta; los datagramas van por la misma conexión. Termina cuando el enlace se
//! cae, cuando el otro lado calla más de `heartbeat_timeout` o cuando `stop` se
//! cumple (el gestor de enlaces avisó de un cambio); en todos los casos el
//! servidor devuelve el foco a su máquina y el cliente suelta todo.

use std::future::Future;

use bytes::Bytes;
use tandem_link::transport::{Connection, ControlStream, TransportError};
use tandem_proto::Hello;
use tokio::sync::{mpsc, watch};
use tokio::time::{self, Instant, MissedTickBehavior};

use super::msg::{Datagram, StreamMsg};
use super::{Capture, ClientSession, Injector, ServerOutput, ServerSession, SessionConfig};
use crate::event::InputEvent;
use crate::geometry::Layout;

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
}

/// Servidor: la captura de esta máquina y su máquina de estados.
pub struct ServerDriver<C> {
    session: ServerSession,
    capture: C,
    events: mpsc::UnboundedReceiver<InputEvent>,
    layouts: Option<watch::Receiver<Layout>>,
    cfg: SessionConfig,
}

impl<C: Capture> ServerDriver<C> {
    /// `events` es por donde la captura manda lo que captura; se conserva entre
    /// sesiones (un enlace nuevo sigue con la misma captura).
    pub fn new(
        session: ServerSession,
        capture: C,
        events: mpsc::UnboundedReceiver<InputEvent>,
        cfg: SessionConfig,
    ) -> Self {
        Self {
            session,
            capture,
            events,
            layouts: None,
            cfg,
        }
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

    pub async fn run(
        &mut self,
        conn: &Connection,
        hello: &Hello,
        stop: impl Future<Output = ()>,
    ) -> SessionEnd {
        let end = self.serve(conn, hello, stop).await;
        for out in self.session.link_lost() {
            apply_local(&mut self.capture, out);
        }
        tracing::info!("sesión de entrada (servidor) terminada: {end}");
        end
    }

    async fn serve(
        &mut self,
        conn: &Connection,
        hello: &Hello,
        stop: impl Future<Output = ()>,
    ) -> SessionEnd {
        let Self {
            session,
            capture,
            events,
            layouts,
            cfg,
        } = self;
        tokio::pin!(stop);
        let mut stream = tokio::select! {
            () = &mut stop => return SessionEnd::Stopped,
            r = time::timeout(cfg.heartbeat_timeout, conn.open_control(hello)) => match r {
                Ok(Ok((stream, _))) => stream,
                Ok(Err(e)) => return e.into(),
                Err(_) => return SessionEnd::Silent,
            },
        };

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
                    // Lo que llegó junto sale junto: así se coalesce el movimiento.
                    let mut outs = session.handle(ev);
                    while let Ok(ev) = events.try_recv() {
                        outs.extend(session.handle(ev));
                    }
                    outs.extend(session.flush().map(ServerOutput::Datagram));
                    if let Err(e) = apply(capture, &mut stream, conn, outs).await {
                        return e.into();
                    }
                    resync.as_mut().reset(Instant::now() + cfg.resync_idle);
                }
                msg = stream.recv::<StreamMsg>() => match msg {
                    // Del cliente solo llegan latidos.
                    Ok(Some(_)) => silence.as_mut().reset(Instant::now() + cfg.heartbeat_timeout),
                    Ok(None) => return TransportError::ControlClosed.into(),
                    Err(e) => return e.into(),
                },
                _ = heartbeat.tick() => {
                    if let Err(e) = stream.send(&StreamMsg::Heartbeat).await {
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
                        if let Err(e) = apply(capture, &mut stream, conn, outs).await {
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

async fn next_layout(layouts: &mut Option<watch::Receiver<Layout>>) -> Option<Layout> {
    match layouts {
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
    cfg: SessionConfig,
}

impl<I: Injector> ClientDriver<I> {
    pub fn new(session: ClientSession, injector: I, cfg: SessionConfig) -> Self {
        Self {
            session,
            injector,
            cfg,
        }
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

    pub async fn run(
        &mut self,
        conn: &Connection,
        hello: &Hello,
        stop: impl Future<Output = ()>,
    ) -> SessionEnd {
        let end = self.serve(conn, hello, stop).await;
        // Pase lo que pase con el enlace, nada queda presionado.
        let release = self.session.link_lost();
        inject(&mut self.injector, release);
        tracing::info!("sesión de entrada (cliente) terminada: {end}");
        end
    }

    async fn serve(
        &mut self,
        conn: &Connection,
        hello: &Hello,
        stop: impl Future<Output = ()>,
    ) -> SessionEnd {
        let Self {
            session,
            injector,
            cfg,
        } = self;
        tokio::pin!(stop);
        let mut stream = tokio::select! {
            () = &mut stop => return SessionEnd::Stopped,
            r = time::timeout(cfg.heartbeat_timeout, conn.accept_control(hello)) => match r {
                Ok(Ok((stream, _))) => stream,
                Ok(Err(e)) => return e.into(),
                Err(_) => return SessionEnd::Silent,
            },
        };

        let mut heartbeat = time::interval(cfg.heartbeat_interval);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let silence = time::sleep(cfg.heartbeat_timeout);
        tokio::pin!(silence);

        loop {
            tokio::select! {
                biased;
                () = &mut stop => return SessionEnd::Stopped,
                msg = stream.recv::<StreamMsg>() => match msg {
                    Ok(Some(msg)) => {
                        silence.as_mut().reset(Instant::now() + cfg.heartbeat_timeout);
                        inject(injector, session.on_stream(msg));
                    }
                    Ok(None) => return TransportError::ControlClosed.into(),
                    Err(e) => return e.into(),
                },
                dg = conn.read_datagram() => match dg {
                    Ok(bytes) => {
                        silence.as_mut().reset(Instant::now() + cfg.heartbeat_timeout);
                        match Datagram::decode(&bytes) {
                            Ok(Datagram::Motion(d)) => inject(injector, session.on_datagram(d)),
                            // Uno roto se pierde como cualquier datagrama.
                            Err(e) => tracing::warn!("datagrama descartado: {e}"),
                        }
                    }
                    Err(e) => return e.into(),
                },
                _ = heartbeat.tick() => {
                    if let Err(e) = stream.send(&StreamMsg::Heartbeat).await {
                        return e.into();
                    }
                }
                () = &mut silence => return SessionEnd::Silent,
            }
        }
    }
}

fn inject<I: Injector>(injector: &mut I, events: Vec<InputEvent>) {
    for e in events {
        if let Err(err) = injector.inject(&e) {
            tracing::error!(event = ?e, "inyección: {err}");
        }
    }
}
