//! API local para la UI: Unix socket en macOS (y Linux) y named pipe en Windows,
//! accesibles solo para el usuario actual (DESIGN §9).
//!
//! Habla los mensajes UI ↔ agente de `tandem-proto`: cada lado manda su [`Hello`]
//! (el agente primero), y después la UI manda [`UiRequest`] y el agente contesta con
//! [`AgentToUi`]. El agente además empuja [`AgentToUi::State`] cada vez que el estado cambia
//! y [`AgentToUi::Txn`] con el avance de cada encendido o apagado, [`AgentToUi::Power`]
//! cuando la otra máquina se da por apagada sin que nadie la apagara, [`AgentToUi::Peer`]
//! cuando cambia el par o el enlace con él, [`AgentToUi::Pairing`] con cada paso del
//! emparejamiento, [`AgentToUi::Input`] cuando cambia el teclado y mouse (roles, sesión,
//! permisos que faltan), [`AgentToUi::Desk`] cuando cambia el puesto compartido,
//! [`AgentToUi::DeskChanged`] cuando ese cambio vino de la otra máquina (el aviso con
//! «Deshacer») y [`AgentToUi::Machines`] cuando cambia lo que anuncia alguna de las dos
//! (módulos y entornos incluidos). Una conexión que pidió
//! [`UiCommand::WatchResources`] recibe además [`AgentToUi::Resources`] de las dos
//! máquinas mientras siga abierta, y una que pidió [`UiCommand::WatchNetwork`] recibe
//! [`AgentToUi::Network`] una vez por segundo (T-102); una que pidió
//! [`UiCommand::WatchCursor`] recibe [`AgentToUi::Cursor`] (a 30 Hz como mucho) y
//! [`AgentToUi::Crossed`] (T-128). [`UiCommand::MeasureLink`] se
//! contesta al terminar la prueba, sin frenar los avisos de esa conexión.
//! [`AgentToUi::Update`] va cuando cambia la versión de alguna de las dos.
//!
//! El transporte va detrás de [`Listener`]: la lógica se prueba con [`mem`] y el
//! socket o pipe reales en las pruebas de integración.

