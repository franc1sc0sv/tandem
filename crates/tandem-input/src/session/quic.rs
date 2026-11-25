//! Drivers: connect the state machines to a `tandem-link` connection.
//!
//! Each `run` lasts as long as a link lasts. The server opens a stream of its own over
//! the active link's connection (with its `Hello` exchange) and the client
//! accepts it; datagrams go over the same connection. It ends when the link
//! drops, when the other side is silent for more than `heartbeat_timeout` or when `stop`
//! is met (the link manager announced a change); in every case the
//! server gives the focus back to its machine and the client releases everything.
//!
//! Measurement (T-093): the server stamps each event on taking it out of the capture; heartbeats
//! carry a stamp and echo to estimate the clock offset; the client measures on injecting and
//! every [`SessionConfig::latency_every`] publishes p50/p99/max, records it and sends it
//! to the server, which also publishes it.

use std::future::Future;
use std::time::Duration;

use bytes::Bytes;
use tandem_link::transport::{Connection, ControlStream, TransportError};
use tandem_proto::{Hello, InputLatency};
use tokio::sync::{mpsc, watch};
use tokio::time::{self, Instant, MissedTickBehavior};

use super::cursor::{CursorFeed, CursorTap, sleep_until_opt};
use super::latency::{Beats, Captured, Histogram, now_us};
use super::msg::{Datagram, StreamMsg};
use super::{Capture, ClientSession, Injector, ServerOutput, ServerSession, SessionConfig};
use crate::event::InputEvent;
use crate::geometry::{Layout, Screen};

/// Why a session ended.
#[derive(Debug, thiserror::Error)]
pub enum SessionEnd {
    #[error("stopped: the link changed")]
    Stopped,
    #[error("the other side stopped giving signs of life")]
    Silent,
    #[error("link: {0}")]
    Link(#[from] TransportError),
    #[error("the capture closed")]
    CaptureClosed,
    #[error("the input stream could not be opened: {0}")]
    Open(String),
}

/// Server: this machine's capture and its state machine.
pub struct ServerDriver<C> {
    session: ServerSession,
    capture: C,
    events: mpsc::UnboundedReceiver<Captured>,
    layouts: Option<watch::Receiver<Layout>>,
    active: watch::Sender<bool>,
    remote: watch::Sender<bool>,
    locked: watch::Sender<bool>,
    /// Some mouse button pressed (T-121: cancel the copy of a drag if it is released).
    held: watch::Sender<bool>,
    /// Lock requests from outside the keyboard (the UI, T-129).
    lock_tx: mpsc::UnboundedSender<bool>,
    lock_rx: mpsc::UnboundedReceiver<bool>,
    activity: watch::Sender<Option<Instant>>,
    latency: watch::Sender<Option<InputLatency>>,
    /// The live cursor, if someone asks for it (T-128).
    cursor: Option<CursorFeed>,
    cfg: SessionConfig,
}

/// At most how often physical input is reported: arbitration (D7) works in
/// tens of milliseconds and so the hot path does not wake anyone per event.
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
    /// `events` is where the capture sends what it captures; it is kept between
    /// sessions (a new link continues with the same capture). What arrives without a
    /// session only updates what is pressed and the local cursor
    /// ([`ServerSession::catch_up`]); while there is no link, [`Self::idle`]
    /// consumes it so that it does not pile up.
    pub fn new(
        session: ServerSession,
        capture: C,
        events: mpsc::UnboundedReceiver<Captured>,
        cfg: SessionConfig,
    ) -> Self {
        let (lock_tx, lock_rx) = mpsc::unbounded_channel();
        Self {
            session,
            capture,
            events,
            layouts: None,
            active: watch::Sender::new(false),
            remote: watch::Sender::new(false),
            locked: watch::Sender::new(false),
            held: watch::Sender::new(false),
            lock_tx,
            lock_rx,
            activity: watch::Sender::new(None),
            latency: watch::Sender::new(None),
            cursor: None,
            cfg,
        }
    }

