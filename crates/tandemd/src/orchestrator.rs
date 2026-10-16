//! Orquestador: ejecuta un [`Plan`] como transacción repartiendo cada paso a la
//! máquina que lo tiene que hacer (DESIGN §3.2 y §5).
//!
//! Cada paso va a esta máquina ([`LocalSteps`]) o al otro agente por el plano de
//! control ([`PeerLink`]), siempre con un tiempo máximo ([`StepTimeouts`]). La vuelta
//! atrás la hace [`tandem_core::run`]; aquí solo se despacha y se publica el avance
//! como [`TxnEvent`] para la UI.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use tandem_core::{
    BuiltinDisplay, Config, Executor, Machine, Plan, State, Step, TxnError, TxnReport,
};
use tandem_proto::{TxnEvent, UndoFailure};
use tokio::sync::broadcast;

/// Lo que responde un paso que esta máquina todavía no sabe hacer. Dispara la vuelta atrás.
pub const STEP_NOT_IMPLEMENTED: &str = "no implementado";
/// El paso le toca al otro agente y no hay conexión con él.
pub const PEER_NOT_CONNECTED: &str = "el otro agente no está conectado";
/// Se cortó la conexión con el otro agente con un paso pendiente.
pub const PEER_LOST: &str = "se perdió la conexión con el otro agente";

/// Resultado de un paso: el error es un texto para mostrar en la UI.
pub type StepFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

/// Los pasos que esta máquina hace por sí misma (DDC, topología, energía...).
pub trait LocalSteps: Send + Sync {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a>;
}

/// Ejecutor de una máquina sin backends todavía: todo paso responde [`STEP_NOT_IMPLEMENTED`].
#[derive(Debug, Default, Clone, Copy)]
pub struct Unimplemented;

impl LocalSteps for Unimplemented {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
        Box::pin(async move { Err(format!("{}: {STEP_NOT_IMPLEMENTED}", describe(step))) })
    }
}

/// El otro agente, visto desde el orquestador.
pub trait PeerLink: Send + Sync {
    /// Pide al otro agente un paso y espera su resultado.
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a>;
    /// Le avisa del estado final de una transacción, sin esperar respuesta.
    fn publish_state(&self, state: &State);
}

/// Sin conexión con el otro agente.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoPeer;

impl PeerLink for NoPeer {
    fn execute<'a>(&'a self, _step: &'a Step) -> StepFuture<'a> {
        Box::pin(async { Err(PEER_NOT_CONNECTED.to_owned()) })
    }

    fn publish_state(&self, _state: &State) {}
}

/// Dónde se ejecuta un paso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    On(Machine),
    /// En las dos máquinas: primero en la que orquesta, después en la otra.
    Both,
}

/// La máquina que sabe hacer cada paso.
pub fn target(step: &Step) -> Target {
    match step {
        // La PC apagada no puede encenderse sola: el Wake-on-LAN sale de la Mac.
        Step::PowerOnPc => Target::On(Machine::Mac),
        Step::PowerOffPc => Target::On(Machine::Pc),
        // Las entradas las cambia la PC por DDC sobre DisplayPort (DESIGN §8).
        Step::SwitchInput { .. } => Target::On(Machine::Pc),
        Step::Attach { machine, .. } | Step::Detach { machine, .. } => Target::On(*machine),
        Step::SetBuiltin { .. } => Target::On(Machine::Mac),
        // Cada motor de entrada necesita la geometría nueva, sea servidor o cliente.
        Step::UpdateInputGeometry => Target::Both,
    }
}