use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tandem_core::{Machine, State};
use tandem_input::session::CursorEvent;
use tandem_proto::{
    AgentToUi, FrameDecoder, Hello, Peer, ProtoError, Reply, UiCommand, UiRequest, decode,
    decode_hello, encode_frame,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;
use tokio::time::{self, Interval, MissedTickBehavior};

use crate::agent::{Agent, CursorWatch, ResourceWatch};

pub mod mem;
#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::{UnixIpcListener as PlatformListener, bind, connect, connect_as, default_endpoint};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{PipeListener as PlatformListener, bind, connect, connect_as, default_endpoint};

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("error de E/S en la IPC: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Proto(#[from] ProtoError),
    #[error("el otro lado cerró la conexión")]
    Closed,
    #[error("se esperaba {expected}, se conectó {got:?}")]
    UnexpectedPeer { expected: &'static str, got: Peer },
    #[error("respuesta inesperada: {0:?}")]
    UnexpectedReply(Box<Reply>),
    #[error("ya hay un tandemd escuchando en {0}")]
    AlreadyRunning(PathBuf),
    #[error("{0} existe y no es un socket; no se borra")]
    NotASocket(PathBuf),
}

/// Fuente de conexiones de la UI.
pub trait Listener: Send {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    /// La siguiente conexión ya aceptada (y, en el socket real, del usuario correcto).
    fn accept(&mut self) -> impl Future<Output = io::Result<Self::Stream>> + Send;
}

/// Mensajes con trama de `tandem-proto` sobre un flujo de bytes.
#[derive(Debug)]
pub struct Framed<S> {
    io: S,
    decoder: FrameDecoder,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Framed<S> {
    pub fn new(io: S) -> Self {
        Framed {
            io,
            decoder: FrameDecoder::new(),
        }
    }

    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<(), IpcError> {
        self.io.write_all(&encode_frame(msg)?).await?;
        self.io.flush().await?;
        Ok(())
    }

    /// `None` si el otro lado cerró limpio, entre dos mensajes.
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<Option<T>, IpcError> {
        match self.recv_frame().await? {
            Some(p) => Ok(Some(decode(&p)?)),
            None => Ok(None),
        }
    }

    /// Lee el [`Hello`] de apertura, rechazando otra versión del protocolo.
    pub async fn recv_hello(&mut self) -> Result<Hello, IpcError> {
        let p = self.recv_frame().await?.ok_or(IpcError::Closed)?;
        Ok(decode_hello(&p)?)
    }

    /// Se puede cancelar (por ejemplo en un `select!`) sin perder bytes: lo leído se
    /// guarda en el decodificador antes del siguiente punto de espera.
    async fn recv_frame(&mut self) -> Result<Option<Vec<u8>>, IpcError> {
        let mut buf = [0u8; 4096];
        loop {
            if let Some(frame) = self.decoder.next_frame()? {
                return Ok(Some(frame));
            }
            let n = self.io.read(&mut buf).await?;
            if n == 0 {
                return if self.decoder.buffered() == 0 {
                    Ok(None)
                } else {
                    Err(io::Error::from(io::ErrorKind::UnexpectedEof).into())
                };
            }
            self.decoder.push(&buf[..n]);
        }
    }
}

/// Atiende una conexión de la UI hasta que se cierra o llega el apagado.
pub async fn serve_connection<S>(
    io: S,
    agent: Arc<Agent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), IpcError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut conn = Framed::new(io);
    conn.send(&Hello::new(Peer::Agent(agent.machine()))).await?;
    let hello = conn.recv_hello().await?;
    // La UI o un cliente (`tandem mcp`, `tandem`): los dos corren como el usuario, el único
    // que la IPC deja entrar. Solo un cliente abre y cierra túneles.
    let client = match hello.peer {
        Peer::Ui => None,
        Peer::Client(kind) => Some(kind),
        Peer::Agent(_) => {
            return Err(IpcError::UnexpectedPeer {
                expected: "la UI o un cliente",
                got: hello.peer,
            });
        }
    };
    let mut states = agent.subscribe();
    states.mark_unchanged();
    let mut txns = agent.subscribe_txns();
    let mut power = agent.subscribe_power();
    let mut drag = agent.drag().subscribe();
    let mut peer = agent.subscribe_peer();
    peer.mark_unchanged();
    let mut pairing = agent.subscribe_pairing();
    pairing.mark_unchanged();
    let mut input = agent.subscribe_input();
    input.mark_unchanged();
    // La sonda de teclado (T-166). Sin estado inicial: la ventana arma la sonda al entrar al
    // paso, y armar ya emite el estado actual.
    let mut key_probe = agent.subscribe_key_probe();
    key_probe.mark_unchanged();
    let mut desk = agent.subscribe_desk();
    desk.mark_unchanged();
    let mut caps = agent.subscribe_capabilities();
    caps.mark_unchanged();
    let mut desk_changes = agent.subscribe_desk_changes();
    let mut monitors = agent.subscribe_monitors();
    monitors.mark_unchanged();
    let mut tunnels = agent.tunnels().subscribe();
    tunnels.mark_unchanged();
    // Los recursos en vivo, si esta conexión los pidió: se dejan de muestrear al cerrarla.
    let mut resources: Option<ResourceWatch> = None;
    // El cursor en vivo, si esta conexión lo pidió: sin conexiones, el motor no lo muestrea.
    let mut cursor: Option<CursorWatch> = None;
    let mut update = agent.subscribe_update();
    update.mark_unchanged();
    // La red una vez por segundo, si esta conexión la pidió (la pantalla Red visible).
    let mut network: Option<Interval> = None;
    // La medición de ancho de banda que pidió esta conexión, con el id de su pedido: tarda
    // unos segundos y mientras tanto la conexión sigue atendiendo lo demás.
    let mut measuring: Option<(u64, Measurement)> = None;
    // Las copias de archivos de esta conexión (T-106), con su canal de respuestas y el avance
    // de las transferencias de este agente.
    let mut jobs = tokio::task::JoinSet::new();
    let (jobs_tx, mut jobs_rx) = tokio::sync::mpsc::unbounded_channel::<(u64, Reply)>();
    let mut progress = agent.files().subscribe_progress();
    loop {
        tokio::select! {
            req = conn.recv::<UiRequest>() => {
                let Some(req) = req? else { return Ok(()) };
                tracing::debug!(id = req.id, command = ?req.command, "pedido de la UI");
                let reply = match req.command {
                    UiCommand::WatchResources { on } => {
                        resources = on.then(|| agent.watch_resources());
                        Reply::Done
                    }
                    UiCommand::WatchNetwork { on } => {
                        network = on.then(network_ticker);
                        Reply::Done
                    }
                    UiCommand::WatchCursor { on } => {
                        // Pedirlo dos veces no suma: la conexión cuenta una sola vez.
                        if on && cursor.is_none() {
                            cursor = Some(agent.watch_cursor());
                        } else if !on {
                            cursor = None;
                        }
                        Reply::Done
                    }
                    UiCommand::MeasureLink { .. } if measuring.is_some() => {
                        Reply::Rejected("Ya se está midiendo un camino".into())
                    }
                    UiCommand::MeasureLink { path } => {
                        let agent = agent.clone();
                        let run: Measurement = Box::pin(async move { agent.measure_link(path).await });
                        measuring = Some((req.id, run));
                        continue;
                    }
                    command if crate::tunnels::is_slow(&command) => {
                        // Una copia tarda: corre aparte y la conexión sigue atendiendo. Si la
                        // conexión se cierra, `jobs` se suelta y la copia se interrumpe (lo
                        // recibido queda en disco y se reanuda).
                        let (agent, done, id) = (agent.clone(), jobs_tx.clone(), req.id);
                        jobs.spawn(async move {
                            let reply = crate::tunnels::handle(&agent, client, command).await;
                            let _ = done.send((id, reply));
                        });
                        continue;
                    }
                    command => crate::tunnels::handle(&agent, client, command).await,
                };
                conn.send(&AgentToUi::Reply { id: req.id, reply }).await?;
            }
            () = next_tick(&mut network) => {
                conn.send(&AgentToUi::Network(Box::new(agent.network_status()))).await?;
            }
            Some((id, reply)) = jobs_rx.recv() => {
                conn.send(&AgentToUi::Reply { id, reply }).await?;
            }
            Some(_) = jobs.join_next(), if !jobs.is_empty() => {}
            p = progress.recv() => match p {
                Ok(p) => conn.send(&AgentToUi::Transfer(p)).await?,
                // Si esta conexión se atrasó, se pierden avances viejos y no pasa nada.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return Ok(()),
            },
            (id, reply) = measured(&mut measuring) => {
                measuring = None;
                conn.send(&AgentToUi::Reply { id, reply }).await?;
            }
            event = next_cursor(&mut cursor) => match event {
                Some(CursorEvent::Sample(sample)) => conn.send(&AgentToUi::Cursor(sample)).await?,
                Some(CursorEvent::Crossed { from, to, edge }) => {
                    conn.send(&AgentToUi::Crossed { from, to, edge }).await?;
                }
                None => cursor = None,
            },
            sample = next_resources(&mut resources) => match sample {
                Some((machine, r)) => {
                    conn.send(&AgentToUi::Resources { machine, resources: Box::new(r) }).await?;
                }
                None => resources = None,
            },
            changed = states.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let state = states.borrow_and_update().clone();
                conn.send(&AgentToUi::State(state)).await?;
            }
            event = txns.recv() => match event {
                Ok(event) => conn.send(&AgentToUi::Txn(event)).await?,
                // La UI vuelve a sincronizarse con el siguiente estado empujado.
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!("la UI se atrasó y perdió {n} avisos de transacción");
                }
                Err(RecvError::Closed) => return Ok(()),
            },
            event = drag.recv() => match event {
                Ok(status) => conn.send(&AgentToUi::Drag(status)).await?,
                // El siguiente aviso reemplaza al que se perdió.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return Ok(()),
            },
            event = power.recv() => match event {
                Ok(event) => conn.send(&AgentToUi::Power(event)).await?,
                // El estado empujado ya dice que la otra máquina está apagada.
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!("la UI se atrasó y perdió {n} avisos de energía");
                }
                Err(RecvError::Closed) => return Ok(()),
            },
            changed = peer.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let status = peer.borrow_and_update().clone();
                conn.send(&AgentToUi::Peer(status)).await?;
            }
            changed = pairing.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let status = pairing.borrow_and_update().clone();
                conn.send(&AgentToUi::Pairing(status)).await?;
            }
            changed = input.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let status = input.borrow_and_update().clone();
                conn.send(&AgentToUi::Input(status)).await?;
            }
            changed = key_probe.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let status = *key_probe.borrow_and_update();
                conn.send(&AgentToUi::KeyProbe(status)).await?;
            }
            changed = desk.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let d = desk.borrow_and_update().clone();
                conn.send(&AgentToUi::Desk(d)).await?;
            }
            event = desk_changes.recv() => match event {
                Ok(c) => {
                    conn.send(&AgentToUi::DeskChanged {
                        by: c.by,
                        rev: c.rev,
                        summary: c.summary,
                        undo_to: c.undo_to,
                    })
                    .await?;
                }
                // El puesto nuevo ya llegó empujado; solo se pierde el aviso.
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!("la UI se atrasó y perdió {n} avisos de cambios del puesto");
                }
                Err(RecvError::Closed) => return Ok(()),
            },
            changed = caps.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let c = caps.borrow_and_update().clone();
                conn.send(&machines(c)).await?;
            }
            changed = monitors.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let views = monitors.borrow_and_update().clone();
                conn.send(&AgentToUi::Monitors(views)).await?;
            }
            changed = update.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let u = update.borrow_and_update().clone();
                conn.send(&AgentToUi::Update(u)).await?;
            }
            changed = tunnels.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                tunnels.borrow_and_update();
                conn.send(&AgentToUi::Tunnels(agent.tunnels().view())).await?;
            }
            () = stopped(&mut shutdown) => return Ok(()),
        }
    }
}

