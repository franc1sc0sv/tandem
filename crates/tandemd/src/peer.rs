//! El otro agente por el plano de control: pedirle pasos ([`PeerClient`]) y atender
//! los que él pide ([`serve`]).
//!
//! Cada agente abre su propio stream de pedidos hacia el otro y atiende todos los
//! streams que el otro abre, así que cualquiera de los dos puede orquestar. Los
//! streams atendidos también contestan pings: sirven igual para el que abre el
//! gestor de enlaces.
//!
//! Las sondas de calibración (T-078) van por el mismo stream de pedidos, pero se
//! atienden aparte: esperan el clic del usuario y no frenan los pasos ni entre ellas.
//!
//! La lógica va sobre [`ControlChannel`]: en el agente es el stream de control de
//! QUIC y en las pruebas puede ser un flujo en memoria.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tandem_core::{Machine, State, Step};
use tandem_link::manager::{LinkKind, Snapshot};
use tandem_link::transport::{Connection, ControlStream, TransportError};
use tandem_proto::{CalibrationProbe, Channel, ControlMsg, Hello, Peer, ProbeOutcome};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::agent::{Agent, IncomingChannel};
use crate::ipc::Framed;
use crate::orchestrator::{PEER_LOST, PeerLink, ProbeFuture, StepFuture};
use crate::store::reconcile;

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
    Probe {
        id: u64,
        probe: CalibrationProbe,
        reply: oneshot::Sender<ProbeOutcome>,
    },
    Msg(ControlMsg),
}

/// Pide pasos al otro agente por un stream propio. Al soltarlo se cierra el stream.
pub struct PeerClient {
    tx: mpsc::UnboundedSender<Outgoing>,
    task: JoinHandle<()>,
    /// Las sondas se numeran aquí: quien espera tiene que saber qué sonda quitar.
    next_probe: AtomicU64,
}

impl PeerClient {
    /// Toma el canal y lo atiende en una tarea propia. Llamar dentro de un runtime de tokio.
    pub fn spawn<C: ControlChannel>(chan: C) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        PeerClient {
            tx,
            task: tokio::spawn(client_loop(chan, rx)),
            next_probe: AtomicU64::new(1),
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

    fn show_probe<'a>(&'a self, probe: &'a CalibrationProbe) -> ProbeFuture<'a> {
        Box::pin(async move {
            let id = self.next_probe.fetch_add(1, Ordering::Relaxed);
            let (reply, result) = oneshot::channel();
            self.tx
                .send(Outgoing::Probe {
                    id,
                    probe: probe.clone(),
                    reply,
                })
                .map_err(|_| PEER_LOST.to_owned())?;
            let mut hide = HideProbe {
                tx: &self.tx,
                id,
                answered: false,
            };
            let outcome = result.await.map_err(|_| PEER_LOST.to_owned())?;
            hide.answered = true;
            Ok(outcome)
        })
    }
}

/// Si quien espera la sonda deja de esperarla (venció en el motor o se canceló), se le
/// pide al otro agente que la quite.
struct HideProbe<'a> {
    tx: &'a mpsc::UnboundedSender<Outgoing>,
    id: u64,
    answered: bool,
}

impl Drop for HideProbe<'_> {
    fn drop(&mut self) {
        if !self.answered {
            let _ = self
                .tx
                .send(Outgoing::Msg(ControlMsg::HideProbe { id: self.id }));
        }
    }
}

impl PeerClient {
    /// Manda un aviso sin respuesta (estado, hub, pantallas). Si el stream ya se cerró
    /// se pierde: quien avisa lo repite al aparecer el enlace siguiente.
    pub fn send(&self, msg: ControlMsg) {
        let _ = self.tx.send(Outgoing::Msg(msg));
    }
}