/// Texto corto de un paso, para errores y logs.
pub fn describe(step: &Step) -> String {
    match step {
        Step::PowerOnPc => "encender la PC".to_owned(),
        Step::PowerOffPc => "apagar la PC".to_owned(),
        Step::SwitchInput { monitor, to, .. } => format!("cambiar la entrada de {monitor} a {to}"),
        Step::Attach { machine, monitor } => {
            format!("agregar {monitor} al escritorio de {machine}")
        }
        Step::Detach { machine, monitor } => {
            format!("quitar {monitor} del escritorio de {machine}")
        }
        Step::SetBuiltin { to, .. } => format!(
            "pantalla integrada {}",
            match to {
                BuiltinDisplay::Off => "apagada",
                BuiltinDisplay::Mirror => "en espejo",
                BuiltinDisplay::Extend => "extendida",
            }
        ),
        Step::UpdateInputGeometry => "recalcular la geometría del teclado y mouse".to_owned(),
    }
}

/// Tiempo máximo por paso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepTimeouts {
    /// Encender la PC incluye esperar a que su agente responda.
    pub power_on: Duration,
    /// Todos los demás pasos.
    pub step: Duration,
}

impl Default for StepTimeouts {
    fn default() -> Self {
        StepTimeouts {
            power_on: Duration::from_secs(120),
            step: Duration::from_secs(10),
        }
    }
}

impl StepTimeouts {
    pub fn for_step(&self, step: &Step) -> Duration {
        match step {
            Step::PowerOnPc => self.power_on,
            _ => self.step,
        }
    }
}

/// Avisos de transacción que se guardan para un suscriptor lento antes de que pierda alguno.
const EVENT_BUFFER: usize = 256;

/// Despacha pasos y publica el avance de las transacciones de un agente.
pub struct Orchestrator {
    me: Machine,
    local: Arc<dyn LocalSteps>,
    peer: RwLock<Arc<dyn PeerLink>>,
    timeouts: StepTimeouts,
    events: broadcast::Sender<TxnEvent>,
    next_txn: AtomicU64,
    busy: AtomicBool,
}

impl std::fmt::Debug for Orchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Orchestrator")
            .field("me", &self.me)
            .field("timeouts", &self.timeouts)
            .field("busy", &self.busy)
            .finish_non_exhaustive()
    }
}

impl Orchestrator {
    pub fn new(me: Machine, local: Arc<dyn LocalSteps>, timeouts: StepTimeouts) -> Self {
        Orchestrator {
            me,
            local,
            peer: RwLock::new(Arc::new(NoPeer)),
            timeouts,
            events: broadcast::channel(EVENT_BUFFER).0,
            next_txn: AtomicU64::new(1),
            busy: AtomicBool::new(false),
        }
    }

    /// Cambia la conexión con el otro agente. Los pasos ya en curso siguen con la anterior.
    pub fn set_peer(&self, peer: Arc<dyn PeerLink>) {
        *self.peer.write().unwrap_or_else(PoisonError::into_inner) = peer;
    }