/// Una medición de ancho de banda en curso ([`Agent::measure_link`]).
type Measurement = Pin<Box<dyn Future<Output = Reply> + Send>>;

/// Cada cuánto va [`AgentToUi::Network`] a quien la pidió.
pub const NETWORK_EVERY: Duration = Duration::from_secs(1);

/// La primera enseguida (la pantalla se dibuja ya), después una por segundo.
fn network_ticker() -> Interval {
    let mut tick = time::interval(NETWORK_EVERY);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    tick
}

/// El próximo envío de la red; nunca llega si la conexión no la pidió.
async fn next_tick(tick: &mut Option<Interval>) {
    match tick {
        Some(t) => {
            t.tick().await;
        }
        None => std::future::pending().await,
    }
}

/// El final de la medición en curso, con el id de su pedido; nunca llega sin medición.
async fn measured(m: &mut Option<(u64, Measurement)>) -> (u64, Reply) {
    match m {
        Some((id, run)) => (*id, run.await),
        None => std::future::pending().await,
    }
}

/// La siguiente muestra de recursos; nunca termina si la conexión no los pidió.
async fn next_resources(
    watch: &mut Option<ResourceWatch>,
) -> Option<(Machine, tandem_proto::Resources)> {
    match watch {
        Some(w) => w.next().await,
        None => std::future::pending().await,
    }
}