    /// Publishes the cursor and the crossings through `tap` while its switch is on
    /// (T-128). Off, the only cost is reading it.
    pub fn with_cursor(mut self, tap: CursorTap) -> Self {
        let machine = self.session.machine();
        self.cursor = Some(CursorFeed::new(tap, machine));
        self
    }

    /// When the last physical input of this machine arrived (with or without a session): the
    /// capture only sees the physical, what is injected does not reach it.
    pub fn activity(&self) -> watch::Receiver<Option<Instant>> {
        self.activity.subscribe()
    }

    /// The latency the client reports every [`SessionConfig::latency_every`].
    pub fn latency(&self) -> watch::Receiver<Option<InputLatency>> {
        self.latency.subscribe()
    }

    /// `true` while the focus is on the other machine.
    pub fn remote(&self) -> watch::Receiver<bool> {
        self.remote.subscribe()
    }

    /// `true` while the cursor is locked on its machine ([`ServerSession::is_locked`]).
    pub fn locked(&self) -> watch::Receiver<bool> {
        self.locked.subscribe()
    }

    /// `true` while a mouse button is pressed.
    pub fn buttons(&self) -> watch::Receiver<bool> {
        self.held.subscribe()
    }

    /// Where to ask to lock (`true`) or release (`false`) the cursor without the shortcut: the same
    /// effect as [`ServerSession::set_locked`], inside the loop that owns the session.
    /// It stops working when the driver is dropped.
    pub fn lock_control(&self) -> mpsc::UnboundedSender<bool> {
        self.lock_tx.clone()
    }

    /// Updates [`Self::remote`] and [`Self::locked`] with the session.
    fn publish(&self) {
        publish(&self.remote, self.session.is_remote());
        publish(&self.locked, self.session.is_locked());
        publish(&self.held, self.session.button_held());
    }

    /// `true` from when the session is established (stream open and the old stuff
    /// discarded) until it ends: what is captured from then on does count.
    pub fn active(&self) -> watch::Receiver<bool> {
        self.active.subscribe()
    }

    /// Hot geometry: every new value is applied at once.
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

    /// Without a link: consumes what is captured until `until` is met, without crossing
    /// or accumulating anything. Returns `Err` if the capture closed.
    pub async fn idle(&mut self, until: impl Future<Output = ()>) -> Result<(), SessionEnd> {
        tokio::pin!(until);
        loop {
            tokio::select! {
                biased;
                () = &mut until => return Ok(()),
                Some(on) = self.lock_rx.recv() => {
                    self.session.set_locked(on);
                    self.publish();
                }
                ev = self.events.recv() => match ev {
                    Some(ev) => {
                        touch(&self.activity);
                        self.session.catch_up(ev.event);
                        self.publish();
                        publish_cursor(&mut self.cursor, &mut self.session);
                    }
                    None => return Err(SessionEnd::CaptureClosed),
                },
                () = sleep_until_opt(self.cursor.as_ref().and_then(CursorFeed::next_due)) => {
                    flush_cursor(&mut self.cursor, &self.session);
                }
            }
        }
    }

    /// A session over `conn`: opens the input stream with the exchange of
    /// [`Hello`] and serves it until it ends.
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

