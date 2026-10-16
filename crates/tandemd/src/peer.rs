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

use tandem_core::{Machine, State, Step};
use tandem_link::transport::{Connection, ControlStream, TransportError};
use tandem_proto::{ControlMsg, Hello, Peer};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::agent::Agent;
use crate::ipc::Framed;
use crate::orchestrator::{PEER_LOST, PeerLink, StepFuture};
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
    State(State),
}

/// Pide pasos al otro agente por un stream propio. Al soltarlo se cierra el stream.
pub struct PeerClient {
    tx: mpsc::UnboundedSender<Outgoing>,
    task: JoinHandle<()>,
}

impl PeerClient {
    /// Toma el canal y lo atiende en una tarea propia. Llamar dentro de un runtime de tokio.
    pub fn spawn<C: ControlChannel>(chan: C) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        PeerClient {
            tx,
            task: tokio::spawn(client_loop(chan, rx)),
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
        let _ = self.tx.send(Outgoing::State(state.clone()));
    }
}

async fn client_loop<C: ControlChannel>(mut chan: C, mut rx: mpsc::UnboundedReceiver<Outgoing>) {
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
            Outgoing::State(state) => chan.send(&ControlMsg::State(state)).await,
        };
        if let Err(e) = sent {
            tracing::debug!("stream de pedidos al otro agente: {e}");
            return;
        }
    }
}

/// Atiende un stream abierto por el otro agente hasta que lo cierre.
///
/// Los pasos se ejecutan de a uno y en orden: así el inverso de un paso que venció
/// en el otro lado nunca corre antes que el paso mismo.
pub async fn serve<C: ControlChannel>(mut chan: C, agent: Arc<Agent>) -> Result<(), String> {
    while let Some(msg) = chan.recv().await? {
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
            ControlMsg::Pong { .. } | ControlMsg::StepResult { .. } => {}
        }
    }
    Ok(())
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
        let agent = agent.clone();
        tokio::spawn(async move {
            if let Err(e) = serve(stream, agent).await {
                tracing::debug!("stream del otro agente terminado: {e}");
            }
        });
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

    use tandem_core::{Input, Mode, Plan, TxnError};
    use tokio::io::duplex;

    use super::*;
    use crate::config::fixtures;
    use crate::orchestrator::describe;
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
}