/// El siguiente evento del cursor; nunca termina si la conexión no lo pidió.
async fn next_cursor(watch: &mut Option<CursorWatch>) -> Option<CursorEvent> {
    match watch {
        Some(w) => w.next().await,
        None => std::future::pending().await,
    }
}

/// Lo que anuncian las dos máquinas, como aviso para la UI.
pub fn machines(c: tandem_proto::DeskCapabilities) -> AgentToUi {
    AgentToUi::Machines {
        local: Box::new(c.here),
        remote: c.peer.map(Box::new),
    }
}

/// Termina cuando se pide el apagado o desaparece quien lo pediría.
async fn stopped(shutdown: &mut watch::Receiver<bool>) {
    // El `Ref` que devuelve `wait_for` no es `Send`: se suelta aquí mismo.
    let _ = shutdown.wait_for(|stop| *stop).await.map(|_| ());
}

/// Cliente de la IPC: lo usan las pruebas y, más adelante, la UI.
#[derive(Debug)]
pub struct IpcClient<S> {
    conn: Framed<S>,
    agent: Machine,
    next_id: u64,
    /// Avisos que llegaron mientras se esperaba una respuesta.
    pending: VecDeque<AgentToUi>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> IpcClient<S> {
    pub async fn handshake(io: S) -> Result<Self, IpcError> {
        Self::handshake_as(io, Peer::Ui).await
    }

    /// Como [`Self::handshake`], presentándose como `peer`: `tandem mcp` y la CLI lo hacen
    /// como [`Peer::Client`] para poder abrir y cerrar túneles.
    pub async fn handshake_as(io: S, peer: Peer) -> Result<Self, IpcError> {
        let mut conn = Framed::new(io);
        conn.send(&Hello::new(peer)).await?;
        let hello = conn.recv_hello().await?;
        let Peer::Agent(agent) = hello.peer else {
            return Err(IpcError::UnexpectedPeer {
                expected: "un agente",
                got: hello.peer,
            });
        };
        Ok(IpcClient {
            conn,
            agent,
            next_id: 0,
            pending: VecDeque::new(),
        })
    }

    /// Máquina del agente al otro lado.
    pub fn agent(&self) -> Machine {
        self.agent
    }

    pub async fn request(&mut self, command: UiCommand) -> Result<Reply, IpcError> {
        let id = self.next_id;
        self.next_id += 1;
        self.conn.send(&UiRequest { id, command }).await?;
        loop {
            match self
                .conn
                .recv::<AgentToUi>()
                .await?
                .ok_or(IpcError::Closed)?
            {
                AgentToUi::Reply { id: got, reply } if got == id => return Ok(reply),
                other => self.pending.push_back(other),
            }
        }
    }

    pub async fn get_state(&mut self) -> Result<State, IpcError> {
        match self.request(UiCommand::GetState).await? {
            Reply::State(s) => Ok(s),
            other => Err(IpcError::UnexpectedReply(Box::new(other))),
        }
    }

    /// El siguiente aviso espontáneo del agente; `None` si cerró.
    pub async fn next_event(&mut self) -> Result<Option<AgentToUi>, IpcError> {
        if let Some(ev) = self.pending.pop_front() {
            return Ok(Some(ev));
        }
        self.conn.recv().await
    }
}

#[cfg(test)]
mod tests {
    use tandem_proto::PROTOCOL_VERSION;
    use tokio::io::duplex;