    /// Like [`Self::run`], with the stream opened by `open` (the agent opens it with its
    /// own greeting, to tell it apart from the connection's other streams).
    /// `open` has [`SessionConfig::heartbeat_timeout`] to finish.
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
        tracing::info!("input session (with the keyboard) ended: {end}");
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
            held,
            lock_rx,
            activity,
            latency,
            cursor,
            cfg,
            ..
        } = self;
        tokio::pin!(stop);
        // What was captured without a session is not replayed: it would cross by itself.
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
            publish(held, session.button_held());
        };
        let outs = session.start();
        if let Err(e) = apply(capture, &mut stream, conn, outs).await {
            return e.into();
        }
        sync(session);
        publish_cursor(cursor, session);

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
                Some(on) = lock_rx.recv() => {
                    session.set_locked(on);
                    sync(session);
                }
                ev = events.recv() => {
                    let Some(ev) = ev else { return SessionEnd::CaptureClosed };
                    touch(activity);
                    // It is sent right away; only what arrived while the
                    // previous one was being sent is gathered (it is in the channel): that way
                    // movement is coalesced without waiting for anyone. Every event carries the stamp of when
                    // it was captured: the wait in the channel counts toward latency.
                    let mut outs = session.handle_at(ev.event, ev.t_us);
                    while let Ok(ev) = events.try_recv() {
                        outs.extend(session.handle_at(ev.event, ev.t_us));
                    }
                    outs.extend(session.flush().map(ServerOutput::Datagram));
                    let sent = apply(capture, &mut stream, conn, outs).await;
                    sync(session);
                    publish_cursor(cursor, session);
                    if let Err(e) = sent {
                        return e.into();
                    }
                    resync.as_mut().reset(Instant::now() + cfg.resync_idle);
                }
                msg = stream.recv::<StreamMsg>() => match msg {
                    // Heartbeats and the latency it measures come from the client.
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
                                    "input latency reported by the other machine"
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
                () = sleep_until_opt(cursor.as_ref().and_then(CursorFeed::next_due)) => {
                    flush_cursor(cursor, session);
                }
                layout = next_layout(layouts) => match layout {
                    Some(l) => {
                        let outs = session.set_layout(l);
                        let sent = apply(capture, &mut stream, conn, outs).await;
                        sync(session);
                        publish_cursor(cursor, session);
                        if let Err(e) = sent {
                            return e.into();
                        }
                    }
                    // Nobody else publishes geometry: it continues with the last one.
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

/// The server's live cursor (T-128): its crossings and, while the cursor is on its
/// screen, its position. If it is on the other machine, the sample is published by the client, which
/// is the one with the focus. Crossings are always collected so that they do not pile up.
fn publish_cursor(cursor: &mut Option<CursorFeed>, session: &mut ServerSession) {
    let crossings = session.take_crossings();
    let Some(feed) = cursor.as_mut() else { return };
    if !feed.active() {
        return;
    }
    for c in crossings {
        feed.crossed(c);
    }
    if session.is_remote() {
        feed.discard();
    } else {
        feed.observe(session.local_cursor(), |p| session.screen_id(p));
    }
}

fn flush_cursor(cursor: &mut Option<CursorFeed>, session: &ServerSession) {
    let Some(feed) = cursor else { return };
    if !feed.active() {
        return;
    }
    // The focus may have gone to the other machine while the sample waited.
    if session.is_remote() {
        feed.discard();
    } else {
        feed.flush(|p| session.screen_id(p));
    }
}

/// Empties what is already in the channel without waiting.
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

/// The next value of an optional `watch`; `None` if it closed, and without a receiver never.
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

/// A capture failure does not cut the session: it is logged and it goes on, so as not to
/// leave the user without a keyboard because of a system error.
fn apply_local<C: Capture>(capture: &mut C, out: ServerOutput) {
    let result = match out {
        ServerOutput::Grab(grab) => capture.set_grab(grab),
        ServerOutput::Warp(pos) => capture.warp(pos),
        ServerOutput::Stream(_) | ServerOutput::Datagram(_) => return,
    };
    if let Err(e) = result {
        tracing::error!("capture: {e}");
    }
}

/// Client: this machine's injection and its state machine.
pub struct ClientDriver<I> {
    session: ClientSession,
    injector: I,
    here: watch::Sender<bool>,
    screens: Option<watch::Receiver<Vec<Screen>>>,
    latency: watch::Sender<Option<InputLatency>>,
    /// Each measured latency, in addition to the window (the data guardian, T-101).
    on_sample: Option<Box<dyn FnMut(Duration) + Send>>,
    /// The live cursor, if someone asks for it (T-128).
    cursor: Option<CursorFeed>,
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
            cursor: None,
            cfg,
        }
    }

    /// Publishes through `tap` the cursor of `machine` (this driver's) while the cursor
    /// is here and the tap's switch is on (T-128).
    pub fn with_cursor(mut self, tap: CursorTap, machine: tandem_core::Machine) -> Self {
        self.cursor = Some(CursorFeed::new(tap, machine));
        self
    }

    /// The latency measured here in the last window ([`SessionConfig::latency_every`]).
    pub fn latency(&self) -> watch::Receiver<Option<InputLatency>> {
        self.latency.subscribe()
    }

    /// Calls `f` with each measured latency (capture → injection).
    pub fn on_latency_sample(mut self, f: impl FnMut(Duration) + Send + 'static) -> Self {
        self.on_sample = Some(Box::new(f));
        self
    }

    /// This machine's hot screens: every new value goes to the session and
    /// the injector reads them again.
    pub fn with_screens(mut self, mut screens: watch::Receiver<Vec<Screen>>) -> Self {
        self.session
            .set_screens(screens.borrow_and_update().clone());
        self.screens = Some(screens);
        self
    }

    /// `true` while the cursor is on this machine.
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

    /// A session over `conn`: accepts the input stream the server opens.
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

    /// Like [`Self::run`], with the stream that `accept` delivers (the agent accepts the
    /// connection's streams and passes this one). `accept` has
    /// [`SessionConfig::heartbeat_timeout`] to finish.
    pub async fn run_with(
        &mut self,
        conn: &Connection,
        accept: impl Future<Output = Result<ControlStream, SessionEnd>>,
        stop: impl Future<Output = ()>,
    ) -> SessionEnd {
        let end = self.serve(conn, accept, stop).await;
        // Whatever happens with the link, nothing is left pressed.
        let release = self.session.link_lost();
        inject(&mut self.injector, release);
        focus(&mut self.injector, &self.here, false);
        tracing::info!("input session (without the keyboard) ended: {end}");
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
            cursor,
            cfg,
        } = self;
        tokio::pin!(stop);
        // What changed between sessions.
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
                        focus(injector, here, session.is_active());
                        client_cursor(cursor, session);
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
                                    client_cursor(cursor, session);
                                }
                            }
                            // A broken one is lost like any datagram.
                            Err(e) => tracing::warn!("datagram discarded: {e}"),
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
                            "input latency (capture → injection)"
                        );
                        latency.send_replace(Some(l));
                        if let Err(e) = stream.send(&StreamMsg::Latency(l)).await {
                            return e.into();
                        }
                    }
                }
                () = &mut silence => return SessionEnd::Silent,
                () = sleep_until_opt(cursor.as_ref().and_then(CursorFeed::next_due)) => {
                    flush_client_cursor(cursor, session);
                }
                list = next_value(screens) => match list {
                    Some(list) => set_screens(session, injector, list),
                    None => *screens = None,
                },
            }
        }
    }
}