    pub fn peer(&self) -> Arc<dyn PeerLink> {
        self.peer
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TxnEvent> {
        self.events.subscribe()
    }

    /// Reserva el orquestador para una transacción; `None` si ya hay una en curso.
    pub fn try_begin(&self) -> Option<u64> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| self.next_txn.fetch_add(1, Ordering::Relaxed))
    }

    /// Libera lo reservado con [`Orchestrator::try_begin`].
    pub fn end(&self) {
        self.busy.store(false, Ordering::Release);
    }

    fn emit(&self, event: TxnEvent) {
        // Sin UI conectada no hay a quién avisar, y eso no es un error.
        let _ = self.events.send(event);
    }

    /// Ejecuta `plan` sobre `state` publicando el avance; el desenlace lo publica
    /// [`Orchestrator::finish`], cuando el estado ya se guardó.
    pub async fn run(
        &self,
        txn: u64,
        cfg: &Config,
        state: &mut State,
        plan: &Plan,
    ) -> Result<TxnReport, TxnError> {
        self.emit(TxnEvent::Started {
            txn,
            steps: plan.steps.clone(),
        });
        let mut d = Dispatcher {
            orch: self,
            txn,
            next: 0,
            failed: false,
        };
        tandem_core::run(cfg, state, plan, &mut d).await
    }

    /// Publica el desenlace a la UI y el estado final al otro agente.
    pub fn finish(&self, txn: u64, state: &State, outcome: &Result<TxnReport, TxnError>) {
        let event = match outcome {
            Ok(_) => TxnEvent::Committed {
                txn,
                state: state.clone(),
            },
            Err(TxnError::RolledBack { .. }) => TxnEvent::RolledBack {
                txn,
                state: state.clone(),
            },
            Err(TxnError::Inconsistent { undo_failures, .. }) => TxnEvent::Inconsistent {
                txn,
                undo_failures: undo_failures
                    .iter()
                    .map(|(step, error)| UndoFailure {
                        step: step.clone(),
                        error: error.clone(),
                    })
                    .collect(),
            },
        };
        self.emit(event);
        self.peer().publish_state(state);
    }

    /// Un paso con su tiempo máximo, en la máquina que le toca.
    pub async fn dispatch(&self, step: &Step) -> Result<(), String> {
        let limit = self.timeouts.for_step(step);
        let work = async {
            match target(step) {
                Target::On(m) if m == self.me => self.local.execute(step).await,
                Target::On(_) => self.remote(step).await,
                Target::Both => {
                    self.local.execute(step).await?;
                    self.remote(step).await
                }
            }
        };
        match tokio::time::timeout(limit, work).await {
            Ok(r) => r,
            Err(_) => Err(format!(
                "{}: sin respuesta en {} s",
                describe(step),
                limit.as_secs()
            )),
        }
    }

    async fn remote(&self, step: &Step) -> Result<(), String> {
        let peer = self.peer();
        peer.execute(step)
            .await
            .map_err(|e| format!("{}: {e}", self.me.other()))
    }

    /// Un paso pedido por el otro agente: solo se aceptan los que le tocan a esta máquina.
    pub async fn execute_for_peer(&self, step: &Step) -> Result<(), String> {
        match target(step) {
            Target::On(m) if m != self.me => {
                return Err(format!(
                    "{}: no le corresponde a {}",
                    describe(step),
                    self.me
                ));
            }
            _ => {}
        }
        let limit = self.timeouts.for_step(step);
        match tokio::time::timeout(limit, self.local.execute(step)).await {
            Ok(r) => r,
            Err(_) => Err(format!(
                "{}: sin respuesta en {} s",
                describe(step),
                limit.as_secs()
            )),
        }
    }
}

/// El [`Executor`] de una transacción: despacha y cuenta los pasos para la UI.
///
/// [`tandem_core::run`] llama primero a los pasos del plan en orden y, tras el primer
/// fallo, solo a los inversos: por eso alcanza con un contador.
struct Dispatcher<'a> {
    orch: &'a Orchestrator,
    txn: u64,
    next: u32,
    failed: bool,
}