async fn client_loop<C: ControlChannel>(mut chan: C, mut rx: mpsc::UnboundedReceiver<Outgoing>) {
    // Al salir se sueltan las respuestas pendientes: quien espera ve [`PEER_LOST`].
    let mut pending: HashMap<u64, oneshot::Sender<Result<(), String>>> = HashMap::new();
    let mut probes: HashMap<u64, oneshot::Sender<ProbeOutcome>> = HashMap::new();
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
                    Ok(Some(ControlMsg::ProbeResult { id, outcome })) => {
                        if let Some(reply) = probes.remove(&id) {
                            let _ = reply.send(outcome);
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
            Outgoing::Probe { id, probe, reply } => {
                probes.retain(|_, r| !r.is_closed());
                probes.insert(id, reply);
                chan.send(&ControlMsg::ShowProbe { id, probe }).await
            }
            Outgoing::Msg(msg) => {
                if let ControlMsg::HideProbe { id } = &msg {
                    probes.remove(id);
                }
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
///
/// Las sondas de calibración ([`ControlMsg::ShowProbe`]) corren en su propia tarea y su
/// respuesta sale cuando el usuario contesta o vence el plazo; al cerrarse el stream se
/// quitan todas las que sigan a la vista.
pub async fn serve<C: ControlChannel>(chan: C, agent: Arc<Agent>) -> Result<(), String> {
    serve_from(chan, agent, None).await
}

async fn serve_from<C: ControlChannel>(
    mut chan: C,
    agent: Arc<Agent>,
    mut first: Option<ControlMsg>,
) -> Result<(), String> {
    let (done_tx, mut done) = mpsc::unbounded_channel::<(u64, ProbeOutcome)>();
    let mut probes: HashMap<u64, Abort> = HashMap::new();
    loop {
        let msg = match first.take() {
            Some(msg) => msg,
            None => tokio::select! {
                msg = chan.recv() => match msg? {
                    Some(msg) => msg,
                    None => return Ok(()),
                },
                Some((id, outcome)) = done.recv() => {
                    probes.remove(&id);
                    chan.send(&ControlMsg::ProbeResult { id, outcome }).await?;
                    continue;
                }
            },
        };
        match msg {
            ControlMsg::ShowProbe { id, probe } => {
                let (agent, done) = (agent.clone(), done_tx.clone());
                let task = tokio::spawn(async move {
                    let outcome = agent.probes().show(&probe, false).await;
                    let _ = done.send((id, outcome));
                });
                probes.insert(id, Abort(task));
            }
            // Soltar la tarea suelta la sonda, y eso la quita de la UI.
            ControlMsg::HideProbe { id } => drop(probes.remove(&id)),
            msg => respond(&mut chan, &agent, msg).await?,
        }
    }
}

/// Una sonda atendida para el otro agente; se quita si el stream se cierra.
struct Abort(JoinHandle<()>);

impl Drop for Abort {
    fn drop(&mut self) {
        self.0.abort();
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
                let state = reconcile(state, &agent.config().desk);
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
            // Solo vale como primer mensaje ([`accept_streams`]).
            ControlMsg::Open(channel) => {
                return Err(format!("canal {channel:?} abierto a mitad de un stream"));
            }
            // Las sondas las atiende `serve_from`; las respuestas son del que pide.
            ControlMsg::ShowProbe { .. } | ControlMsg::HideProbe { .. } => {}
            ControlMsg::Pong { .. }
            | ControlMsg::StepResult { .. }
            | ControlMsg::ProbeResult { .. } => {}
        }
    }
    Ok(())
}

/// Abre un stream propio de `channel` sobre una conexión con el otro agente
/// ([`ControlMsg::Open`]); el otro lado lo entrega con [`crate::agent::PeerInbox`].
pub async fn open_channel(
    conn: &Connection,
    me: Machine,
    channel: Channel,
) -> Result<ControlStream, PeerError> {
    let (mut stream, theirs) = conn.open_control(&Hello::new(Peer::Agent(me))).await?;
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
            Err(e) => {
                tracing::debug!("conexión con el otro agente terminada: {e}");
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
    let first = match stream.recv::<ControlMsg>().await {
        Ok(Some(first)) => first,
        Ok(None) => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    if let ControlMsg::Open(channel) = first {
        agent.inbox().deliver(IncomingChannel {
            channel,
            conn,
            stream,
        });
        return Ok(());
    }
    serve_from(stream, agent, Some(first)).await
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

    use tandem_core::{Input, Mode, Plan, TxnError};
    use tokio::io::duplex;

    use super::*;
    use crate::config::fixtures;
    use crate::orchestrator::describe;
    use crate::orchestrator::tests::FakeSteps;
    use crate::store::StateStore;
    use tandem_proto::ProbeAnswer;

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

    fn switch_34_to_pc() -> Step {
        Step::SwitchInput {
            monitor: "m34".into(),
            from: Machine::Mac,
            to: Machine::Pc,
            input: Input::DisplayPort,
        }
    }

    #[tokio::test]
    async fn remote_step_runs_on_the_peer() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, mac_steps) = agent(dir.path(), Machine::Mac, FakeSteps::default());
        let (pc, pc_steps) = agent(dir.path(), Machine::Pc, FakeSteps::default());
        let _serve = link(&mac, &pc);
        mac.orchestrator()
            .dispatch(&switch_34_to_pc())
            .await
            .unwrap();
        assert_eq!(pc_steps.steps(), [switch_34_to_pc()]);
        assert!(mac_steps.steps().is_empty());
    }

    #[tokio::test]
    async fn published_state_reaches_the_peer() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, _) = agent(dir.path(), Machine::Mac, FakeSteps::default());
        let (pc, _) = agent(dir.path(), Machine::Pc, FakeSteps::default());
        let _serve = link(&mac, &pc);
        let mut s = mac.state();
        s.pc_on = true;
        s.owner.insert("m34".into(), Machine::Pc);
        let mut rx = pc.subscribe();
        mac.orchestrator().peer().publish_state(&s);
        rx.wait_for(|got| *got == s).await.unwrap();
    }

    #[tokio::test]
    async fn closed_stream_fails_pending_and_new_steps() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, _) = agent(dir.path(), Machine::Mac, FakeSteps::default());
        let (a, b) = duplex(1024);
        mac.set_peer(Arc::new(PeerClient::spawn(Framed::new(a))));
        // El otro lado lee el pedido y cierra sin contestar.
        tokio::spawn(async move {
            let mut f = Framed::new(b);
            let _ = f.recv::<ControlMsg>().await;
        });
        let peer = mac.orchestrator().peer();
        assert_eq!(
            peer.execute(&switch_34_to_pc()).await.unwrap_err(),
            PEER_LOST
        );
        assert_eq!(
            peer.execute(&switch_34_to_pc()).await.unwrap_err(),
            PEER_LOST
        );
    }

    #[tokio::test]
    async fn serve_answers_ping_and_state() {
        let dir = tempfile::tempdir().unwrap();
        let (pc, _) = agent(dir.path(), Machine::Pc, FakeSteps::default());
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
            step: Step::PowerOnPc,
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
        let (pc, _) = agent(dir.path(), Machine::Pc, FakeSteps::default());
        let (a, b) = duplex(64 * 1024);
        let task = tokio::spawn(serve(Framed::new(b), pc.clone()));
        let client = PeerClient::spawn(Framed::new(a));
        let report = tandem_proto::HubReport {
            hub: true,
            server: Machine::Mac,
        };
        let screens = vec![tandem_proto::ScreenRect {
            monitor: None,
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

    fn probe(timeout_ms: u64) -> CalibrationProbe {
        CalibrationProbe {
            monitor: tandem_proto::ProbeMonitor {
                edid: None,
                name: "Odyssey G5".into(),
            },
            input: Input::Hdmi,
            machine: Machine::Mac,
            ask: tandem_proto::ProbeAsk::Candidate { code: 0x05 },
            timeout_ms,
        }
    }

    async fn shown(rx: &mut tokio::sync::broadcast::Receiver<tandem_proto::ProbeEvent>) -> u64 {
        match rx.recv().await.unwrap() {
            tandem_proto::ProbeEvent::Show { id, .. } => id,
            other => panic!("se esperaba Show: {other:?}"),
        }
    }

    /// T-078: la PC pide la sonda, la UI de la Mac la muestra y el clic vuelve a la PC.
    #[tokio::test(start_paused = true)]
    async fn a_remote_probe_is_shown_and_answered_by_the_peer() {
        let dir = tempfile::tempdir().unwrap();
        let (pc, _) = agent(dir.path(), Machine::Pc, FakeSteps::default());
        let (mac, _) = agent(dir.path(), Machine::Mac, FakeSteps::default());
        let _serve = link(&pc, &mac);
        let mut ui = mac.subscribe_probes();
        let peer = pc.orchestrator().peer();
        let wanted = probe(8000);
        let ask = peer.show_probe(&wanted);
        let click = async {
            let id = shown(&mut ui).await;
            mac.probes().answer(id, ProbeAnswer::Seen).unwrap();
            id
        };
        let (outcome, id) = tokio::join!(ask, click);
        assert_eq!(outcome, Ok(ProbeOutcome::Answered(ProbeAnswer::Seen)));
        assert_eq!(
            ui.recv().await.unwrap(),
            tandem_proto::ProbeEvent::Hide { id }
        );
    }

    /// Sin clic, la Mac cuenta la sonda como no vista en su plazo y la quita sola.
    #[tokio::test(start_paused = true)]
    async fn without_a_click_the_peer_answers_unseen_in_time() {
        let dir = tempfile::tempdir().unwrap();
        let (pc, _) = agent(dir.path(), Machine::Pc, FakeSteps::default());
        let (mac, _) = agent(dir.path(), Machine::Mac, FakeSteps::default());
        let _serve = link(&pc, &mac);
        let mut ui = mac.subscribe_probes();
        let start = tokio::time::Instant::now();
        let outcome = pc.orchestrator().peer().show_probe(&probe(8000)).await;
        assert_eq!(outcome, Ok(ProbeOutcome::NoAnswer));
        assert_eq!(start.elapsed(), Duration::from_secs(8));
        let id = shown(&mut ui).await;
        assert_eq!(
            ui.recv().await.unwrap(),
            tandem_proto::ProbeEvent::Hide { id }
        );
    }

    /// Si la PC deja de esperar (el motor venció o se canceló), la sonda de la Mac se
    /// quita en ese momento, sin esperar su plazo.
    #[tokio::test(start_paused = true)]
    async fn giving_up_hides_the_peers_probe() {
        let dir = tempfile::tempdir().unwrap();
        let (pc, _) = agent(dir.path(), Machine::Pc, FakeSteps::default());
        let (mac, _) = agent(dir.path(), Machine::Mac, FakeSteps::default());
        let _serve = link(&pc, &mac);
        let mut ui = mac.subscribe_probes();
        let peer = pc.orchestrator().peer();
        let wanted = probe(120_000);
        let start = tokio::time::Instant::now();
        let gave_up = tokio::time::timeout(Duration::from_secs(2), peer.show_probe(&wanted)).await;
        assert!(gave_up.is_err());
        let id = shown(&mut ui).await;
        assert_eq!(
            ui.recv().await.unwrap(),
            tandem_proto::ProbeEvent::Hide { id }
        );
        assert!(start.elapsed() < Duration::from_secs(3));
        // Un clic tardío ya no llega a nadie.
        assert!(mac.probes().answer(id, ProbeAnswer::Seen).is_err());
    }

    /// Una sonda esperando el clic no frena los pasos que pide el mismo agente.
    #[tokio::test(start_paused = true)]
    async fn a_pending_probe_does_not_block_steps() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, _) = agent(dir.path(), Machine::Mac, FakeSteps::default());
        let (pc, pc_steps) = agent(dir.path(), Machine::Pc, FakeSteps::default());
        let _serve = link(&mac, &pc);
        let mut ui = pc.subscribe_probes();
        let peer = mac.orchestrator().peer();
        let wanted = probe(120_000);
        let pending = peer.show_probe(&wanted);
        tokio::pin!(pending);
        let step = async {
            shown(&mut ui).await;
            peer.execute(&switch_34_to_pc()).await
        };
        tokio::select! {
            r = step => assert_eq!(r, Ok(())),
            _ = &mut pending => panic!("la sonda no debería contestarse sola"),
        }
        assert_eq!(pc_steps.steps(), [switch_34_to_pc()]);
    }

    /// Sin UI en la otra máquina la sonda falla enseguida: quien calibra pregunta aquí.
    #[tokio::test(start_paused = true)]
    async fn a_peer_without_a_window_fails_the_probe() {
        let dir = tempfile::tempdir().unwrap();
        let (pc, _) = agent(dir.path(), Machine::Pc, FakeSteps::default());
        let (mac, _) = agent(dir.path(), Machine::Mac, FakeSteps::default());
        let _serve = link(&pc, &mac);
        let outcome = pc.orchestrator().peer().show_probe(&probe(8000)).await;
        assert_eq!(
            outcome,
            Ok(ProbeOutcome::Failed(crate::calibration::NO_UI.into()))
        );
    }

    /// El `Confirmer` real: lo de la entrada propia se muestra aquí, lo de la otra en la
    /// otra máquina, y si ella no puede mostrarlo (sin ventana) se pregunta aquí.
    #[tokio::test(start_paused = true)]
    async fn the_confirmer_routes_each_probe_to_the_machine_that_should_see_it() {
        use tandem_display::calibrate::{Confirmer, Probe, ProbeKind};
        use tandem_display::ddc::MonitorInfo;
        let dir = tempfile::tempdir().unwrap();
        let (pc, _) = agent(dir.path(), Machine::Pc, FakeSteps::default());
        let (mac, _) = agent(dir.path(), Machine::Mac, FakeSteps::default());
        let _serve = link(&pc, &mac);
        let confirmer = crate::AgentConfirmer::new(pc.clone(), Input::DisplayPort);
        let probe = |input| Probe {
            monitor: MonitorInfo {
                index: 0,
                description: "Monitor PnP genérico".into(),
                device: String::new(),
                edid: None,
            },
            input,
            kind: ProbeKind::Candidate { code: 0x0F },
            timeout: Duration::from_secs(8),
        };
        let mut pc_ui = pc.subscribe_probes();
        use tandem_display::calibrate::Answer;
        use tandem_proto::{ProbeEvent, ProbePlace};

        /// Las dos sondas que se muestran a la vez en la PC (pregunta y pantalla completa),
        /// en el orden que lleguen.
        async fn shown(
            ui: &mut tokio::sync::broadcast::Receiver<ProbeEvent>,
            n: usize,
        ) -> Vec<(u64, ProbePlace)> {
            let mut v = Vec::new();
            while v.len() < n {
                if let ProbeEvent::Show { id, place, probe } = ui.recv().await.unwrap() {
                    assert_eq!(
                        probe.machine,
                        if probe.input == Input::DisplayPort {
                            Machine::Pc
                        } else {
                            Machine::Mac
                        }
                    );
                    v.push((id, place));
                }
            }
            v
        }

        // DisplayPort es de la PC: su propia UI a pantalla completa y, a la vez, la
        // pregunta de opción múltiple. Un clic en la pantalla completa basta.
        let dp = probe(Input::DisplayPort);
        let (seen, ()) = tokio::join!(confirmer.confirm(&dp), async {
            let both = shown(&mut pc_ui, 2).await;
            let places: Vec<_> = both.iter().map(|(_, p)| p.clone()).collect();
            assert!(places.contains(&ProbePlace::Question), "{places:?}");
            assert!(
                places.contains(&ProbePlace::Monitor { screen: None }),
                "{places:?}"
            );
            let (id, _) = both
                .iter()
                .find(|(_, p)| *p != ProbePlace::Question)
                .unwrap();
            pc.probes().answer(*id, ProbeAnswer::Seen).unwrap();
        });
        assert_eq!(seen, Ok(Some(Answer::sees(Input::DisplayPort))));

        // HDMI es de la Mac, que no tiene ventana abierta: queda la pregunta de la PC. El
        // usuario ve la PC: el código no llevó el monitor a HDMI.
        let hdmi = probe(Input::Hdmi);
        let (seen, ()) = tokio::join!(confirmer.confirm(&hdmi), async {
            let (id, place) = loop {
                let found = shown(&mut pc_ui, 1).await.remove(0);
                if found.1 == ProbePlace::Question {
                    break found;
                }
            };
            assert_eq!(place, ProbePlace::Question);
            pc.probes()
                .answer(id, ProbeAnswer::Machine(Machine::Pc))
                .unwrap();
        });
        assert_eq!(seen, Ok(Some(Answer::sees(Input::DisplayPort))));

        // Negro y parar, desde la pregunta.
        for (answer, want) in [
            (ProbeAnswer::Black, Answer::black()),
            (ProbeAnswer::Stop, Answer::Stop),
            (
                ProbeAnswer::Machine(Machine::Mac),
                Answer::sees(Input::Hdmi),
            ),
        ] {
            let (got, ()) = tokio::join!(confirmer.confirm(&hdmi), async {
                let (id, _) = loop {
                    let found = shown(&mut pc_ui, 1).await.remove(0);
                    if found.1 == ProbePlace::Question {
                        break found;
                    }
                };
                pc.probes().answer(id, answer).unwrap();
            });
            assert_eq!(got, Ok(Some(want)));
        }

        // Sin ventana en ninguna de las dos, la calibración no puede seguir.
        drop(pc_ui);
        let err = confirmer.confirm(&hdmi).await.unwrap_err();
        assert_eq!(err.0, crate::calibration::NO_UI);
    }

    /// Sin par, `NoPeer` no tiene a quién pedirle la sonda.
    #[tokio::test]
    async fn without_a_peer_there_is_no_remote_probe() {
        let dir = tempfile::tempdir().unwrap();
        let (pc, _) = agent(dir.path(), Machine::Pc, FakeSteps::default());
        assert!(
            pc.orchestrator()
                .peer()
                .show_probe(&probe(8000))
                .await
                .is_err()
        );
    }

    #[test]
    fn peer_must_be_the_other_machine() {
        assert!(check_peer(Machine::Mac, Peer::Agent(Machine::Pc)).is_ok());
        assert!(check_peer(Machine::Mac, Peer::Agent(Machine::Mac)).is_err());
        assert!(check_peer(Machine::Pc, Peer::Ui).is_err());
    }

    /// El paso remoto se cuelga en la PC: la Mac lo da por vencido a los 10 s y deshace.
    #[tokio::test(start_paused = true)]
    async fn remote_timeout_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let attach_pc = Step::Attach {
            machine: Machine::Pc,
            monitor: "m34".into(),
        };
        let (mac, _) = agent(dir.path(), Machine::Mac, FakeSteps::default());
        let (pc, pc_steps) = agent(
            dir.path(),
            Machine::Pc,
            FakeSteps {
                delays: vec![(attach_pc.clone(), Duration::from_secs(3600))],
                ..Default::default()
            },
        );
        let _serve = link(&mac, &pc);
        let cfg = mac.config().desk.clone();
        let mut state = mac.state();
        state.pc_on = true;
        let initial = state.clone();
        let mode = Mode::Shared([("m34".into(), Machine::Pc)].into());
        let plan = Plan::to_mode(&cfg, &state, &mode).unwrap();
        let err = mac
            .orchestrator()
            .run(1, &cfg, &mut state, &plan)
            .await
            .unwrap_err();
        assert_eq!(
            err,
            TxnError::RolledBack {
                step: attach_pc.clone(),
                error: format!("{}: sin respuesta en 10 s", describe(&attach_pc)),
            }
        );
        assert_eq!(state, initial);
        // La PC deshizo el paso colgado (incierto) y el cambio de entrada después de
        // él, no antes.
        assert_eq!(
            pc_steps.steps(),
            [
                switch_34_to_pc(),
                attach_pc.clone(),
                attach_pc.inverse(&cfg).unwrap(),
                switch_34_to_pc().inverse(&cfg).unwrap()
            ]
        );
    }

    /// QA ronda 2: mientras la PC hace pasos para la Mac no puede empezar un cambio
    /// propio; el estado final de la Mac la libera.
    #[tokio::test]
    async fn the_peers_final_state_ends_the_loan() {
        let dir = tempfile::tempdir().unwrap();
        let (pc, pc_steps) = agent(dir.path(), Machine::Pc, FakeSteps::default());
        let (a, b) = duplex(64 * 1024);
        let _task = tokio::spawn(serve(Framed::new(b), pc.clone()));
        let mut f = Framed::new(a);
        f.send(&ControlMsg::ExecuteStep {
            id: 1,
            step: switch_34_to_pc(),
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
        assert_eq!(pc_steps.steps(), [switch_34_to_pc()]);
        assert_eq!(pc.orchestrator().try_begin(), None);

        let mut s = pc.state();
        s.pc_on = true;
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
}
