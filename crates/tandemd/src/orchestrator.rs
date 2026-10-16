//! Orquestador: ejecuta un [`Plan`] como transacción repartiendo cada paso a la
//! máquina que lo tiene que hacer (DESIGN §3.2 y §5).
//!
//! Cada paso va a esta máquina ([`LocalSteps`]) o al otro agente por el plano de
//! control ([`PeerLink`]), siempre con un tiempo máximo ([`StepTimeouts`]). La vuelta
//! atrás la hace [`tandem_core::run`]; aquí solo se despacha y se publica el avance
//! como [`TxnEvent`] para la UI.
//!
//! Dos pasos tienen reglas propias: [`Step::PowerOnPc`] es mandar el Wake-on-LAN
//! (repetido cada [`WOL_RESEND`]) y esperar a que el agente de la PC se conecte, y
//! [`Step::PowerOffPc`] solo se despacha si el plan ya devolvió todos los monitores
//! a la Mac (si no, la Mac se quedaría sin pantallas).
//!
//! Un paso que falla por tiempo o porque se cortó la conexión con el otro agente
//! pudo haberse hecho igual (se perdió la respuesta, no el pedido). Ese paso incierto
//! también se deshace, antes que los demás; los pasos son idempotentes, así que
//! deshacer uno que no llegó a hacerse no cambia nada.
//!
//! Las dos máquinas pueden orquestar, pero de a una: la que hace pasos para la otra
//! queda prestada hasta recibir su estado final, y la que orquesta rechaza los pasos
//! ajenos.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use tandem_core::{
    BuiltinDisplay, Config, Executor, Machine, Plan, State, Step, TxnError, TxnReport,
};
use tandem_proto::{TxnEvent, UndoFailure};
use tokio::sync::{broadcast, watch};

/// Lo que responde un paso que esta máquina todavía no sabe hacer. Dispara la vuelta atrás.
pub const STEP_NOT_IMPLEMENTED: &str = "no implementado";
/// El paso le toca al otro agente y no hay conexión con él.
pub const PEER_NOT_CONNECTED: &str = "el otro agente no está conectado";
/// Se cortó la conexión con el otro agente con un paso pendiente.
pub const PEER_LOST: &str = "se perdió la conexión con el otro agente";
/// El otro agente pidió un paso mientras esta máquina orquesta su propio cambio.
pub const BUSY_HERE: &str = "esta máquina está en medio de otro cambio";

/// Cada cuánto se repite el Wake-on-LAN mientras se espera a la PC: un paquete UDP
/// se puede perder, y la tarjeta tarda en quedar lista tras cortarse la luz.
pub const WOL_RESEND: Duration = Duration::from_secs(5);

/// Resultado de un paso: el error es un texto para mostrar en la UI.
pub type StepFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