    use super::*;
    use tandem_proto::TxnEvent;

    use crate::agent::tests::agent;
    use crate::orchestrator::STEP_NOT_IMPLEMENTED;

    fn start(dir: &std::path::Path) -> (Arc<Agent>, tokio::io::DuplexStream, watch::Sender<bool>) {
        let agent = Arc::new(agent(dir));
        let (client, server) = duplex(64);
        let (stop, stop_rx) = watch::channel(false);
        let a = agent.clone();
        tokio::spawn(async move { serve_connection(server, a, stop_rx).await });
        (agent, client, stop)
    }

    #[tokio::test]
    async fn client_gets_state() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        assert_eq!(c.agent(), Machine::A);
        assert_eq!(c.get_state().await.unwrap(), agent.state());
    }

    /// T-105: los recursos llegan solo a la conexión que los pidió, de las dos máquinas, y
    /// dejan de llegar al pedir que no.
    #[tokio::test(start_paused = true)]
    async fn resources_flow_only_while_the_connection_watches() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        assert_eq!(
            c.request(UiCommand::WatchResources { on: true })
                .await
                .unwrap(),
            Reply::Done
        );
        let Some(AgentToUi::Resources { machine, .. }) = c.next_event().await.unwrap() else {
            panic!("se esperaban recursos");
        };
        assert_eq!(machine, Machine::A);
        let mut theirs = tandem_proto::Resources::default();
        theirs.cpu.load = 420;
        agent.receive_peer_resources(theirs.clone());
        loop {
            match c.next_event().await.unwrap() {
                Some(AgentToUi::Resources {
                    machine: Machine::A,
                    ..
                }) => continue,
                Some(AgentToUi::Resources { machine, resources }) => {
                    assert_eq!((machine, *resources), (Machine::B, theirs));
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(
            c.request(UiCommand::WatchResources { on: false })
                .await
                .unwrap(),
            Reply::Done
        );
        // Lo que llegó antes de la respuesta se acaba; después, nada.
        let mut after = 0;
        while let Ok(ev) =
            tokio::time::timeout(std::time::Duration::from_secs(5), c.next_event()).await
        {
            assert!(matches!(ev.unwrap(), Some(AgentToUi::Resources { .. })));
            after += 1;
            assert!(after < 5, "siguen llegando recursos");
        }
    }

    /// T-102: sin suscripción no llega la red; con ella, una vez por segundo; al quitarla,
    /// deja de llegar.
    #[tokio::test(start_paused = true)]
    async fn network_flows_only_while_the_connection_watches() {
        let dir = tempfile::tempdir().unwrap();
        let (_agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        assert!(
            time::timeout(Duration::from_secs(5), c.next_event())
                .await
                .is_err(),
            "llegó algo sin suscripción"
        );
        assert_eq!(
            c.request(UiCommand::WatchNetwork { on: true })
                .await
                .unwrap(),
            Reply::Done
        );
        let t0 = time::Instant::now();
        for _ in 0..3 {
            let Some(AgentToUi::Network(s)) = c.next_event().await.unwrap() else {
                panic!("se esperaba la red");
            };
            assert_eq!(s.paths.len(), 3);
        }
        // La primera enseguida y después una por segundo.
        assert_eq!(t0.elapsed(), 2 * NETWORK_EVERY);
        assert_eq!(
            c.request(UiCommand::WatchNetwork { on: false })
                .await
                .unwrap(),
            Reply::Done
        );
        let mut after = 0;
        while let Ok(ev) = time::timeout(Duration::from_secs(5), c.next_event()).await {
            assert!(matches!(ev.unwrap(), Some(AgentToUi::Network(_))));
            after += 1;
            assert!(after < 3, "sigue llegando la red");
        }
    }

    /// T-128: el cursor llega solo a la conexión que lo pidió, el motor solo lo muestrea
    /// mientras alguna lo pide (contadas por ventana), y al cerrarse la conexión se deja.
    #[tokio::test]
    async fn the_cursor_flows_only_to_connections_that_watch_and_is_counted_per_window() {
        use tandem_proto::CursorSample;

        let dir = tempfile::tempdir().unwrap();
        let (agent, io, stop) = start(dir.path());
        let mut watching = IpcClient::handshake(io).await.unwrap();
        let (client, server) = duplex(64);
        let (a, stopped) = (agent.clone(), stop.subscribe());
        tokio::spawn(async move { serve_connection(server, a, stopped).await });
        let mut idle = IpcClient::handshake(client).await.unwrap();

        let sample = |x| CursorSample {
            machine: Machine::A,
            screen: "mac-1".into(),
            x,
            y: 1,
        };
        let gate = agent.cursor_gate();
        assert!(!gate.is_on());
        agent.publish_cursor(CursorEvent::Sample(sample(0)));

        // Dos pedidos de la misma conexión cuentan una vez; una conexión que no lo pide no
        // lo recibe.
        for _ in 0..2 {
            assert_eq!(
                watching
                    .request(UiCommand::WatchCursor { on: true })
                    .await
                    .unwrap(),
                Reply::Done
            );
        }
        assert!(gate.is_on());
        agent.publish_cursor(CursorEvent::Sample(sample(1)));
        agent.publish_cursor(CursorEvent::Crossed {
            from: Machine::A,
            to: Machine::B,
            edge: tandem_core::Side::Right,
        });
        assert_eq!(
            watching.next_event().await.unwrap(),
            Some(AgentToUi::Cursor(sample(1)))
        );
        assert_eq!(
            watching.next_event().await.unwrap(),
            Some(AgentToUi::Crossed {
                from: Machine::A,
                to: Machine::B,
                edge: tandem_core::Side::Right
            })
        );
        assert!(
            time::timeout(Duration::from_millis(100), idle.next_event())
                .await
                .is_err(),
            "llegó el cursor a una conexión que no lo pidió"
        );

        // Una segunda ventana que mira y se va no apaga la primera.
        assert_eq!(
            idle.request(UiCommand::WatchCursor { on: true })
                .await
                .unwrap(),
            Reply::Done
        );
        assert_eq!(
            idle.request(UiCommand::WatchCursor { on: false })
                .await
                .unwrap(),
            Reply::Done
        );
        assert!(gate.is_on(), "la otra ventana sigue mirando");

        // Con `on: false` la última se apaga y el motor deja de muestrear.
        assert_eq!(
            watching
                .request(UiCommand::WatchCursor { on: false })
                .await
                .unwrap(),
            Reply::Done
        );
        assert!(!gate.is_on());

        // Cerrar la conexión sin avisar también deja de mirar.
        watching
            .request(UiCommand::WatchCursor { on: true })
            .await
            .unwrap();
        assert!(gate.is_on());
        drop(watching);
        time::timeout(Duration::from_secs(5), async {
            while gate.is_on() {
                time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("la conexión cerrada siguió contando");
    }

    /// Medir sin conexión por ese camino se rechaza enseguida, por la misma conexión.
    #[tokio::test]
    async fn measuring_a_path_without_connection_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let (_agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        let reply = c
            .request(UiCommand::MeasureLink {
                path: tandem_proto::Link::Wifi,
            })
            .await
            .unwrap();
        assert!(matches!(reply, Reply::Rejected(_)), "{reply:?}");
    }

    #[tokio::test]
    async fn peer_status_is_answered_and_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        assert_eq!(
            c.request(UiCommand::GetPeer).await.unwrap(),
            Reply::Peer(tandem_proto::PeerStatus::Unpaired)
        );
        let paired = tandem_proto::PeerStatus::Paired {
            name: "PC".into(),
            link: Some(tandem_proto::Link::Usb4),
        };
        agent.set_peer_status(paired.clone());
        assert_eq!(c.next_event().await.unwrap(), Some(AgentToUi::Peer(paired)));
    }

    #[tokio::test]
    async fn pairing_status_is_answered_and_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        assert_eq!(
            c.request(UiCommand::GetPairing).await.unwrap(),
            Reply::Pairing(tandem_proto::PairingStatus::Idle)
        );
        let comparing = tandem_proto::PairingStatus::Comparing {
            code: "012345".into(),
            peer: "PC".into(),
            confirmed: false,
        };
        agent.set_pairing_status(comparing.clone());
        assert_eq!(
            c.next_event().await.unwrap(),
            Some(AgentToUi::Pairing(comparing))
        );
    }

    #[tokio::test]
    async fn input_status_is_answered_and_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        let initial = tandem_proto::InputStatus::new(Machine::A);
        assert_eq!(
            c.request(UiCommand::GetInput).await.unwrap(),
            Reply::Input(initial.clone())
        );
        let missing = tandem_proto::InputStatus {
            missing: vec![tandem_proto::InputPermission::Accessibility],
            ..initial
        };
        agent.set_input_status(missing.clone());
        // Los permisos también cambian lo que anuncia esta máquina (T-084).
        let mut seen = Vec::new();
        while seen.len() < 2 {
            seen.push(c.next_event().await.unwrap());
        }
        assert!(seen.contains(&Some(AgentToUi::Input(missing))), "{seen:?}");
        assert!(
            seen.iter().any(|e| matches!(
                e,
                Some(AgentToUi::Machines { local, .. }) if local.input.missing == [tandem_proto::InputPermission::Accessibility]
            )),
            "{seen:?}"
        );
    }

    #[tokio::test]
    async fn power_change_progress_is_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        let r = c
            .request(UiCommand::SetPower {
                machine: Machine::B,
                on: true,
            })
            .await
            .unwrap();
        let Reply::Accepted { txn } = r else {
            panic!("se esperaba Accepted: {r:?}");
        };
        // Sin backends, el primer paso falla con un motivo claro y el estado no cambia.
        let mut events = Vec::new();
        loop {
            match c.next_event().await.unwrap().unwrap() {
                AgentToUi::Txn(e) => {
                    let end = matches!(e, TxnEvent::Failed { .. });
                    events.push(e);
                    if end {
                        break;
                    }
                }
                other => panic!("aviso inesperado: {other:?}"),
            }
        }
        assert!(matches!(&events[0], TxnEvent::Started { txn: t, .. } if *t == txn));
        assert_eq!(events[1], TxnEvent::StepStarted { txn, index: 0 });
        assert!(matches!(
            &events[2],
            TxnEvent::StepFailed { index: 0, error, .. } if error.contains(STEP_NOT_IMPLEMENTED)
        ));
        assert_eq!(
            events[3],
            TxnEvent::Failed {
                txn,
                state: agent.state()
            }
        );
        assert_eq!(c.get_state().await.unwrap(), agent.state());
    }

    #[tokio::test]
    async fn state_changes_are_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        // Tras el saludo, la suscripción del servidor ya existe si respondió un pedido.
        c.get_state().await.unwrap();
        let mut s = agent.state();
        s.on.b = true;
        agent.set_state(s.clone()).unwrap();
        assert_eq!(c.next_event().await.unwrap(), Some(AgentToUi::State(s)));
    }

    #[tokio::test]
    async fn power_events_are_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        c.get_state().await.unwrap();
        let ev = tandem_proto::PowerEvent::PeerLost {
            machine: Machine::B,
            reason: "sin enlaces".into(),
        };
        agent.emit_power(ev.clone());
        assert_eq!(c.next_event().await.unwrap(), Some(AgentToUi::Power(ev)));
    }