/// The client's live cursor (T-128): its position while the focus is here.
fn client_cursor(cursor: &mut Option<CursorFeed>, session: &ClientSession) {
    let Some(feed) = cursor.as_mut() else { return };
    if !feed.active() {
        return;
    }
    match session.cursor() {
        Some(p) => feed.observe(p, |p| session.screen_id(p)),
        None => feed.discard(),
    }
}

fn flush_client_cursor(cursor: &mut Option<CursorFeed>, session: &ClientSession) {
    let Some(feed) = cursor else { return };
    if !feed.active() {
        return;
    }
    if session.is_active() {
        feed.flush(|p| session.screen_id(p));
    } else {
        feed.discard();
    }
}

fn set_screens<I: Injector>(session: &mut ClientSession, injector: &mut I, screens: Vec<Screen>) {
    session.set_screens(screens);
    if let Err(e) = injector.refresh() {
        tracing::warn!("injection: the screens could not be reread: {e}");
    }
}

/// Tells the injector and whoever watches when the cursor arrives or leaves; only on changes.
fn focus<I: Injector>(injector: &mut I, here: &watch::Sender<bool>, active: bool) {
    if *here.borrow() == active {
        return;
    }
    if let Err(e) = injector.set_focus(active) {
        tracing::warn!("injection: {e}");
    }
    publish(here, active);
}

fn inject<I: Injector>(injector: &mut I, events: Vec<InputEvent>) {
    for e in events {
        if let Err(err) = injector.inject(&e) {
            tracing::error!(event = ?e, "injection: {err}");
        }
    }
}