/// Los pasos que esta máquina hace por sí misma (DDC, topología, energía...).
///
/// [`Step::PowerOnPc`] aquí es solo mandar un Wake-on-LAN: esperar a la PC y
/// repetir el envío lo hace el [`Orchestrator`].
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
    /// Si hay conexión con el otro agente: encender la PC termina cuando pasa a `true`.
    connected: watch::Sender<bool>,
    timeouts: StepTimeouts,
    events: broadcast::Sender<TxnEvent>,
    next_txn: AtomicU64,
    busy: AtomicBool,
    /// Hasta cuándo esta máquina está prestada a una transacción del otro agente.
    /// Las dos máquinas tocan el mismo puesto: mientras una orquesta, la otra no
    /// puede empezar un cambio propio, y una que orquesta no acepta pasos ajenos.
    /// Las dos comprobaciones van bajo este candado para que no se crucen.
    peer_lease: Mutex<Option<tokio::time::Instant>>,
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
            connected: watch::Sender::new(false),
            timeouts,
            events: broadcast::channel(EVENT_BUFFER).0,
            next_txn: AtomicU64::new(1),
            busy: AtomicBool::new(false),
            peer_lease: Mutex::new(None),
        }
    }

    /// Cambia la conexión con el otro agente. Los pasos ya en curso siguen con la anterior.
    pub fn set_peer(&self, peer: Arc<dyn PeerLink>) {
        *self.peer.write().unwrap_or_else(PoisonError::into_inner) = peer;
        self.connected.send_replace(true);
    }

    /// Se perdió la conexión con el otro agente (por ejemplo, la PC se apagó).
    pub fn clear_peer(&self) {
        *self.peer.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(NoPeer);
        self.connected.send_replace(false);
    }

    /// Si hay conexión con el otro agente, y avisos cuando cambia.
    pub fn peer_connected(&self) -> watch::Receiver<bool> {
        self.connected.subscribe()
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

    /// Reserva el orquestador para una transacción; `None` si ya hay una en curso,
    /// propia o del otro agente.
    pub fn try_begin(&self) -> Option<u64> {
        let lease = self.lease();
        if lease.is_some_and(|until| tokio::time::Instant::now() < until) {
            return None;
        }
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| self.next_txn.fetch_add(1, Ordering::Relaxed))
    }

    fn lease(&self) -> std::sync::MutexGuard<'_, Option<tokio::time::Instant>> {
        self.peer_lease
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// La transacción del otro agente terminó (llegó su estado final).
    pub fn release_peer_lease(&self) {
        *self.lease() = None;
    }

    /// Cuánto dura el préstamo tras cada paso del otro agente: lo más que puede
    /// tardar él en pedir el siguiente (un paso propio con su tiempo máximo, el
    /// más largo es encender la PC) más un paso nuestro. Si se pierde su estado
    /// final, la máquina queda libre a lo sumo tras esto.
    fn lease_length(&self) -> Duration {
        self.timeouts.power_on + self.timeouts.step
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
            cfg,
            txn,
            next: 0,
            failed: false,
            state: state.clone(),
            uncertain_undo_failures: Vec::new(),
        };
        let outcome = tandem_core::run(cfg, state, plan, &mut d).await;
        if d.uncertain_undo_failures.is_empty() {
            return outcome;
        }
        // No se pudo deshacer el paso incierto: el estado real no se sabe.
        let mut undo_failures = d.uncertain_undo_failures;
        match outcome {
            Err(TxnError::RolledBack { step, error }) => Err(TxnError::Inconsistent {
                step,
                error,
                undo_failures,
            }),
            Err(TxnError::Inconsistent {
                step,
                error,
                undo_failures: rest,
            }) => {
                undo_failures.extend(rest);
                Err(TxnError::Inconsistent {
                    step,
                    error,
                    undo_failures,
                })
            }
            ok => ok,
        }
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
        self.attempt(step).await.map_err(|f| f.error)
    }

    async fn attempt(&self, step: &Step) -> Result<(), StepFailure> {
        let limit = self.timeouts.for_step(step);
        let work = async {
            match target(step) {
                Target::On(m) if m == self.me => {
                    self.run_local(step).await.map_err(StepFailure::certain)
                }
                Target::On(_) => self.remote(step).await,
                Target::Both => {
                    self.run_local(step).await.map_err(StepFailure::certain)?;
                    self.remote(step).await
                }
            }
        };
        match tokio::time::timeout(limit, work).await {
            Ok(r) => r,
            Err(_) => Err(StepFailure {
                error: format!("{}: sin respuesta en {} s", describe(step), limit.as_secs()),
                uncertain: true,
            }),
        }
    }

    async fn run_local(&self, step: &Step) -> Result<(), String> {
        match step {
            Step::PowerOnPc => self.power_on_pc().await,
            _ => self.local.execute(step).await,
        }
    }

    /// Wake-on-LAN hasta que el agente de la PC se conecta. El tope lo pone quien llama
    /// (`power_on_timeout_secs`); un error al mandar el paquete corta enseguida.
    async fn power_on_pc(&self) -> Result<(), String> {
        let mut connected = self.connected.subscribe();
        loop {
            self.local.execute(&Step::PowerOnPc).await?;
            tokio::select! {
                // El emisor vive en `self`: la espera no puede quedarse sin emisor.
                r = connected.wait_for(|c| *c) => {
                    return r.map(|_| ()).map_err(|_| PEER_LOST.to_owned());
                }
                () = tokio::time::sleep(WOL_RESEND) => {
                    tracing::debug!("la PC todavía no responde: se repite el Wake-on-LAN");
                }
            }
        }
    }

    async fn remote(&self, step: &Step) -> Result<(), StepFailure> {
        let peer = self.peer();
        peer.execute(step).await.map_err(|e| StepFailure {
            uncertain: e == PEER_LOST,
            error: format!("{}: {e}", self.me.other()),
        })
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
        {
            let mut lease = self.lease();
            if self.busy.load(Ordering::Acquire) {
                return Err(format!("{}: {BUSY_HERE}", describe(step)));
            }
            *lease = Some(tokio::time::Instant::now() + self.lease_length());
        }
        let limit = self.timeouts.for_step(step);
        match tokio::time::timeout(limit, self.run_local(step)).await {
            Ok(r) => r,
            Err(_) => Err(format!(
                "{}: sin respuesta en {} s",
                describe(step),
                limit.as_secs()
            )),
        }
    }
}