    #[tokio::test]
    async fn pushed_state_during_request_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        c.get_state().await.unwrap();
        let mut s = agent.state();
        s.on.b = true;
        agent.set_state(s.clone()).unwrap();
        // El aviso llega antes que la respuesta; no se pierde.
        assert_eq!(c.get_state().await.unwrap(), s);
        assert_eq!(c.next_event().await.unwrap(), Some(AgentToUi::State(s)));
    }

    #[tokio::test]
    async fn server_rejects_other_protocol_version() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (client, server) = duplex(64);
        let (_stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(serve_connection(server, agent, stop_rx));
        let mut c = Framed::new(client);
        c.send(&Hello {
            version: PROTOCOL_VERSION + 1,
            peer: Peer::Ui,
        })
        .await
        .unwrap();
        let err = task.await.unwrap().unwrap_err();
        assert!(
            matches!(err, IpcError::Proto(ProtoError::IncompatibleVersion { .. })),
            "{err}"
        );
    }

    #[tokio::test]
    async fn server_rejects_an_agent_as_ui() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (client, server) = duplex(64);
        let (_stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(serve_connection(server, agent, stop_rx));
        let mut c = Framed::new(client);
        c.send(&Hello::new(Peer::Agent(Machine::B))).await.unwrap();
        let err = task.await.unwrap().unwrap_err();
        assert!(matches!(err, IpcError::UnexpectedPeer { .. }), "{err}");
    }

    #[tokio::test]
    async fn client_rejects_a_ui_as_agent() {
        let (client, server) = duplex(64);
        tokio::spawn(async move {
            let mut s = Framed::new(server);
            s.send(&Hello::new(Peer::Ui)).await.unwrap();
            let _ = s.recv_hello().await;
        });
        let err = IpcClient::handshake(client).await.unwrap_err();
        assert!(matches!(err, IpcError::UnexpectedPeer { .. }), "{err}");
    }

    #[tokio::test]
    async fn shutdown_ends_the_connection() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (client, server) = duplex(64);
        let (stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(serve_connection(server, agent, stop_rx));
        let mut c = IpcClient::handshake(client).await.unwrap();
        c.get_state().await.unwrap();
        stop.send_replace(true);
        task.await.unwrap().unwrap();
        assert!(matches!(c.next_event().await, Ok(None)));
    }

    // QA ronda 1: una UI puede mandar varios pedidos sin esperar respuesta; cada
    // respuesta lleva su id y salen en orden.
    #[tokio::test]
    async fn pipelined_requests_are_answered_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = Framed::new(io);
        c.send(&Hello::new(Peer::Ui)).await.unwrap();
        assert_eq!(c.recv_hello().await.unwrap().peer, Peer::Agent(Machine::A));
        let ids = [7u64, 3, u64::MAX, 3];
        for id in ids {
            c.send(&UiRequest {
                id,
                command: UiCommand::GetState,
            })
            .await
            .unwrap();
        }
        for id in ids {
            assert_eq!(
                c.recv::<AgentToUi>().await.unwrap(),
                Some(AgentToUi::Reply {
                    id,
                    reply: Reply::State(agent.state())
                })
            );
        }
    }

    #[tokio::test]
    async fn garbage_request_ends_only_that_connection() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (stop, stop_rx) = watch::channel(false);
        let (bad_client, bad_server) = duplex(64);
        let bad = tokio::spawn(serve_connection(bad_server, agent.clone(), stop_rx.clone()));
        let (good_client, good_server) = duplex(64);
        tokio::spawn(serve_connection(good_server, agent.clone(), stop_rx));

        let mut b = Framed::new(bad_client);
        b.send(&Hello::new(Peer::Ui)).await.unwrap();
        b.recv_hello().await.unwrap();
        // Una trama bien formada cuyo contenido no es un `UiRequest`.
        b.io.write_all(&[3, 0, 0, 0, 0xFF, 0xFF, 0xFF])
            .await
            .unwrap();
        let err = bad.await.unwrap().unwrap_err();
        assert!(
            matches!(err, IpcError::Proto(ProtoError::Decode(_))),
            "{err}"
        );

        // La otra UI sigue atendida.
        let mut g = IpcClient::handshake(good_client).await.unwrap();
        assert_eq!(g.get_state().await.unwrap(), agent.state());
        drop(stop);
    }

    #[tokio::test]
    async fn oversized_frame_from_ui_is_rejected_without_reading_it() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (client, server) = duplex(64);
        let (_stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(serve_connection(server, agent, stop_rx));
        let mut c = Framed::new(client);
        c.send(&Hello::new(Peer::Ui)).await.unwrap();
        c.recv_hello().await.unwrap();
        // Solo la cabecera: anuncia 4 GiB y el agente corta sin esperar el cuerpo.
        c.io.write_all(&u32::MAX.to_le_bytes()).await.unwrap();
        let err = task.await.unwrap().unwrap_err();
        assert!(
            matches!(err, IpcError::Proto(ProtoError::FrameTooLarge { .. })),
            "{err}"
        );
    }

    #[tokio::test]
    async fn request_before_hello_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (client, server) = duplex(64);
        let (_stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(serve_connection(server, agent, stop_rx));
        let mut c = Framed::new(client);
        c.recv_hello().await.unwrap();
        // Un pedido en lugar del saludo: no se atiende.
        c.send(&UiRequest {
            id: 0,
            command: UiCommand::GetState,
        })
        .await
        .unwrap();
        assert!(task.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn clean_close_between_messages_is_none() {
        let (a, b) = duplex(64);
        drop(b);
        let mut f = Framed::new(a);
        assert!(matches!(f.recv::<UiRequest>().await, Ok(None)));
    }

    #[tokio::test]
    async fn close_mid_frame_is_an_error() {
        let (a, mut b) = duplex(64);
        b.write_all(&[10, 0, 0, 0, 1]).await.unwrap();
        drop(b);
        let mut f = Framed::new(a);
        assert!(matches!(f.recv::<UiRequest>().await, Err(IpcError::Io(_))));
    }

    #[tokio::test]
    async fn hello_on_closed_stream_is_closed() {
        let (a, b) = duplex(64);
        drop(b);
        let mut f = Framed::new(a);
        assert!(matches!(f.recv_hello().await, Err(IpcError::Closed)));
    }

    #[tokio::test]
    async fn request_on_closed_connection_fails() {
        let (client, server) = duplex(64);
        tokio::spawn(async move {
            let mut s = Framed::new(server);
            s.send(&Hello::new(Peer::Agent(Machine::B))).await.unwrap();
            let _ = s.recv_hello().await;
            let _ = s.recv::<UiRequest>().await;
        });
        let mut c = IpcClient::handshake(client).await.unwrap();
        assert!(matches!(c.get_state().await, Err(IpcError::Closed)));
    }
}