impl Executor for Dispatcher<'_> {
    async fn execute(&mut self, step: &Step) -> Result<(), String> {
        if self.failed {
            let r = self.orch.dispatch(step).await;
            if let Err(e) = &r {
                tracing::error!(txn = self.txn, "no se pudo deshacer: {e}");
            }
            return r;
        }
        let (txn, index) = (self.txn, self.next);
        self.orch.emit(TxnEvent::StepStarted { txn, index });
        let r = self.orch.dispatch(step).await;
        match &r {
            Ok(()) => {
                self.orch.emit(TxnEvent::StepDone { txn, index });
                self.next += 1;
            }
            Err(error) => {
                tracing::warn!(txn, index, "falló un paso: {error}");
                self.orch.emit(TxnEvent::StepFailed {
                    txn,
                    index,
                    error: error.clone(),
                });
                self.orch.emit(TxnEvent::RollingBack { txn });
                self.failed = true;
            }
        }
        r
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use tandem_core::{Input, Mode};

    use super::*;
    use crate::config::fixtures;

    /// Pasos simulados: registran todo, fallan en los indicados y tardan lo que se les diga.
    #[derive(Default)]
    pub struct FakeSteps {
        pub log: Mutex<Vec<Step>>,
        pub fail_on: Vec<Step>,
        pub delays: Vec<(Step, Duration)>,
    }

    impl LocalSteps for FakeSteps {
        fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
            Box::pin(async move {
                self.log
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(step.clone());
                if let Some((_, d)) = self.delays.iter().find(|(s, _)| s == step) {
                    tokio::time::sleep(*d).await;
                }
                if self.fail_on.contains(step) {
                    Err("falla simulada".to_owned())
                } else {
                    Ok(())
                }
            })
        }
    }

    impl FakeSteps {
        pub fn steps(&self) -> Vec<Step> {
            self.log
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    /// El otro agente simulado sin red: otro [`FakeSteps`].
    pub struct FakePeer(pub Arc<FakeSteps>);

    impl PeerLink for FakePeer {
        fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
            self.0.execute(step)
        }
        fn publish_state(&self, _state: &State) {}
    }

    fn switch_34_to_pc() -> Step {
        Step::SwitchInput {
            monitor: "m34".into(),
            from: Machine::Mac,
            to: Machine::Pc,
            input: Input::DisplayPort,
        }
    }

    fn mac(local: FakeSteps, peer: FakeSteps) -> (Orchestrator, Arc<FakeSteps>, Arc<FakeSteps>) {
        let local = Arc::new(local);
        let peer = Arc::new(peer);
        let o = Orchestrator::new(Machine::Mac, local.clone(), StepTimeouts::default());
        o.set_peer(Arc::new(FakePeer(peer.clone())));
        (o, local, peer)
    }

    fn drain(rx: &mut broadcast::Receiver<TxnEvent>) -> Vec<TxnEvent> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    #[test]
    fn routes_each_step_to_its_machine() {
        assert_eq!(target(&Step::PowerOnPc), Target::On(Machine::Mac));
        assert_eq!(target(&Step::PowerOffPc), Target::On(Machine::Pc));
        assert_eq!(target(&switch_34_to_pc()), Target::On(Machine::Pc));
        for machine in [Machine::Mac, Machine::Pc] {
            let monitor = "m34".into();
            assert_eq!(
                target(&Step::Attach {
                    machine,
                    monitor: "m34".into()
                }),
                Target::On(machine)
            );
            assert_eq!(
                target(&Step::Detach { machine, monitor }),
                Target::On(machine)
            );
        }
        assert_eq!(
            target(&Step::SetBuiltin {
                from: BuiltinDisplay::Off,
                to: BuiltinDisplay::Extend
            }),
            Target::On(Machine::Mac)
        );
        assert_eq!(target(&Step::UpdateInputGeometry), Target::Both);
    }

    #[test]
    fn power_on_gets_the_long_timeout() {
        let t = StepTimeouts::default();
        assert_eq!(t.for_step(&Step::PowerOnPc), Duration::from_secs(120));
        assert_eq!(t.for_step(&switch_34_to_pc()), Duration::from_secs(10));
        assert_eq!(t.for_step(&Step::PowerOffPc), Duration::from_secs(10));
    }

    #[tokio::test]
    async fn dispatches_locally_remotely_or_both() {
        let (o, local, peer) = mac(FakeSteps::default(), FakeSteps::default());
        let attach_mac = Step::Attach {
            machine: Machine::Mac,
            monitor: "m34".into(),
        };
        for s in [&attach_mac, &switch_34_to_pc(), &Step::UpdateInputGeometry] {
            o.dispatch(s).await.unwrap();
        }
        assert_eq!(local.steps(), [attach_mac, Step::UpdateInputGeometry]);
        assert_eq!(peer.steps(), [switch_34_to_pc(), Step::UpdateInputGeometry]);
    }

    #[tokio::test]
    async fn remote_errors_name_the_machine() {
        let (o, _, _) = mac(
            FakeSteps::default(),
            FakeSteps {
                fail_on: vec![switch_34_to_pc()],
                ..Default::default()
            },
        );
        let e = o.dispatch(&switch_34_to_pc()).await.unwrap_err();
        assert_eq!(e, "pc: falla simulada");
    }

    #[tokio::test]
    async fn without_peer_remote_steps_fail_clearly() {
        let o = Orchestrator::new(
            Machine::Mac,
            Arc::new(Unimplemented),
            StepTimeouts::default(),
        );
        let e = o.dispatch(&switch_34_to_pc()).await.unwrap_err();
        assert!(e.contains(PEER_NOT_CONNECTED), "{e}");
        let e = o.dispatch(&Step::PowerOnPc).await.unwrap_err();
        assert_eq!(e, format!("encender la PC: {STEP_NOT_IMPLEMENTED}"));
    }

    #[tokio::test]
    async fn refuses_peer_requests_for_the_peers_own_steps() {
        let (o, local, _) = mac(FakeSteps::default(), FakeSteps::default());
        assert!(o.execute_for_peer(&switch_34_to_pc()).await.is_err());
        o.execute_for_peer(&Step::PowerOnPc).await.unwrap();
        o.execute_for_peer(&Step::UpdateInputGeometry)
            .await
            .unwrap();
        assert_eq!(local.steps(), [Step::PowerOnPc, Step::UpdateInputGeometry]);
    }

    #[test]
    fn only_one_transaction_at_a_time() {
        let o = Orchestrator::new(
            Machine::Mac,
            Arc::new(Unimplemented),
            StepTimeouts::default(),
        );
        let a = o.try_begin().unwrap();
        assert_eq!(o.try_begin(), None);
        o.end();
        let b = o.try_begin().unwrap();
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn publishes_progress_of_a_committed_transaction() {
        let cfg = fixtures::desk().desk;
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        let mut rx = o.subscribe();
        let mut state = State::initial(&cfg);
        let mode = Mode::Shared([("m34".into(), Machine::Pc)].into());
        let plan = Plan::to_mode(&cfg, &state, &mode).unwrap();
        let outcome = o.run(7, &cfg, &mut state, &plan).await;
        o.finish(7, &state, &outcome);
        assert!(outcome.is_ok());

        let mut want = vec![TxnEvent::Started {
            txn: 7,
            steps: plan.steps.clone(),
        }];
        for index in 0..plan.steps.len() as u32 {
            want.push(TxnEvent::StepStarted { txn: 7, index });
            want.push(TxnEvent::StepDone { txn: 7, index });
        }
        want.push(TxnEvent::Committed { txn: 7, state });
        assert_eq!(drain(&mut rx), want);
    }

    #[tokio::test]
    async fn failed_step_publishes_failure_and_rollback() {
        let cfg = fixtures::desk().desk;
        let attach_pc = Step::Attach {
            machine: Machine::Pc,
            monitor: "m34".into(),
        };
        let (o, _, peer) = mac(
            FakeSteps::default(),
            FakeSteps {
                fail_on: vec![attach_pc.clone()],
                ..Default::default()
            },
        );
        let mut rx = o.subscribe();
        let mut state = State::initial(&cfg);
        state.pc_on = true;
        let initial = state.clone();
        let mode = Mode::Shared([("m34".into(), Machine::Pc)].into());
        let plan = Plan::to_mode(&cfg, &state, &mode).unwrap();
        let outcome = o.run(1, &cfg, &mut state, &plan).await;
        o.finish(1, &state, &outcome);
        assert!(matches!(outcome, Err(TxnError::RolledBack { .. })));
        assert_eq!(state, initial);
        // El cambio de entrada se deshizo en la PC.
        assert_eq!(
            peer.steps().last(),
            switch_34_to_pc().inverse(&cfg).as_ref()
        );

        let events = drain(&mut rx);
        assert_eq!(
            events[events.len() - 3..],
            [
                TxnEvent::StepFailed {
                    txn: 1,
                    index: 1,
                    error: "pc: falla simulada".into()
                },
                TxnEvent::RollingBack { txn: 1 },
                TxnEvent::RolledBack {
                    txn: 1,
                    state: initial
                },
            ]
        );
    }

    #[tokio::test]
    async fn failed_undo_publishes_inconsistent() {
        let cfg = fixtures::desk().desk;
        let back = switch_34_to_pc().inverse(&cfg).unwrap();
        let (o, _, _) = mac(
            FakeSteps {
                fail_on: vec![Step::UpdateInputGeometry],
                ..Default::default()
            },
            FakeSteps {
                fail_on: vec![back.clone()],
                ..Default::default()
            },
        );
        let mut rx = o.subscribe();
        let mut state = State::initial(&cfg);
        state.pc_on = true;
        let mode = Mode::Shared([("m34".into(), Machine::Pc)].into());
        let plan = Plan::to_mode(&cfg, &state, &mode).unwrap();
        let outcome = o.run(2, &cfg, &mut state, &plan).await;
        o.finish(2, &state, &outcome);
        let last = drain(&mut rx).pop().unwrap();
        assert_eq!(
            last,
            TxnEvent::Inconsistent {
                txn: 2,
                undo_failures: vec![UndoFailure {
                    step: back,
                    error: "pc: falla simulada".into()
                }]
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn step_timeout_rolls_back() {
        let cfg = fixtures::desk().desk;
        let detach_mac = Step::Detach {
            machine: Machine::Mac,
            monitor: "m34".into(),
        };
        // Quitar el 34" de la Mac se cuelga más que el tope de 10 s.
        let (o, local, peer) = mac(
            FakeSteps {
                delays: vec![(detach_mac.clone(), Duration::from_secs(3600))],
                ..Default::default()
            },
            FakeSteps::default(),
        );
        let mut state = State::initial(&cfg);
        state.pc_on = true;
        let initial = state.clone();
        let mode = Mode::Shared([("m34".into(), Machine::Pc)].into());
        let plan = Plan::to_mode(&cfg, &state, &mode).unwrap();
        let start = tokio::time::Instant::now();
        let err = o.run(3, &cfg, &mut state, &plan).await.unwrap_err();
        assert_eq!(
            err,
            TxnError::RolledBack {
                step: detach_mac.clone(),
                error: format!("{}: sin respuesta en 10 s", describe(&detach_mac)),
            }
        );
        assert_eq!(start.elapsed(), Duration::from_secs(10));
        assert_eq!(state, initial);
        assert_eq!(local.steps(), [detach_mac]);
        // En la PC: entrada, agregar, y después quitar y devolver la entrada.
        let back = switch_34_to_pc().inverse(&cfg).unwrap();
        assert_eq!(
            peer.steps()[2..],
            [
                Step::Detach {
                    machine: Machine::Pc,
                    monitor: "m34".into()
                },
                back
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn power_on_may_take_longer_than_other_steps() {
        let cfg = fixtures::desk().desk;
        // 60 s para encender la PC está dentro de los 120 s.
        let (o, _, _) = mac(
            FakeSteps {
                delays: vec![(Step::PowerOnPc, Duration::from_secs(60))],
                ..Default::default()
            },
            FakeSteps::default(),
        );
        let mut state = State::initial(&cfg);
        let plan = Plan::to_mode(&cfg, &state, &Mode::SoloPc).unwrap();
        assert_eq!(plan.steps[0], Step::PowerOnPc);
        o.run(4, &cfg, &mut state, &plan).await.unwrap();
        assert!(state.pc_on);

        // 121 s, no.
        let (o, _, _) = mac(
            FakeSteps {
                delays: vec![(Step::PowerOnPc, Duration::from_secs(121))],
                ..Default::default()
            },
            FakeSteps::default(),
        );
        let mut state = State::initial(&cfg);
        let err = o.run(5, &cfg, &mut state, &plan).await.unwrap_err();
        assert!(matches!(
            err,
            TxnError::RolledBack {
                step: Step::PowerOnPc,
                ..
            }
        ));
        assert_eq!(state, State::initial(&cfg));
    }
}