/// Por qué falló un paso.
struct StepFailure {
    error: String,
    /// Si pudo haberse hecho igual: venció el tiempo o se cortó la conexión.
    uncertain: bool,
}

impl StepFailure {
    fn certain(error: String) -> Self {
        StepFailure {
            error,
            uncertain: false,
        }
    }
}

/// El [`Executor`] de una transacción: despacha y cuenta los pasos para la UI.
///
/// [`tandem_core::run`] llama primero a los pasos del plan en orden y, tras el primer
/// fallo, solo a los inversos: por eso alcanza con un contador.
struct Dispatcher<'a> {
    orch: &'a Orchestrator,
    cfg: &'a Config,
    txn: u64,
    next: u32,
    failed: bool,
    /// El estado con los pasos ya aplicados, para validar los que dependen de él.
    state: State,
    /// El inverso del paso incierto, si tampoco se pudo hacer.
    uncertain_undo_failures: Vec<(Step, String)>,
}

impl Dispatcher<'_> {
    /// Reglas que dependen de lo que el plan ya hizo. Hoy solo una: apagar la PC con un
    /// monitor todavía en ella dejaría a la Mac sin esa pantalla (DESIGN §5).
    fn check(&self, step: &Step) -> Result<(), String> {
        if *step != Step::PowerOffPc {
            return Ok(());
        }
        let on_pc: Vec<String> = self
            .state
            .owner
            .iter()
            .filter(|(_, m)| **m != Machine::Mac)
            .map(|(id, _)| id.to_string())
            .collect();
        if on_pc.is_empty() {
            return Ok(());
        }
        Err(format!(
            "{}: {} {} en la PC; primero hay que devolver los monitores a la Mac",
            describe(step),
            on_pc.join(", "),
            if on_pc.len() == 1 { "sigue" } else { "siguen" },
        ))
    }
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
        let r = match self.check(step) {
            Ok(()) => self.orch.attempt(step).await,
            Err(e) => Err(StepFailure::certain(e)),
        };
        match r {
            Ok(()) => {
                self.orch.emit(TxnEvent::StepDone { txn, index });
                self.next += 1;
                step.apply_to(&mut self.state);
                Ok(())
            }
            // El apagado es lo último del plan, con todo ya en la Mac. Si la respuesta
            // no llega, lo más probable es que la PC se haya apagado (o esté en eso):
            // deshacer devolvería los monitores a una PC sin imagen y la Mac se
            // quitaría los suyos. Se da por hecho; si la PC sigue prendida, queda sin
            // monitores y el próximo cambio la vuelve a esperar con Wake-on-LAN.
            Err(StepFailure {
                error,
                uncertain: true,
            }) if *step == Step::PowerOffPc => {
                tracing::warn!(
                    txn,
                    index,
                    "el apagado no respondió; se da por hecho: {error}"
                );
                self.orch.emit(TxnEvent::StepDone { txn, index });
                self.next += 1;
                Ok(())
            }
            Err(StepFailure { error, uncertain }) => {
                tracing::warn!(txn, index, "falló un paso: {error}");
                self.orch.emit(TxnEvent::StepFailed {
                    txn,
                    index,
                    error: error.clone(),
                });
                self.orch.emit(TxnEvent::RollingBack { txn });
                self.failed = true;
                // La geometría no hace falta: la vuelta atrás la recalcula al final.
                let undo = step.inverse(self.cfg).filter(|_| uncertain);
                if let Some(inv) = undo.filter(|inv| *inv != Step::UpdateInputGeometry)
                    && let Err(e) = self.orch.dispatch(&inv).await
                {
                    tracing::error!(txn, "no se pudo deshacer el paso incierto: {e}");
                    self.uncertain_undo_failures.push((inv, e));
                }
                Err(error)
            }
        }
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
        /// Se aplican, pero la respuesta se pierde con la conexión.
        pub lost_reply_on: Vec<Step>,
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
                } else if self.lost_reply_on.contains(step) {
                    Err(PEER_LOST.to_owned())
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
                // La geometría sigue fallando al recalcularla al final de la vuelta atrás.
                undo_failures: vec![
                    UndoFailure {
                        step: back,
                        error: "pc: falla simulada".into()
                    },
                    UndoFailure {
                        step: Step::UpdateInputGeometry,
                        error: "falla simulada".into()
                    }
                ]
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
        // El paso vencido es incierto: también se deshace.
        assert_eq!(
            local.steps(),
            [detach_mac.clone(), detach_mac.inverse(&cfg).unwrap()]
        );
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

    fn pc_with(monitors_on_pc: &[&str]) -> (tandem_core::Config, State) {
        let cfg = fixtures::desk().desk;
        let mut state = State::initial(&cfg);
        state.pc_on = true;
        for m in monitors_on_pc {
            state.owner.insert((*m).into(), Machine::Pc);
        }
        (cfg, state)
    }

    #[tokio::test]
    async fn power_off_is_refused_while_a_monitor_is_on_the_pc() {
        let (cfg, mut state) = pc_with(&["m34"]);
        let initial = state.clone();
        let (o, _, peer) = mac(FakeSteps::default(), FakeSteps::default());
        let mut rx = o.subscribe();
        let plan = Plan {
            steps: vec![Step::PowerOffPc],
        };
        let err = o.run(1, &cfg, &mut state, &plan).await.unwrap_err();
        let TxnError::RolledBack { step, error } = err else {
            panic!("se esperaba RolledBack");
        };
        assert_eq!(step, Step::PowerOffPc);
        assert_eq!(
            error,
            "apagar la PC: m34 sigue en la PC; primero hay que devolver los monitores a la Mac"
        );
        assert!(peer.steps().is_empty(), "la PC no recibió la orden");
        assert_eq!(state, initial);
        assert!(drain(&mut rx).contains(&TxnEvent::StepFailed {
            txn: 1,
            index: 0,
            error
        }));
    }

    #[tokio::test]
    async fn power_off_after_returning_only_some_monitors_is_refused() {
        let (cfg, mut state) = pc_with(&["m34", "m32"]);
        let (o, _, peer) = mac(FakeSteps::default(), FakeSteps::default());
        // El plan devuelve solo el 34" antes de apagar.
        let mut plan = Plan::to_mode(
            &cfg,
            &state,
            &Mode::Shared([("m32".into(), Machine::Pc)].into()),
        )
        .unwrap();
        plan.steps.push(Step::PowerOffPc);
        let err = o.run(1, &cfg, &mut state, &plan).await.unwrap_err();
        assert!(
            matches!(&err, TxnError::RolledBack { step: Step::PowerOffPc, error } if error.contains("m32 sigue")),
            "{err}"
        );
        assert!(!peer.steps().contains(&Step::PowerOffPc));
        // Lo que sí se hizo se deshizo: los dos monitores siguen en la PC.
        assert_eq!(state.owner[&"m34".into()], Machine::Pc);
        assert!(state.pc_on);
    }

    #[tokio::test]
    async fn shutdown_plan_returns_monitors_then_powers_off() {
        let (cfg, mut state) = pc_with(&["m34", "m32"]);
        let (o, _, peer) = mac(FakeSteps::default(), FakeSteps::default());
        let plan = Plan::shutdown_pc(&cfg, &state).unwrap();
        o.run(1, &cfg, &mut state, &plan).await.unwrap();
        assert_eq!(peer.steps().last(), Some(&Step::PowerOffPc));
        assert!(!state.pc_on);
        assert!(state.owner.values().all(|m| *m == Machine::Mac));
    }

    #[tokio::test(start_paused = true)]
    async fn power_on_resends_wake_until_the_pc_agent_connects() {
        let local = Arc::new(FakeSteps::default());
        let o = Arc::new(Orchestrator::new(
            Machine::Mac,
            local.clone(),
            StepTimeouts::default(),
        ));
        assert!(!*o.peer_connected().borrow());
        let o2 = o.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(12)).await;
            o2.set_peer(Arc::new(FakePeer(Arc::new(FakeSteps::default()))));
        });
        let start = tokio::time::Instant::now();
        o.dispatch(&Step::PowerOnPc).await.unwrap();
        assert_eq!(start.elapsed(), Duration::from_secs(12));
        // Envíos en 0, 5 y 10 s.
        assert_eq!(local.steps(), vec![Step::PowerOnPc; 3]);
    }

    #[tokio::test(start_paused = true)]
    async fn power_on_without_the_pc_agent_times_out() {
        let local = Arc::new(FakeSteps::default());
        let o = Orchestrator::new(Machine::Mac, local.clone(), StepTimeouts::default());
        let e = o.dispatch(&Step::PowerOnPc).await.unwrap_err();
        assert_eq!(e, "encender la PC: sin respuesta en 120 s");
        // Un envío cada 5 s desde 0; el de 120 s coincide con el tope y puede salir o no.
        let sent = local.steps().len();
        assert!((24..=25).contains(&sent), "{sent} envíos");
    }

    #[tokio::test(start_paused = true)]
    async fn power_on_stops_at_a_send_error() {
        let o = Orchestrator::new(
            Machine::Mac,
            Arc::new(FakeSteps {
                fail_on: vec![Step::PowerOnPc],
                ..Default::default()
            }),
            StepTimeouts::default(),
        );
        let start = tokio::time::Instant::now();
        assert_eq!(
            o.dispatch(&Step::PowerOnPc).await.unwrap_err(),
            "falla simulada"
        );
        assert_eq!(start.elapsed(), Duration::ZERO);
    }

    #[tokio::test]
    async fn losing_the_peer_makes_power_on_wait_again() {
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        assert!(*o.peer_connected().borrow());
        o.clear_peer();
        assert!(!*o.peer_connected().borrow());
        let e = o.dispatch(&switch_34_to_pc()).await.unwrap_err();
        assert!(e.contains(PEER_NOT_CONNECTED), "{e}");
    }

    fn machine() -> impl proptest::strategy::Strategy<Value = Machine> {
        proptest::prop_oneof![
            proptest::strategy::Just(Machine::Mac),
            proptest::strategy::Just(Machine::Pc)
        ]
    }

    proptest::proptest! {
        /// Para cualquier reparto de partida y de llegada, apagar la PC al final del
        /// plan llega a la PC si y solo si el reparto de llegada deja todo en la Mac.
        #[test]
        fn power_off_reaches_the_pc_only_with_every_monitor_on_the_mac(
            from in proptest::collection::vec(machine(), 2),
            to in proptest::collection::vec(machine(), 2),
        ) {
            let cfg = fixtures::desk().desk;
            let mut state = State::initial(&cfg);
            state.pc_on = true;
            let ids = ["m34", "m32"];
            for (id, m) in ids.iter().zip(&from) {
                state.owner.insert((*id).into(), *m);
            }
            let mode = Mode::Shared(ids.iter().map(|id| (*id).into()).zip(to.iter().copied()).collect());
            let mut plan = Plan::to_mode(&cfg, &state, &mode).unwrap();
            plan.steps.push(Step::PowerOffPc);
            let all_mac = to.iter().all(|m| *m == Machine::Mac);

            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let (o, _, peer) = mac(FakeSteps::default(), FakeSteps::default());
            let initial = state.clone();
            let outcome = rt.block_on(o.run(1, &cfg, &mut state, &plan));
            proptest::prop_assert_eq!(outcome.is_ok(), all_mac);
            proptest::prop_assert_eq!(peer.steps().contains(&Step::PowerOffPc), all_mac);
            if all_mac {
                proptest::prop_assert!(!state.pc_on);
            } else {
                proptest::prop_assert_eq!(state, initial);
            }
        }
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

    /// La PC agrega el 34" pero la respuesta se pierde: la Mac no sabe si se hizo, así
    /// que también lo deshace antes de deshacer el resto.
    #[tokio::test]
    async fn uncertain_step_is_undone_too() {
        let cfg = fixtures::desk().desk;
        let attach_pc = Step::Attach {
            machine: Machine::Pc,
            monitor: "m34".into(),
        };
        let (o, _, peer) = mac(
            FakeSteps::default(),
            FakeSteps {
                lost_reply_on: vec![attach_pc.clone()],
                ..Default::default()
            },
        );
        let mut state = State::initial(&cfg);
        state.pc_on = true;
        let initial = state.clone();
        let mode = Mode::Shared([("m34".into(), Machine::Pc)].into());
        let plan = Plan::to_mode(&cfg, &state, &mode).unwrap();
        let err = o.run(1, &cfg, &mut state, &plan).await.unwrap_err();
        assert!(matches!(err, TxnError::RolledBack { .. }), "{err:?}");
        assert_eq!(state, initial);
        assert_eq!(
            peer.steps(),
            [
                switch_34_to_pc(),
                attach_pc.clone(),
                attach_pc.inverse(&cfg).unwrap(),
                switch_34_to_pc().inverse(&cfg).unwrap(),
            ]
        );
    }

    /// Un paso que falla con certeza no se deshace: no llegó a hacerse.
    #[tokio::test]
    async fn certain_failure_is_not_undone() {
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
        let mut state = State::initial(&cfg);
        state.pc_on = true;
        let mode = Mode::Shared([("m34".into(), Machine::Pc)].into());
        let plan = Plan::to_mode(&cfg, &state, &mode).unwrap();
        o.run(1, &cfg, &mut state, &plan).await.unwrap_err();
        assert_eq!(
            peer.steps(),
            [
                switch_34_to_pc(),
                attach_pc,
                switch_34_to_pc().inverse(&cfg).unwrap(),
            ]
        );
    }

    /// Si tampoco se puede deshacer el paso incierto, el resultado es incierto.
    #[tokio::test]
    async fn failed_undo_of_uncertain_step_is_inconsistent() {
        let cfg = fixtures::desk().desk;
        let attach_pc = Step::Attach {
            machine: Machine::Pc,
            monitor: "m34".into(),
        };
        let detach_pc = attach_pc.inverse(&cfg).unwrap();
        let (o, _, _) = mac(
            FakeSteps::default(),
            FakeSteps {
                lost_reply_on: vec![attach_pc],
                fail_on: vec![detach_pc.clone()],
                ..Default::default()
            },
        );
        let mut state = State::initial(&cfg);
        state.pc_on = true;
        let mode = Mode::Shared([("m34".into(), Machine::Pc)].into());
        let plan = Plan::to_mode(&cfg, &state, &mode).unwrap();
        let err = o.run(1, &cfg, &mut state, &plan).await.unwrap_err();
        let TxnError::Inconsistent { undo_failures, .. } = err else {
            panic!("{err:?}");
        };
        assert_eq!(
            undo_failures,
            [(detach_pc, "pc: falla simulada".to_owned())]
        );
    }

    // QA ronda 2: cada agente solo se cuidaba de sus propias transacciones, así que
    // la Mac y la PC podían orquestar a la vez sobre el mismo puesto.
    #[tokio::test]
    async fn while_orchestrating_peer_steps_are_refused_without_running() {
        let (o, local, _) = mac(FakeSteps::default(), FakeSteps::default());
        let _txn = o.try_begin().unwrap();
        let e = o.execute_for_peer(&Step::PowerOnPc).await.unwrap_err();
        assert!(e.contains(BUSY_HERE), "{e}");
        assert!(local.steps().is_empty());
        o.end();
        o.execute_for_peer(&Step::PowerOnPc).await.unwrap();
        assert_eq!(local.steps(), [Step::PowerOnPc]);
    }

    #[tokio::test(start_paused = true)]
    async fn serving_the_peer_blocks_own_changes_until_it_finishes() {
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        o.execute_for_peer(&Step::UpdateInputGeometry)
            .await
            .unwrap();
        assert_eq!(o.try_begin(), None, "la máquina está prestada");
        // Llega el estado final de la transacción del otro: queda libre.
        o.release_peer_lease();
        let txn = o.try_begin().unwrap();
        o.end();

        // Si el estado final se pierde, el préstamo vence solo.
        o.execute_for_peer(&Step::UpdateInputGeometry)
            .await
            .unwrap();
        let t = StepTimeouts::default();
        tokio::time::advance(t.power_on + t.step - Duration::from_millis(1)).await;
        assert_eq!(o.try_begin(), None);
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(o.try_begin().is_some_and(|next| next > txn));
    }

    // QA ronda 2: si la respuesta del apagado se perdía, la Mac deshacía todo lo
    // anterior y se quitaba los monitores aunque la PC ya se estuviera apagando.
    #[tokio::test]
    async fn lost_reply_to_power_off_is_taken_as_done() {
        let cfg = fixtures::desk().desk;
        let (o, local, peer) = mac(
            FakeSteps::default(),
            FakeSteps {
                lost_reply_on: vec![Step::PowerOffPc],
                ..Default::default()
            },
        );
        let mut state = State::initial(&cfg);
        state.pc_on = true;
        state.owner.insert("m34".into(), Machine::Pc);
        let plan = Plan::shutdown_pc(&cfg, &state).unwrap();
        let report = o.run(1, &cfg, &mut state, &plan).await.unwrap();
        assert_eq!(report.applied, plan.steps);
        assert!(!state.pc_on);
        assert_eq!(state, {
            let mut s = State::initial(&cfg);
            s.builtin_pref = state.builtin_pref;
            s
        });
        // Nada se deshizo.
        assert_eq!(peer.steps().last(), Some(&Step::PowerOffPc));
        assert_eq!(local.steps().last(), Some(&Step::UpdateInputGeometry));

        // Un apagado que falla con certeza sí se deshace.
        let (o, _, peer) = mac(
            FakeSteps::default(),
            FakeSteps {
                fail_on: vec![Step::PowerOffPc],
                ..Default::default()
            },
        );
        let mut state = State::initial(&cfg);
        state.pc_on = true;
        state.owner.insert("m34".into(), Machine::Pc);
        let before = state.clone();
        o.run(2, &cfg, &mut state, &plan).await.unwrap_err();
        assert_eq!(state, before);
        assert_ne!(peer.steps().last(), Some(&Step::PowerOffPc));
    }
}
