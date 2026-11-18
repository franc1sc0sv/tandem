//! El otro agente por el plano de control: pedirle pasos ([`PeerClient`]) y atender
//! los que él pide ([`serve`]).
//!
//! Cada agente abre su propio stream de pedidos hacia el otro y atiende todos los
//! streams que el otro abre, así que cualquiera de los dos puede orquestar. Los
//! streams atendidos también contestan pings: sirven igual para el que abre el
//! gestor de enlaces.
//!
//! La lógica va sobre [`ControlChannel`]: en el agente es el stream de control de
//! QUIC y en las pruebas puede ser un flujo en memoria.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tandem_core::{Machine, State, Step};
use tandem_input::session::CursorEvent;
use tandem_link::manager::{LinkKind, Snapshot};
use tandem_link::qos::StreamClass;
use tandem_link::transport::{Connection, ControlStream, TransportError};
use tandem_proto::net::MEASURE_SECS;
use tandem_proto::{Channel, ControlMsg, Hello, Peer, ProtoError};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::agent::{Agent, IncomingChannel};
use crate::ipc::Framed;
use crate::orchestrator::{PEER_LOST, PeerLink, StepFuture};

/// Mensajes de control en los dos sentidos.
pub trait ControlChannel: Send + 'static {
    fn send(&mut self, msg: &ControlMsg) -> impl Future<Output = Result<(), String>> + Send;

    /// `None` si el otro lado cerró. Tiene que poder cancelarse sin perder mensajes.
    fn recv(&mut self) -> impl Future<Output = Result<Option<ControlMsg>, String>> + Send;
}

impl ControlChannel for ControlStream {
    async fn send(&mut self, msg: &ControlMsg) -> Result<(), String> {
        ControlStream::send(self, msg)
            .await
            .map_err(|e| e.to_string())
    }

    async fn recv(&mut self) -> Result<Option<ControlMsg>, String> {
        ControlStream::recv(self).await.map_err(|e| e.to_string())
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send + 'static> ControlChannel for Framed<S> {
    async fn send(&mut self, msg: &ControlMsg) -> Result<(), String> {
        Framed::send(self, msg).await.map_err(|e| e.to_string())
    }

    async fn recv(&mut self) -> Result<Option<ControlMsg>, String> {
        Framed::recv(self).await.map_err(|e| e.to_string())
    }
}

enum Outgoing {
    Step(Step, oneshot::Sender<Result<(), String>>),
    Msg(ControlMsg),
    /// Avisa cuando todo lo anterior ya salió por el stream ([`PeerClient::flush`]).
    Flush(oneshot::Sender<()>),
    /// Como `Msg`, pero cuenta en [`PeerClient::lossy`]: se descarta si el stream se atrasa.
    Lossy(ControlMsg),
}

/// Cuántos avisos descartables esperan a la vez en un [`PeerClient`] antes de empezar a
/// tirar los nuevos. Con 30 muestras por segundo, dos pendientes son unos 70 ms de atraso.
pub const LOSSY_BACKLOG: usize = 2;

/// Pide pasos al otro agente por un stream propio. Al soltarlo se cierra el stream.
pub struct PeerClient {
    tx: mpsc::UnboundedSender<Outgoing>,
    /// Avisos descartables en la cola (el cursor en vivo, T-128).
    lossy: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl PeerClient {
    /// Toma el canal y lo atiende en una tarea propia. Llamar dentro de un runtime de tokio.
    pub fn spawn<C: ControlChannel>(chan: C) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let lossy = Arc::new(AtomicUsize::new(0));
        PeerClient {
            tx,
            task: tokio::spawn(client_loop(chan, rx, lossy.clone())),
            lossy,
        }
    }

    /// Abre el stream de pedidos sobre una conexión QUIC ya autenticada.
    pub async fn open(conn: &Connection, me: Machine) -> Result<Self, PeerError> {
        let (stream, theirs) = conn.open_control(&Hello::new(Peer::Agent(me))).await?;
        check_peer(me, theirs.peer)?;
        Ok(Self::spawn(stream))
    }
}

impl Drop for PeerClient {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl PeerLink for PeerClient {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
        Box::pin(async move {
            let (reply, result) = oneshot::channel();
            self.tx
                .send(Outgoing::Step(step.clone(), reply))
                .map_err(|_| PEER_LOST.to_owned())?;
            result.await.map_err(|_| PEER_LOST.to_owned())?
        })
    }

    fn publish_state(&self, state: &State) {
        self.send(ControlMsg::State(state.clone()));
    }

    fn send(&self, msg: ControlMsg) {
        PeerClient::send(self, msg);
    }

    fn send_lossy(&self, msg: ControlMsg) {
        PeerClient::send_lossy(self, msg);
    }
}

impl PeerClient {
    /// Espera a que lo mandado hasta ahora haya salido por el stream (no a que el otro lo
    /// lea): para avisar algo justo antes de cerrar la conexión.
    pub async fn flush(&self) {
        let (done, sent) = oneshot::channel();
        if self.tx.send(Outgoing::Flush(done)).is_ok() {
            let _ = sent.await;
        }
    }

    /// Manda un aviso sin respuesta (estado, hub, pantallas). Si el stream ya se cerró
    /// se pierde: quien avisa lo repite al aparecer el enlace siguiente.
    pub fn send(&self, msg: ControlMsg) {
        let _ = self.tx.send(Outgoing::Msg(msg));
    }

    /// Como [`PeerClient::send`] para lo que se reemplaza solo (las muestras del cursor): si
    /// ya esperan [`LOSSY_BACKLOG`] avisos así porque el stream se atrasó, este se descarta
    /// en lugar de acumularse.
    pub fn send_lossy(&self, msg: ControlMsg) {
        if self.lossy.fetch_add(1, Ordering::AcqRel) >= LOSSY_BACKLOG {
            self.lossy.fetch_sub(1, Ordering::AcqRel);
            return;
        }
        if self.tx.send(Outgoing::Lossy(msg)).is_err() {
            self.lossy.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

async fn client_loop<C: ControlChannel>(
    mut chan: C,
    mut rx: mpsc::UnboundedReceiver<Outgoing>,
    lossy: Arc<AtomicUsize>,
) {
    // Al salir se sueltan las respuestas pendientes: quien espera ve [`PEER_LOST`].
    let mut pending: HashMap<u64, oneshot::Sender<Result<(), String>>> = HashMap::new();
    let mut next_id = 0u64;
    loop {
        // El envío va fuera del `select!`: cortarlo a medias desalinearía el stream.
        let out = tokio::select! {
            out = rx.recv() => match out {
                Some(out) => out,
                None => return,
            },
            msg = chan.recv() => {
                match msg {
                    Ok(Some(ControlMsg::StepResult { id, outcome })) => {
                        // Si quien pidió ya se cansó de esperar, nadie escucha.
                        if let Some(reply) = pending.remove(&id) {
                            let _ = reply.send(outcome.into());
                        }
                    }
                    Ok(Some(ControlMsg::Ping { nonce })) => {
                        if let Err(e) = chan.send(&ControlMsg::Pong { nonce }).await {
                            tracing::debug!("stream de pedidos al otro agente: {e}");
                            return;
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => return,
                    Err(e) => {
                        tracing::debug!("stream de pedidos al otro agente: {e}");
                        return;
                    }
                }
                continue;
            }
        };
        let sent = match out {
            Outgoing::Step(step, reply) => {
                let id = next_id;
                next_id = next_id.wrapping_add(1);
                pending.retain(|_, r| !r.is_closed());
                pending.insert(id, reply);
                chan.send(&ControlMsg::ExecuteStep { id, step }).await
            }
            Outgoing::Msg(msg) => chan.send(&msg).await,
            Outgoing::Flush(done) => {
                let _ = done.send(());
                Ok(())
            }
            Outgoing::Lossy(msg) => {
                // Sale de la cola: el que llegue ahora ya no espera detrás de este.
                lossy.fetch_sub(1, Ordering::AcqRel);
                chan.send(&msg).await
            }
        };
        if let Err(e) = sent {
            tracing::debug!("stream de pedidos al otro agente: {e}");
            return;
        }
    }
}

/// Un pedido al otro agente que viaja por el enlace `kind`: falla con [`PEER_LOST`]
/// en cuanto el gestor de enlaces da ese enlace por caído (DESIGN §6: sin esperar
/// timeouts largos). La caída se nota en un ping, unos 1,25 s; el tiempo máximo del
/// paso es de 10 s. Así la transacción deshace (o sigue) enseguida por el enlace
/// siguiente.
pub async fn over_link<H, F, T>(
    mut links: watch::Receiver<Snapshot<H>>,
    kind: LinkKind,
    request: F,
) -> Result<T, String>
where
    F: Future<Output = Result<T, String>>,
{
    // El gestor tarda al menos un ping en volver a dar el enlace por sano, así que
    // la foto con el enlace caído no se pierde entre dos lecturas.
    let lost = links.wait_for(|s| !s.links.iter().any(|l| l.kind == kind && l.rtt.is_some()));
    tokio::select! {
        // Si la respuesta ya llegó, vale aunque el enlace haya caído después.
        biased;
        r = request => r,
        // Si el gestor se cerró tampoco queda enlace.
        _ = lost => Err(PEER_LOST.to_owned()),
    }
}

/// Atiende un stream abierto por el otro agente hasta que lo cierre.
///
/// Los pasos se ejecutan de a uno y en orden: así el inverso de un paso que venció
/// en el otro lado nunca corre antes que el paso mismo. Si el inverso llega por otro
/// stream (cayó el enlace), el turno del orquestador lo hace esperar igual.
pub async fn serve<C: ControlChannel>(chan: C, agent: Arc<Agent>) -> Result<(), String> {
    serve_from(chan, agent, None).await
}

async fn serve_from<C: ControlChannel>(
    mut chan: C,
    agent: Arc<Agent>,
    mut first: Option<ControlMsg>,
) -> Result<(), String> {
    loop {
        let msg = match first.take() {
            Some(msg) => msg,
            None => match chan.recv().await? {
                Some(msg) => msg,
                None => return Ok(()),
            },
        };
        respond(&mut chan, &agent, msg).await?;
    }
}

async fn respond<C: ControlChannel>(
    chan: &mut C,
    agent: &Arc<Agent>,
    msg: ControlMsg,
) -> Result<(), String> {
    {
        match msg {
            ControlMsg::ExecuteStep { id, step } => {
                let outcome = agent.orchestrator().execute_for_peer(&step).await;
                chan.send(&ControlMsg::StepResult {
                    id,
                    outcome: outcome.into(),
                })
                .await?;
            }
            ControlMsg::Ping { nonce } => chan.send(&ControlMsg::Pong { nonce }).await?,
            ControlMsg::GetState => chan.send(&ControlMsg::State(agent.state())).await?,
            // El estado final de una transacción del otro agente.
            ControlMsg::State(state) => {
                agent.orchestrator().release_peer_lease();
                if let Err(e) = agent.set_state(state) {
                    tracing::warn!("no se pudo guardar el estado que mandó el otro agente: {e}");
                }
            }
            ControlMsg::Hub(report) => {
                agent.inbox().hub.send_replace(Some(report));
            }
            ControlMsg::Screens(screens) => {
                agent.inbox().screens.send_replace(Some(screens));
            }
            ControlMsg::Capabilities(caps) => agent.set_peer_capabilities(*caps),
            ControlMsg::Desk(sync) => agent.receive_desk(*sync),
            ControlMsg::KeyProbe(msg) => agent.receive_key_probe(msg),
            ControlMsg::Keyboard(machine) => agent.receive_keyboard(machine),
            ControlMsg::Monitors(reports) => agent.receive_monitor_reports(reports),
            ControlMsg::WatchResources { on } => agent.receive_watch_resources(on),
            ControlMsg::Resources(r) => agent.receive_peer_resources(*r),
            ControlMsg::Rescan => agent.receive_rescan(),
            ControlMsg::Unpaired => agent.receive_unpaired(),
            ControlMsg::Update(msg) => agent.receive_update(msg),
            ControlMsg::Focus(msg) => agent.inbox().deliver_focus(msg),
            ControlMsg::Drag(msg) => agent.drag().deliver(msg),
            ControlMsg::WatchCursor { on } => agent.receive_watch_cursor(on),
            ControlMsg::Cursor(sample) => agent.receive_peer_cursor(CursorEvent::Sample(sample)),
            ControlMsg::Crossed { from, to, edge } => {
                agent.receive_peer_cursor(CursorEvent::Crossed { from, to, edge });
            }
            // Solo vale como primer mensaje ([`accept_streams`]).
            ControlMsg::Open(channel) => {
                return Err(format!("canal {channel:?} abierto a mitad de un stream"));
            }
            // Las respuestas son del que pide.
            // Se atiende en `serve_stream`, que tiene la conexión.
            ControlMsg::LinkClass { .. } => {}
            ControlMsg::Pong { .. } | ControlMsg::StepResult { .. } => {}
        }
    }
    Ok(())
}

/// Abre un stream propio de `channel` sobre una conexión con el otro agente
/// ([`ControlMsg::Open`]); el otro lado lo entrega con [`crate::agent::PeerInbox`].
/// Cada canal tiene su prioridad ([`StreamClass::of`]): las teclas no esperan detrás de
/// una imagen del portapapeles.
pub async fn open_channel(
    conn: &Connection,
    me: Machine,
    channel: Channel,
) -> Result<ControlStream, PeerError> {
    let (mut stream, theirs) = conn.open_control(&Hello::new(Peer::Agent(me))).await?;
    stream.set_class(StreamClass::of(channel))?;
    check_peer(me, theirs.peer)?;
    stream.send(&ControlMsg::Open(channel)).await?;
    Ok(stream)
}

/// Atiende cada stream que el otro agente abra en `conn`, hasta que la conexión se cierre.
pub async fn accept_streams(conn: Connection, agent: Arc<Agent>) {
    let me = agent.machine();
    let hello = Hello::new(Peer::Agent(me));
    loop {
        let (stream, theirs) = match conn.accept_control(&hello).await {
            Ok(s) => s,
            // Otra versión (por ejemplo, una anterior sin anuncio de capacidades): no se
            // le habla, y el log dice por qué y qué hacer.
            Err(TransportError::Proto(e @ ProtoError::IncompatibleVersion { theirs, .. })) => {
                tracing::warn!("se rechaza al otro agente: {e}");
                agent.peer_protocol_mismatch(theirs);
                return;
            }
            Err(e) => {
                tracing::debug!("conexión con el otro agente terminada: {e}");
                // Su clasificación vale por conexión: la siguiente trae la suya.
                conn.qos().clear_peer_link_of(conn.id());
                return;
            }
        };
        if let Err(e) = check_peer(me, theirs.peer) {
            tracing::warn!("{e}");
            continue;
        }
        let (agent, conn) = (agent.clone(), conn.clone());
        tokio::spawn(async move {
            if let Err(e) = serve_stream(stream, conn, agent).await {
                tracing::debug!("stream del otro agente terminado: {e}");
            }
        });
    }
}

/// Un stream que abrió el otro agente: el de un canal propio se entrega entero al motor
/// de entrada; cualquier otro es de pedidos (o de pings) y se atiende aquí.
async fn serve_stream(
    mut stream: ControlStream,
    conn: Connection,
    agent: Arc<Agent>,
) -> Result<(), String> {
    let mut first = match stream.recv::<ControlMsg>().await {
        Ok(Some(first)) => first,
        Ok(None) => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    // El otro lado dice cómo clasifica esta conexión antes que nada (T-094).
    if let ControlMsg::LinkClass { kind } = first {
        apply_link_class(&conn, kind.into());
        first = match stream.recv::<ControlMsg>().await {
            Ok(Some(next)) => next,
            Ok(None) => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
    }
    if let ControlMsg::Open(Channel::Tunnel) = first {
        // Los túneles no van al motor de entrada: los atiende su módulo.
        return crate::tunnels::serve(stream, &conn, &agent).await;
    }
    if first == ControlMsg::Open(Channel::Measure) {
        return answer_measure(stream).await;
    }
    if let ControlMsg::Open(channel) = first {
        // La prioridad es de quien escribe: lo que este lado mande por el canal también.
        if let Err(e) = stream.set_class(StreamClass::of(channel)) {
            return Err(e.to_string());
        }
        agent.inbox().deliver(IncomingChannel {
            channel,
            conn,
            stream,
        });
        return Ok(());
    }
    serve_from(stream, agent, Some(first)).await
}

/// Guarda la clasificación del otro extremo en el `Qos` del camino, que calcula la etiqueta
/// efectiva (la peor de las dos). Se guarda aparte de la propia, así que se recupera cuando
/// el otro vuelve a anunciar un cable, y no se pierde si la propia aún no se conoce.
fn apply_link_class(conn: &Connection, theirs: LinkKind) {
    let qos = conn.qos();
    let before = qos.link();
    qos.set_peer_link_from(conn.id(), theirs);
    let label = qos.link();
    if label != before {
        tracing::info!(suya = %theirs, ?label, "el otro extremo ve el enlace peor: etiqueta nueva");
    }
}

/// Lo más que puede tardar una prueba de ancho de banda, de punta a punta: la prueba y
/// lo que tarde en vaciarse lo que quedó en camino.
pub const MEASURE_TIMEOUT: Duration = Duration::from_secs(MEASURE_SECS + 10);

/// Mide el ancho de banda de `conn` hacia el otro agente (T-102): datos sin tope durante
/// [`MEASURE_SECS`] por un stream de datos; cuenta el otro lado. En bytes por segundo. Se
/// corta si el `Qos` del camino pausa los datos.
pub async fn measure(conn: &Connection, me: Machine) -> Result<u64, String> {
    let run = async {
        let stream = open_channel(conn, me, Channel::Measure)
            .await
            .map_err(|e| e.to_string())?;
        let report = stream
            .measure(Duration::from_secs(MEASURE_SECS), conn.qos())
            .await
            .map_err(|e| e.to_string())?;
        report
            .bytes_per_sec()
            .ok_or_else(|| "no llegó nada que medir".to_owned())
    };
    tokio::time::timeout(MEASURE_TIMEOUT, run)
        .await
        .map_err(|_| "la prueba no terminó a tiempo".to_owned())?
}

/// El otro lado de [`measure`].
async fn answer_measure(stream: ControlStream) -> Result<(), String> {
    match tokio::time::timeout(MEASURE_TIMEOUT, stream.answer_measure()).await {
        Ok(Ok(report)) => {
            tracing::debug!(?report, "prueba de ancho de banda contestada");
            Ok(())
        }
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("la prueba de ancho de banda no terminó a tiempo".to_owned()),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("se esperaba el agente de {expected}, saludó {got:?}")]
    UnexpectedPeer { expected: Machine, got: Peer },
}

/// El otro lado tiene que ser el agente de la otra máquina.
fn check_peer(me: Machine, got: Peer) -> Result<(), PeerError> {
    if got == Peer::Agent(me.other()) {
        Ok(())
    } else {
        Err(PeerError::UnexpectedPeer {
            expected: me.other(),
            got,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tandem_core::Plan;
    use tokio::io::duplex;

    use super::*;
    use crate::config::fixtures;
    use crate::orchestrator::tests::FakeSteps;
    use crate::store::StateStore;

    fn agent(
        dir: &std::path::Path,
        machine: Machine,
        steps: FakeSteps,
    ) -> (Arc<Agent>, Arc<FakeSteps>) {
        let mut cfg = fixtures::desk();
        cfg.machine = machine;
        let steps = Arc::new(steps);
        let a = Agent::with_local_steps(
            cfg,
            StateStore::new(dir.join(format!("{machine}.json"))),
            steps.clone(),
        );
        (Arc::new(a), steps)
    }

    /// La Mac pide pasos a la PC por un flujo en memoria.
    fn link(mac: &Arc<Agent>, pc: &Arc<Agent>) -> JoinHandle<Result<(), String>> {
        let (a, b) = duplex(64 * 1024);
        mac.set_peer(Arc::new(PeerClient::spawn(Framed::new(a))));
        tokio::spawn(serve(Framed::new(b), pc.clone()))
    }

    /// Un paso que le toca a la PC.
    fn pc_off() -> Step {
        Step::PowerOff {
            machine: Machine::B,
        }
    }

    #[tokio::test]
    async fn remote_step_runs_on_the_peer() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, mac_steps) = agent(dir.path(), Machine::A, FakeSteps::default());
        let (pc, pc_steps) = agent(dir.path(), Machine::B, FakeSteps::default());
        let _serve = link(&mac, &pc);
        mac.orchestrator().dispatch(&pc_off()).await.unwrap();
        assert_eq!(pc_steps.steps(), [pc_off()]);
        assert!(mac_steps.steps().is_empty());
    }

    #[tokio::test]
    async fn published_state_reaches_the_peer() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, _) = agent(dir.path(), Machine::A, FakeSteps::default());
        let (pc, _) = agent(dir.path(), Machine::B, FakeSteps::default());
        let _serve = link(&mac, &pc);
        // El de la Mac: ella prendida (la PC ya lo estaba para sí misma).
        let mut s = pc.state();
        s.on.a = true;
        let mut rx = pc.subscribe();
        mac.orchestrator().peer().publish_state(&s);
        rx.wait_for(|got| *got == s).await.unwrap();
    }

    #[tokio::test]
    async fn closed_stream_fails_pending_and_new_steps() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, _) = agent(dir.path(), Machine::A, FakeSteps::default());
        let (a, b) = duplex(1024);
        mac.set_peer(Arc::new(PeerClient::spawn(Framed::new(a))));
        // El otro lado lee el pedido y cierra sin contestar.
        tokio::spawn(async move {
            let mut f = Framed::new(b);
            let _ = f.recv::<ControlMsg>().await;
        });
        let peer = mac.orchestrator().peer();
        assert_eq!(peer.execute(&pc_off()).await.unwrap_err(), PEER_LOST);
        assert_eq!(peer.execute(&pc_off()).await.unwrap_err(), PEER_LOST);
    }

    #[tokio::test]
    async fn serve_answers_ping_and_state() {
        let dir = tempfile::tempdir().unwrap();
        let (pc, _) = agent(dir.path(), Machine::B, FakeSteps::default());
        let (a, b) = duplex(64 * 1024);
        let task = tokio::spawn(serve(Framed::new(b), pc.clone()));
        let mut f = Framed::new(a);
        f.send(&ControlMsg::Ping { nonce: 5 }).await.unwrap();
        assert_eq!(f.recv().await.unwrap(), Some(ControlMsg::Pong { nonce: 5 }));
        f.send(&ControlMsg::GetState).await.unwrap();
        assert_eq!(f.recv().await.unwrap(), Some(ControlMsg::State(pc.state())));
        // Un paso que no le toca a la PC se rechaza sin ejecutarse.
        f.send(&ControlMsg::ExecuteStep {
            id: 1,
            step: Step::PowerOn {
                machine: Machine::B,
            },
        })
        .await
        .unwrap();
        assert!(matches!(
            f.recv().await.unwrap(),
            Some(ControlMsg::StepResult {
                id: 1,
                outcome: tandem_proto::StepOutcome::Failed(_)
            })
        ));
        drop(f);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn serve_keeps_the_peers_input_reports() {
        let dir = tempfile::tempdir().unwrap();
        let (pc, _) = agent(dir.path(), Machine::B, FakeSteps::default());
        let (a, b) = duplex(64 * 1024);
        let task = tokio::spawn(serve(Framed::new(b), pc.clone()));
        let client = PeerClient::spawn(Framed::new(a));
        let report = tandem_proto::HubReport {
            hub: true,
            server: Machine::A,
        };
        let screens = vec![tandem_proto::ScreenRect {
            id: "mac-1".into(),
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            scale_milli: 1000,
        }];
        let (mut hub, mut scr) = (pc.inbox().hub.subscribe(), pc.inbox().screens.subscribe());
        client.send(ControlMsg::Hub(report));
        client.send(ControlMsg::Screens(screens.clone()));
        hub.wait_for(|h| *h == Some(report)).await.unwrap();
        scr.wait_for(|s| s.as_ref() == Some(&screens))
            .await
            .unwrap();
        // Abrir un canal a mitad del stream de pedidos lo corta.
        client.send(ControlMsg::Open(Channel::Input));
        assert!(task.await.unwrap().is_err());
    }

    #[test]
    fn peer_must_be_the_other_machine() {
        assert!(check_peer(Machine::A, Peer::Agent(Machine::B)).is_ok());
        assert!(check_peer(Machine::A, Peer::Agent(Machine::A)).is_err());
        assert!(check_peer(Machine::B, Peer::Ui).is_err());
    }

    /// El apagado remoto se cuelga en la PC: la Mac lo da por hecho a los 10 s (lo más
    /// probable es que se esté apagando).
    #[tokio::test(start_paused = true)]
    async fn a_remote_power_off_that_hangs_is_taken_as_done() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, _) = agent(dir.path(), Machine::A, FakeSteps::default());
        let (pc, pc_steps) = agent(
            dir.path(),
            Machine::B,
            FakeSteps {
                delays: vec![(pc_off(), Duration::from_secs(3600))],
                ..Default::default()
            },
        );
        let _serve = link(&mac, &pc);
        let mut state = mac.state();
        state.on.b = true;
        let plan = Plan::power_off(&state, Machine::B);
        let start = tokio::time::Instant::now();
        mac.orchestrator().run(1, &mut state, &plan).await.unwrap();
        assert_eq!(start.elapsed(), Duration::from_secs(10));
        assert!(!state.on.b);
        assert_eq!(pc_steps.steps(), [pc_off()]);
    }

    /// QA ronda 2: mientras la PC hace pasos para la Mac no puede empezar un cambio
    /// propio; el estado final de la Mac la libera.
    #[tokio::test]
    async fn the_peers_final_state_ends_the_loan() {
        let dir = tempfile::tempdir().unwrap();
        let (pc, pc_steps) = agent(dir.path(), Machine::B, FakeSteps::default());
        let (a, b) = duplex(64 * 1024);
        let _task = tokio::spawn(serve(Framed::new(b), pc.clone()));
        let mut f = Framed::new(a);
        f.send(&ControlMsg::ExecuteStep {
            id: 1,
            step: pc_off(),
        })
        .await
        .unwrap();
        assert!(matches!(
            f.recv().await.unwrap(),
            Some(ControlMsg::StepResult {
                id: 1,
                outcome: tandem_proto::StepOutcome::Done
            })
        ));
        assert_eq!(pc_steps.steps(), [pc_off()]);
        assert_eq!(pc.orchestrator().try_begin(), None);

        // El estado final de la Mac: ella prendida (la PC ya lo estaba para sí misma).
        let mut s = pc.state();
        assert!(s.is_on(Machine::B) && !s.is_on(Machine::A));
        s.on.a = true;
        let mut rx = pc.subscribe();
        f.send(&ControlMsg::State(s.clone())).await.unwrap();
        rx.wait_for(|got| *got == s).await.unwrap();
        assert!(pc.orchestrator().try_begin().is_some());
    }

    fn snapshot(up: &[LinkKind]) -> Snapshot<()> {
        Snapshot {
            active: None,
            links: [LinkKind::Usb4, LinkKind::Ethernet]
                .into_iter()
                .map(|kind| tandem_link::manager::LinkStatus {
                    kind,
                    addr: std::net::SocketAddr::from(([127, 0, 0, 1], 1)),
                    rtt: up.contains(&kind).then_some(Duration::from_millis(1)),
                })
                .collect(),
            handles: up.iter().map(|k| (*k, ())).collect(),
        }
    }

    /// T-064: el pedido no espera su tiempo máximo si el gestor da su enlace por caído.
    #[tokio::test(start_paused = true)]
    async fn request_fails_when_its_link_goes_down() {
        let (tx, rx) = watch::channel(snapshot(&[LinkKind::Usb4, LinkKind::Ethernet]));
        let start = tokio::time::Instant::now();
        let request = over_link(
            rx,
            LinkKind::Usb4,
            std::future::pending::<Result<(), String>>(),
        );
        let cut = async {
            tokio::time::sleep(Duration::from_millis(1250)).await;
            // Caer otro enlace no afecta al pedido.
            tx.send_replace(snapshot(&[LinkKind::Usb4]));
            tokio::time::sleep(Duration::from_millis(100)).await;
            tx.send_replace(snapshot(&[LinkKind::Ethernet]));
            std::future::pending::<()>().await;
        };
        let r = tokio::select! {
            r = request => r,
            () = cut => unreachable!(),
        };
        assert_eq!(r.unwrap_err(), PEER_LOST);
        assert_eq!(start.elapsed(), Duration::from_millis(1350));
    }

    #[tokio::test(start_paused = true)]
    async fn request_answered_before_the_drop_keeps_its_result() {
        let (tx, rx) = watch::channel(snapshot(&[LinkKind::Usb4]));
        assert_eq!(
            over_link(rx.clone(), LinkKind::Usb4, async { Ok(()) }).await,
            Ok(())
        );
        assert_eq!(
            over_link(rx, LinkKind::Usb4, async {
                Err::<(), _>("falla".to_owned())
            })
            .await,
            Err("falla".to_owned())
        );
        // Con el enlace ya caído no hay a quién pedirle nada.
        tx.send_replace(snapshot(&[]));
        assert_eq!(
            over_link(
                tx.subscribe(),
                LinkKind::Usb4,
                std::future::pending::<Result<(), String>>()
            )
            .await,
            Err(PEER_LOST.to_owned())
        );
    }

    /// Si el gestor se cierra, el pedido tampoco queda esperando.
    #[tokio::test(start_paused = true)]
    async fn request_fails_when_the_manager_closes() {
        let (tx, rx) = watch::channel(snapshot(&[LinkKind::Usb4]));
        drop(tx);
        assert_eq!(
            over_link(
                rx,
                LinkKind::Usb4,
                std::future::pending::<Result<(), String>>()
            )
            .await,
            Err(PEER_LOST.to_owned())
        );
    }

    /// Un canal que se traba hasta que se lo suelta y anota lo que logra mandar.
    struct Stalled {
        open: Arc<tokio::sync::Semaphore>,
        sent: Arc<std::sync::Mutex<Vec<ControlMsg>>>,
    }

    impl ControlChannel for Stalled {
        async fn send(&mut self, msg: &ControlMsg) -> Result<(), String> {
            let _ = self.open.acquire().await.map_err(|e| e.to_string())?;
            self.sent.lock().unwrap().push(msg.clone());
            Ok(())
        }

        async fn recv(&mut self) -> Result<Option<ControlMsg>, String> {
            std::future::pending().await
        }
    }

    /// El cursor en vivo (T-128) no se acumula si el stream se atrasa: se descartan los
    /// nuevos y lo importante (un cruce) sigue por el camino de siempre.
    #[tokio::test]
    async fn lossy_messages_are_dropped_while_the_stream_is_backed_up() {
        let open = Arc::new(tokio::sync::Semaphore::new(0));
        let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
        let client = PeerClient::spawn(Stalled {
            open: open.clone(),
            sent: sent.clone(),
        });
        let sample = |x: i32| {
            ControlMsg::Cursor(tandem_proto::CursorSample {
                machine: Machine::A,
                screen: String::new(),
                x,
                y: 0,
            })
        };
        // El primero se lo lleva la tarea (queda trabado enviando); los siguientes esperan.
        client.send_lossy(sample(0));
        tokio::time::sleep(Duration::from_millis(50)).await;
        for x in 1..=20 {
            client.send_lossy(sample(x));
        }
        client.send(ControlMsg::Rescan);
        open.add_permits(1000);
        tokio::time::timeout(Duration::from_secs(5), async {
            while sent.lock().unwrap().len() < LOSSY_BACKLOG + 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let got = sent.lock().unwrap().clone();
        // La que estaba en vuelo, las dos que cupieron en la cola y el aviso normal.
        assert_eq!(
            got,
            vec![sample(0), sample(1), sample(2), ControlMsg::Rescan],
            "{got:?}"
        );
        // Ya vacía la cola, vuelve a aceptar.
        client.send_lossy(sample(99));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(sent.lock().unwrap().last(), Some(&sample(99)));
    }
}
