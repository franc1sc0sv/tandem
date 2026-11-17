//! Orquestador: ejecuta un [`Plan`] como transacción repartiendo cada paso a la
//! máquina que lo tiene que hacer (DESIGN §3.3 y §5).
//!
//! Cada paso va a esta máquina ([`LocalSteps`]) o al otro agente por el plano de
//! control ([`PeerLink`]), siempre con un tiempo máximo ([`StepTimeouts`]). La ejecución
//! la hace [`tandem_core::run`]; aquí solo se despacha y se publica el avance como
//! [`TxnEvent`] para la UI.
//!
//! Los pasos de energía tienen reglas propias: [`Step::PowerOn`] es mandar el Wake-on-LAN
//! desde la otra máquina (repetido cada [`WOL_RESEND`]) y esperar a que el agente de la
//! encendida se conecte, y un [`Step::PowerOff`] cuya respuesta se pierde se da por hecho
//! (lo más probable es que la máquina se esté apagando).
//!
//! [`Step::SwitchMonitor`] lo hace el dueño del monitor (DECISIONS D13), que sale de
//! [`MonitorControl`]: si es esta máquina, aquí; si no, se le pide a la otra por el plano de
//! control. Si falla un paso posterior, o el cambio se queda sin veredicto, se vuelve a la
//! entrada de antes (vuelta atrás).
//!
//! No hay una máquina central (DESIGN §3.7): cualquiera de las dos orquesta, pero de a
//! una: la que hace pasos para la otra queda prestada hasta recibir su estado final, y la
//! que orquesta rechaza los pasos ajenos.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use tandem_core::{Executor, Machine, MonitorInput, Plan, State, Step, TxnError, TxnReport};
use tandem_proto::{ControlMsg, TxnEvent};

use tokio::sync::{broadcast, watch};

/// Lo que responde un paso que esta máquina todavía no sabe hacer.
pub const STEP_NOT_IMPLEMENTED: &str = "no implementado";
/// El paso le toca al otro agente y no hay conexión con él.
pub const PEER_NOT_CONNECTED: &str = "el otro agente no está conectado";
/// Se cortó la conexión con el otro agente con un paso pendiente.
pub const PEER_LOST: &str = "se perdió la conexión con el otro agente";
/// El otro agente pidió un paso mientras esta máquina orquesta su propio cambio.
pub const BUSY_HERE: &str = "esta máquina está en medio de otro cambio";
/// Un monitor que ninguna máquina puede controlar ahora.
pub const NO_OWNER: &str = "ninguna máquina tiene canal con el monitor";

/// Los monitores del puesto, vistos desde el orquestador ([`crate::monitors::MonitorHub`]).
pub trait MonitorControl: Send + Sync {
    /// La máquina dueña de `monitor`, que es la que lo cambia.
    fn owner(&self, monitor: &str) -> Option<Machine>;
    /// La entrada en pantalla según el dueño: a ella se vuelve si hay que deshacer.
    fn input(&self, monitor: &str) -> Option<MonitorInput>;
    /// Cambia la entrada aquí (esta máquina es la dueña) y espera el veredicto. Un error que
    /// empieza con [`tandem_monitor::NO_VERDICT`] deja la duda de si cambió.
    fn switch<'a>(&'a self, monitor: &'a str, input: MonitorInput) -> StepFuture<'a>;
}

/// Cada cuánto se repite el Wake-on-LAN mientras se espera a la otra máquina: un paquete
/// UDP se puede perder, y la tarjeta tarda en quedar lista tras cortarse la luz.
pub const WOL_RESEND: Duration = Duration::from_secs(5);

/// Resultado de un paso: el error es un texto para mostrar en la UI.
pub type StepFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

/// Los pasos que esta máquina hace por sí misma (energía).
///
/// [`Step::PowerOn`] aquí es solo mandar un Wake-on-LAN a la otra máquina: esperarla y
/// repetir el envío lo hace el [`Orchestrator`].
pub trait LocalSteps: Send + Sync {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a>;
}

/// Ejecutor de una máquina sin backends: todo paso responde [`STEP_NOT_IMPLEMENTED`].
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
    /// Le manda un aviso sin respuesta (capacidades, puesto). Sin conexión se pierde:
    /// se repite al aparecer el enlace siguiente.
    fn send(&self, msg: ControlMsg) {
        let _ = msg;
    }
    /// Retiene el enlace por el que van los pasos mientras dure una transacción
    /// propia: no se cambia de enlace a medias salvo que el activo caiga. Se libera
    /// al soltar lo devuelto. Sin gestor de enlaces no hay nada que retener.
    fn hold_link(&self) -> Option<LinkHold> {
        None
    }
    /// El enlace activo con el otro agente y lo que tardó su último ping (T-087: el asistente
    /// lo muestra). `None` sin enlace.
    fn rtt(&self) -> Option<(tandem_proto::Link, Duration)> {
        None
    }
}

/// Lo que devuelve [`PeerLink::hold_link`]; la retención dura lo que viva.
pub type LinkHold = Box<dyn Send + Sync>;

/// Sin conexión con el otro agente.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoPeer;

impl PeerLink for NoPeer {
    fn execute<'a>(&'a self, _step: &'a Step) -> StepFuture<'a> {
        Box::pin(async { Err(PEER_NOT_CONNECTED.to_owned()) })
    }

    fn publish_state(&self, _state: &State) {}
}

/// La máquina que sabe hacer cada paso de energía; un cambio de monitor depende de su
/// dueño ([`Orchestrator::target_of`]) y da `None`.
pub fn target(step: &Step) -> Option<Machine> {
    match step {
        // Una máquina apagada no puede encenderse sola: el Wake-on-LAN sale de la otra.
        Step::PowerOn { machine } => Some(machine.other()),
        Step::PowerOff { machine } => Some(*machine),
        Step::SwitchMonitor { .. } => None,
    }
}

/// Texto corto de un paso, para errores y logs.
pub fn describe(step: &Step) -> String {
    match step {
        Step::PowerOn { machine } => format!("encender la máquina {machine}"),
        Step::PowerOff { machine } => format!("apagar la máquina {machine}"),
        Step::SwitchMonitor { monitor, input } => {
            format!("poner {input} en el monitor {monitor}")
        }
    }
}

/// El error de un cambio de monitor deja la duda de si cambió (sin veredicto del firmware).
fn no_verdict(error: &str) -> bool {
    error.contains(tandem_monitor::NO_VERDICT)
}

/// Tiempo máximo por paso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepTimeouts {
    /// Encender la otra máquina incluye esperar a que su agente responda.
    pub power_on: Duration,
    /// Todos los demás pasos.
    pub step: Duration,
    /// Cambiar la entrada de un monitor: el firmware tarda unos segundos en dar su veredicto
    /// (la espera del veredicto, 12 s, queda dentro).
    pub monitor: Duration,
}

/// Lo que puede tardar un cambio de entrada con su veredicto.
pub const MONITOR_STEP: Duration = Duration::from_secs(20);

impl Default for StepTimeouts {
    fn default() -> Self {
        StepTimeouts {
            power_on: Duration::from_secs(120),
            step: Duration::from_secs(10),
            monitor: MONITOR_STEP,
        }
    }
}

impl StepTimeouts {
    pub fn for_step(&self, step: &Step) -> Duration {
        match step {
            Step::PowerOn { .. } => self.power_on,
            Step::PowerOff { .. } => self.step,
            Step::SwitchMonitor { .. } => self.monitor,
        }
    }
}

/// Avisos de transacción que se guardan para un suscriptor lento antes de que pierda alguno.
const EVENT_BUFFER: usize = 256;

/// Despacha pasos y publica el avance de las transacciones de un agente.
pub struct Orchestrator {
    /// El lado de esta máquina; cambia solo si al emparejar resulta ser el otro.
    me: Mutex<Machine>,
    local: Arc<dyn LocalSteps>,
    peer: RwLock<Arc<dyn PeerLink>>,
    /// Si hay conexión con el otro agente: encender la otra termina cuando pasa a `true`.
    connected: watch::Sender<bool>,
    timeouts: StepTimeouts,
    events: broadcast::Sender<TxnEvent>,
    next_txn: AtomicU64,
    busy: AtomicBool,
    /// Hasta cuándo esta máquina está prestada a una transacción del otro agente.
    /// Mientras una orquesta, la otra no puede empezar un cambio propio, y una que
    /// orquesta no acepta pasos ajenos. Las dos comprobaciones van bajo este candado para
    /// que no se crucen.
    peer_lease: Mutex<Option<tokio::time::Instant>>,
    /// Los pasos del otro agente corren de a uno aunque lleguen por streams (enlaces)
    /// distintos.
    peer_turn: tokio::sync::Mutex<()>,
    /// El enlace retenido durante la transacción propia en curso.
    link_hold: Mutex<Option<LinkHold>>,
    /// Los monitores; sin ellos, todo cambio de monitor falla con [`NO_OWNER`].
    monitors: RwLock<Option<Arc<dyn MonitorControl>>>,
}

impl std::fmt::Debug for Orchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Orchestrator")
            .field("me", &self.me())
            .field("timeouts", &self.timeouts)
            .field("busy", &self.busy)
            .finish_non_exhaustive()
    }
}

impl Orchestrator {
    pub fn new(me: Machine, local: Arc<dyn LocalSteps>, timeouts: StepTimeouts) -> Self {
        Orchestrator {
            me: Mutex::new(me),
            local,
            peer: RwLock::new(Arc::new(NoPeer)),
            connected: watch::Sender::new(false),
            timeouts,
            events: broadcast::channel(EVENT_BUFFER).0,
            next_txn: AtomicU64::new(1),
            busy: AtomicBool::new(false),
            peer_lease: Mutex::new(None),
            peer_turn: tokio::sync::Mutex::new(()),
            link_hold: Mutex::new(None),
            monitors: RwLock::new(None),
        }
    }

    /// Quién sabe de los monitores del puesto.
    pub fn set_monitors(&self, monitors: Arc<dyn MonitorControl>) {
        *self
            .monitors
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(monitors);
    }

    fn monitors(&self) -> Option<Arc<dyn MonitorControl>> {
        self.monitors
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// La máquina que hace `step`: la de [`target`], o el dueño del monitor.
    pub fn target_of(&self, step: &Step) -> Result<Machine, String> {
        match step {
            Step::SwitchMonitor { monitor, .. } => self
                .monitors()
                .and_then(|m| m.owner(monitor))
                .ok_or_else(|| format!("{}: {NO_OWNER}", describe(step))),
            _ => target(step).ok_or_else(|| describe(step)),
        }
    }

    fn me(&self) -> Machine {
        *self.me.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Esta máquina resultó ser el otro lado ([`crate::Agent::adopt_side`]).
    pub fn set_machine(&self, side: Machine) {
        *self.me.lock().unwrap_or_else(PoisonError::into_inner) = side;
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
        let txn = self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| self.next_txn.fetch_add(1, Ordering::Relaxed))?;
        *self.hold() = self.peer().hold_link();
        Some(txn)
    }

    fn hold(&self) -> std::sync::MutexGuard<'_, Option<LinkHold>> {
        self.link_hold
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn lease(&self) -> std::sync::MutexGuard<'_, Option<tokio::time::Instant>> {
        self.peer_lease
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Ninguna transacción en curso, ni propia ni del otro agente. Es solo una foto: para
    /// reservar el orquestador está [`Orchestrator::try_begin`].
    pub fn is_idle(&self) -> bool {
        let leased = self
            .lease()
            .is_some_and(|until| tokio::time::Instant::now() < until);
        !leased && !self.busy.load(Ordering::Acquire)
    }

    /// La transacción del otro agente terminó (llegó su estado final).
    pub fn release_peer_lease(&self) {
        *self.lease() = None;
    }

    /// Cuánto dura el préstamo tras cada paso del otro agente: lo más que puede
    /// tardar él en pedir el siguiente (un paso propio con su tiempo máximo, el
    /// más largo es encender la otra) más un paso nuestro. Si se pierde su estado
    /// final, la máquina queda libre a lo sumo tras esto.
    fn lease_length(&self) -> Duration {
        self.timeouts.power_on + self.timeouts.step
    }

    /// Libera lo reservado con [`Orchestrator::try_begin`].
    pub fn end(&self) {
        // Primero el enlace: el aviso final al otro agente ya puede ir por el mejor.
        drop(self.hold().take());
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
            inverses: Vec::new(),
        };
        tandem_core::run(state, plan, &mut d).await
    }

    /// Publica el desenlace a la UI y el estado final al otro agente.
    pub fn finish(&self, txn: u64, state: &State, outcome: &Result<TxnReport, TxnError>) {
        let event = match outcome {
            Ok(_) => TxnEvent::Committed {
                txn,
                state: state.clone(),
            },
            Err(TxnError::Failed { .. }) => TxnEvent::Failed {
                txn,
                state: state.clone(),
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
            let target = self.target_of(step).map_err(StepFailure::certain)?;
            if target == self.me() {
                self.run_local(step).await.map_err(|error| StepFailure {
                    uncertain: no_verdict(&error),
                    error,
                })
            } else {
                self.remote(step).await
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
            Step::PowerOn { machine } if *machine != self.me() => self.power_on_peer(step).await,
            Step::SwitchMonitor { monitor, input } => match self.monitors() {
                Some(m) => m.switch(monitor, *input).await,
                None => Err(format!("{}: {NO_OWNER}", describe(step))),
            },
            _ => self.local.execute(step).await,
        }
    }

    /// Wake-on-LAN hasta que el agente de la otra máquina se conecta. El tope lo pone
    /// quien llama (`power_on_timeout_secs`); un error al mandar el paquete corta enseguida.
    async fn power_on_peer(&self, step: &Step) -> Result<(), String> {
        let mut connected = self.connected.subscribe();
        loop {
            self.local.execute(step).await?;
            tokio::select! {
                // El emisor vive en `self`: la espera no puede quedarse sin emisor.
                r = connected.wait_for(|c| *c) => {
                    return r.map(|_| ()).map_err(|_| PEER_LOST.to_owned());
                }
                () = tokio::time::sleep(WOL_RESEND) => {
                    tracing::debug!(
                        "la máquina {} todavía no responde: se repite el Wake-on-LAN",
                        self.me().other()
                    );
                }
            }
        }
    }

    async fn remote(&self, step: &Step) -> Result<(), StepFailure> {
        let peer = self.peer();
        peer.execute(step).await.map_err(|e| StepFailure {
            uncertain: e == PEER_LOST || no_verdict(&e),
            error: format!("máquina {}: {e}", self.me().other()),
        })
    }

    /// Un paso pedido por el otro agente: solo se aceptan los que le tocan a esta máquina.
    pub async fn execute_for_peer(&self, step: &Step) -> Result<(), String> {
        let me = self.me();
        if self.target_of(step)? != me {
            return Err(format!(
                "{}: no le corresponde a la máquina {me}",
                describe(step)
            ));
        }
        // La espera está acotada: quien tiene el turno corre con su tiempo máximo.
        let _turn = self.peer_turn.lock().await;
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
struct Dispatcher<'a> {
    orch: &'a Orchestrator,
    txn: u64,
    next: u32,
    /// Cada cambio de monitor hecho y el que lo deshace (volver a la entrada de antes).
    inverses: Vec<(Step, Step)>,
}

impl Dispatcher<'_> {
    /// El paso que deja el monitor como estaba antes de `step`, si se sabe cómo estaba.
    fn inverse(&self, step: &Step) -> Option<Step> {
        let Step::SwitchMonitor { monitor, input } = step else {
            return None;
        };
        let before = self.orch.monitors()?.input(monitor)?;
        (before != *input).then(|| Step::SwitchMonitor {
            monitor: monitor.clone(),
            input: before,
        })
    }

    async fn roll_back(&self, inverse: &Step) -> Result<(), String> {
        tracing::warn!(txn = self.txn, "vuelta atrás: {}", describe(inverse));
        self.orch.dispatch(inverse).await
    }
}

impl Executor for Dispatcher<'_> {
    async fn execute(&mut self, step: &Step) -> Result<(), String> {
        let (txn, index) = (self.txn, self.next);
        self.orch.emit(TxnEvent::StepStarted { txn, index });
        let inverse = self.inverse(step);
        match self.orch.attempt(step).await {
            Ok(()) => {
                if let Some(inverse) = inverse {
                    self.inverses.push((step.clone(), inverse));
                }
                self.orch.emit(TxnEvent::StepDone { txn, index });
                self.next += 1;
                Ok(())
            }
            // Si la respuesta del apagado no llega, lo más probable es que se haya apagado
            // (o esté en eso). Se da por hecho; si sigue prendida, la vigilancia del par la
            // vuelve a ver y la marca prendida.
            Err(StepFailure {
                error,
                uncertain: true,
            }) if matches!(step, Step::PowerOff { .. }) => {
                tracing::warn!(
                    txn,
                    index,
                    "el apagado no respondió; se da por hecho: {error}"
                );
                self.orch.emit(TxnEvent::StepDone { txn, index });
                self.next += 1;
                Ok(())
            }
            Err(StepFailure {
                mut error,
                uncertain,
            }) => {
                tracing::warn!(txn, index, "falló un paso: {error}");
                // Un cambio de entrada que pudo haberse hecho (sin veredicto, sin respuesta) se
                // deshace aquí: para la transacción no quedó aplicado.
                if uncertain
                    && let Some(inverse) = &inverse
                    && let Err(e) = self.roll_back(inverse).await
                {
                    error.push_str(&format!("; no se pudo volver atrás: {e}"));
                }
                self.orch.emit(TxnEvent::StepFailed {
                    txn,
                    index,
                    error: error.clone(),
                });
                Err(error)
            }
        }
    }

    async fn undo(&mut self, step: &Step) -> Result<(), String> {
        let Some(at) = self.inverses.iter().rposition(|(done, _)| done == step) else {
            return Ok(());
        };
        let (_, inverse) = self.inverses.remove(at);
        self.roll_back(&inverse).await
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use tandem_core::Abilities;

    use super::*;

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

    const PC_ON: Step = Step::PowerOn {
        machine: Machine::B,
    };
    const PC_OFF: Step = Step::PowerOff {
        machine: Machine::B,
    };

    fn mac(local: FakeSteps, peer: FakeSteps) -> (Orchestrator, Arc<FakeSteps>, Arc<FakeSteps>) {
        let local = Arc::new(local);
        let peer = Arc::new(peer);
        let o = Orchestrator::new(Machine::A, local.clone(), StepTimeouts::default());
        o.set_peer(Arc::new(FakePeer(peer.clone())));
        (o, local, peer)
    }

    fn drain(rx: &mut broadcast::Receiver<TxnEvent>) -> Vec<TxnEvent> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    /// Visto desde la Mac, con la PC prendida.
    fn both_on() -> State {
        let mut s = State::initial(Machine::A);
        s.on.b = true;
        s
    }

    #[test]
    fn routes_each_step_to_its_machine() {
        assert_eq!(target(&PC_ON), Some(Machine::A));
        assert_eq!(target(&PC_OFF), Some(Machine::B));
        assert_eq!(describe(&PC_ON), "encender la máquina B");
        assert_eq!(
            describe(&Step::PowerOff {
                machine: Machine::A
            }),
            "apagar la máquina A"
        );
    }

    #[test]
    fn power_on_gets_the_long_timeout() {
        let t = StepTimeouts::default();
        assert_eq!(t.for_step(&PC_ON), Duration::from_secs(120));
        assert_eq!(t.for_step(&PC_OFF), Duration::from_secs(10));
    }

    #[tokio::test]
    async fn dispatches_locally_or_remotely() {
        let (o, local, peer) = mac(FakeSteps::default(), FakeSteps::default());
        o.dispatch(&PC_OFF).await.unwrap();
        o.dispatch(&PC_ON).await.unwrap();
        assert_eq!(local.steps(), [PC_ON]);
        assert_eq!(peer.steps(), [PC_OFF]);
    }

    #[tokio::test]
    async fn remote_errors_name_the_machine() {
        let (o, _, _) = mac(
            FakeSteps::default(),
            FakeSteps {
                fail_on: vec![PC_OFF],
                ..Default::default()
            },
        );
        let e = o.dispatch(&PC_OFF).await.unwrap_err();
        assert_eq!(e, "máquina B: falla simulada");
    }

    #[tokio::test]
    async fn without_peer_remote_steps_fail_clearly() {
        let o = Orchestrator::new(Machine::A, Arc::new(Unimplemented), StepTimeouts::default());
        let e = o.dispatch(&PC_OFF).await.unwrap_err();
        assert!(e.contains(PEER_NOT_CONNECTED), "{e}");
        let e = o.dispatch(&PC_ON).await.unwrap_err();
        assert_eq!(e, format!("encender la máquina B: {STEP_NOT_IMPLEMENTED}"));
    }

    #[tokio::test]
    async fn refuses_peer_requests_for_the_peers_own_steps() {
        let (o, local, _) = mac(FakeSteps::default(), FakeSteps::default());
        assert!(o.execute_for_peer(&PC_OFF).await.is_err());
        o.execute_for_peer(&PC_ON).await.unwrap();
        assert_eq!(local.steps(), [PC_ON]);
    }

    #[test]
    fn only_one_transaction_at_a_time() {
        let o = Orchestrator::new(Machine::A, Arc::new(Unimplemented), StepTimeouts::default());
        let a = o.try_begin().unwrap();
        assert_eq!(o.try_begin(), None);
        o.end();
        let b = o.try_begin().unwrap();
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn publishes_progress_of_a_committed_transaction() {
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        let mut rx = o.subscribe();
        let mut state = both_on();
        let plan = Plan::power_off(&state, Machine::B);
        let outcome = o.run(7, &mut state, &plan).await;
        o.finish(7, &state, &outcome);
        assert!(outcome.is_ok());
        assert!(!state.on.b);
        assert_eq!(
            drain(&mut rx),
            [
                TxnEvent::Started {
                    txn: 7,
                    steps: plan.steps.clone(),
                },
                TxnEvent::StepStarted { txn: 7, index: 0 },
                TxnEvent::StepDone { txn: 7, index: 0 },
                TxnEvent::Committed { txn: 7, state },
            ]
        );
    }

    #[tokio::test]
    async fn a_failed_step_publishes_the_failure_and_keeps_the_state() {
        let (o, _, _) = mac(
            FakeSteps::default(),
            FakeSteps {
                fail_on: vec![PC_OFF],
                ..Default::default()
            },
        );
        let mut rx = o.subscribe();
        let mut state = both_on();
        let initial = state.clone();
        let plan = Plan::power_off(&state, Machine::B);
        let outcome = o.run(1, &mut state, &plan).await;
        o.finish(1, &state, &outcome);
        assert!(matches!(outcome, Err(TxnError::Failed { .. })));
        assert_eq!(state, initial);
        let events = drain(&mut rx);
        assert_eq!(
            events[events.len() - 2..],
            [
                TxnEvent::StepFailed {
                    txn: 1,
                    index: 0,
                    error: "máquina B: falla simulada".into()
                },
                TxnEvent::Failed {
                    txn: 1,
                    state: initial
                },
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn power_on_resends_wake_until_the_pc_agent_connects() {
        let local = Arc::new(FakeSteps::default());
        let o = Arc::new(Orchestrator::new(
            Machine::A,
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
        o.dispatch(&PC_ON).await.unwrap();
        assert_eq!(start.elapsed(), Duration::from_secs(12));
        // Envíos en 0, 5 y 10 s.
        assert_eq!(local.steps(), vec![PC_ON; 3]);
    }

    #[tokio::test(start_paused = true)]
    async fn power_on_without_the_pc_agent_times_out() {
        let local = Arc::new(FakeSteps::default());
        let o = Orchestrator::new(Machine::A, local.clone(), StepTimeouts::default());
        let e = o.dispatch(&PC_ON).await.unwrap_err();
        assert_eq!(e, "encender la máquina B: sin respuesta en 120 s");
        // Un envío cada 5 s desde 0; el de 120 s coincide con el tope y puede salir o no.
        let sent = local.steps().len();
        assert!((24..=25).contains(&sent), "{sent} envíos");
    }

    #[tokio::test(start_paused = true)]
    async fn power_on_stops_at_a_send_error() {
        let o = Orchestrator::new(
            Machine::A,
            Arc::new(FakeSteps {
                fail_on: vec![PC_ON],
                ..Default::default()
            }),
            StepTimeouts::default(),
        );
        let start = tokio::time::Instant::now();
        assert_eq!(o.dispatch(&PC_ON).await.unwrap_err(), "falla simulada");
        assert_eq!(start.elapsed(), Duration::ZERO);
    }

    #[tokio::test]
    async fn losing_the_peer_makes_power_on_wait_again() {
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        assert!(*o.peer_connected().borrow());
        o.clear_peer();
        assert!(!*o.peer_connected().borrow());
        let e = o.dispatch(&PC_OFF).await.unwrap_err();
        assert!(e.contains(PEER_NOT_CONNECTED), "{e}");
    }

    #[tokio::test(start_paused = true)]
    async fn power_on_may_take_longer_than_other_steps() {
        // 60 s para encender la máquina B está dentro de los 120 s.
        let (o, _, _) = mac(
            FakeSteps {
                delays: vec![(PC_ON, Duration::from_secs(60))],
                ..Default::default()
            },
            FakeSteps::default(),
        );
        let mut state = State::initial(Machine::A);
        let plan = Plan::power_on(&state, Machine::B, &Abilities::both_wake()).unwrap();
        o.run(4, &mut state, &plan).await.unwrap();
        assert!(state.on.b);

        // 121 s, no.
        let (o, _, _) = mac(
            FakeSteps {
                delays: vec![(PC_ON, Duration::from_secs(121))],
                ..Default::default()
            },
            FakeSteps::default(),
        );
        let mut state = State::initial(Machine::A);
        let err = o.run(5, &mut state, &plan).await.unwrap_err();
        assert!(matches!(err, TxnError::Failed { step: PC_ON, .. }));
        assert_eq!(state, State::initial(Machine::A));
    }

    // QA ronda 2: cada agente solo se cuidaba de sus propias transacciones, así que
    // la Mac y la PC podían orquestar a la vez sobre el mismo puesto.
    #[tokio::test]
    async fn while_orchestrating_peer_steps_are_refused_without_running() {
        let (o, local, _) = mac(FakeSteps::default(), FakeSteps::default());
        let _txn = o.try_begin().unwrap();
        let e = o.execute_for_peer(&PC_ON).await.unwrap_err();
        assert!(e.contains(BUSY_HERE), "{e}");
        assert!(local.steps().is_empty());
        o.end();
        o.execute_for_peer(&PC_ON).await.unwrap();
        assert_eq!(local.steps(), [PC_ON]);
    }

    // T-064: dos pedidos del otro agente que llegan por streams distintos no se pisan.
    #[tokio::test(start_paused = true)]
    async fn peer_steps_from_different_streams_run_one_at_a_time() {
        let mac_off = Step::PowerOff {
            machine: Machine::A,
        };
        let (o, local, _) = mac(
            FakeSteps {
                delays: vec![(PC_ON, Duration::from_secs(3))],
                ..Default::default()
            },
            FakeSteps::default(),
        );
        let start = tokio::time::Instant::now();
        let (first, second) = tokio::join!(
            async {
                o.execute_for_peer(&PC_ON).await.unwrap();
                start.elapsed()
            },
            async {
                tokio::task::yield_now().await;
                o.execute_for_peer(&mac_off).await.unwrap();
                start.elapsed()
            },
        );
        assert_eq!(first, Duration::from_secs(3));
        assert!(second >= first, "el segundo terminó antes: {second:?}");
        assert_eq!(local.steps(), [PC_ON, mac_off]);
    }

    /// Un otro agente que cuenta cuántas retenciones de enlace hay vivas.
    struct HoldingPeer(Arc<std::sync::atomic::AtomicUsize>);

    struct CountedHold(Arc<std::sync::atomic::AtomicUsize>);

    impl Drop for CountedHold {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl PeerLink for HoldingPeer {
        fn execute<'a>(&'a self, _step: &'a Step) -> StepFuture<'a> {
            Box::pin(async { Ok(()) })
        }
        fn publish_state(&self, _state: &State) {}
        fn hold_link(&self) -> Option<LinkHold> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Some(Box::new(CountedHold(self.0.clone())))
        }
    }

    // T-065: durante una transacción propia el enlace no cambia salvo que caiga.
    #[test]
    fn a_transaction_holds_the_link_until_it_ends() {
        let holds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let o = Orchestrator::new(
            Machine::A,
            Arc::new(FakeSteps::default()),
            StepTimeouts::default(),
        );
        o.set_peer(Arc::new(HoldingPeer(holds.clone())));
        let count = || holds.load(Ordering::SeqCst);

        assert_eq!(count(), 0);
        o.try_begin().unwrap();
        assert_eq!(count(), 1);
        // Un segundo intento rechazado no retiene nada más.
        assert_eq!(o.try_begin(), None);
        assert_eq!(count(), 1);
        o.end();
        assert_eq!(count(), 0);
        o.try_begin().unwrap();
        o.end();
        assert_eq!(count(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn serving_the_peer_blocks_own_changes_until_it_finishes() {
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        o.execute_for_peer(&PC_ON).await.unwrap();
        assert_eq!(o.try_begin(), None, "la máquina está prestada");
        // Llega el estado final de la transacción del otro: queda libre.
        o.release_peer_lease();
        let txn = o.try_begin().unwrap();
        o.end();

        // Si el estado final se pierde, el préstamo vence solo.
        o.execute_for_peer(&PC_ON).await.unwrap();
        let t = StepTimeouts::default();
        tokio::time::advance(t.power_on + t.step - Duration::from_millis(1)).await;
        assert_eq!(o.try_begin(), None);
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(o.try_begin().is_some_and(|next| next > txn));
    }

    /// Monitores de prueba: un dueño fijo, una entrada y lo que contesta cada cambio.
    struct FakeMonitors {
        owner: Option<Machine>,
        input: MonitorInput,
        answer: Mutex<Vec<Result<(), String>>>,
        switched: Mutex<Vec<MonitorInput>>,
    }

    impl FakeMonitors {
        fn new(owner: Option<Machine>, answers: Vec<Result<(), String>>) -> Arc<Self> {
            Arc::new(FakeMonitors {
                owner,
                input: MonitorInput::DisplayPort,
                answer: Mutex::new(answers),
                switched: Mutex::new(Vec::new()),
            })
        }

        fn switched(&self) -> Vec<MonitorInput> {
            self.switched.lock().unwrap().clone()
        }
    }

    impl MonitorControl for FakeMonitors {
        fn owner(&self, _monitor: &str) -> Option<Machine> {
            self.owner
        }
        fn input(&self, _monitor: &str) -> Option<MonitorInput> {
            Some(self.input)
        }
        fn switch<'a>(&'a self, _monitor: &'a str, input: MonitorInput) -> StepFuture<'a> {
            self.switched.lock().unwrap().push(input);
            let mut answers = self.answer.lock().unwrap();
            let r = if answers.is_empty() {
                Ok(())
            } else {
                answers.remove(0)
            };
            Box::pin(async move { r })
        }
    }

    fn to_hdmi() -> Step {
        Step::SwitchMonitor {
            monitor: "m".into(),
            input: MonitorInput::Hdmi,
        }
    }

    #[tokio::test]
    async fn a_monitor_switch_goes_to_its_owner() {
        // Dueña esta máquina: lo hace aquí, sin pasar por los pasos de energía.
        let (o, local, peer) = mac(FakeSteps::default(), FakeSteps::default());
        let m = FakeMonitors::new(Some(Machine::A), vec![]);
        o.set_monitors(m.clone());
        o.dispatch(&to_hdmi()).await.unwrap();
        assert_eq!(m.switched(), [MonitorInput::Hdmi]);
        assert!(local.steps().is_empty() && peer.steps().is_empty());
        assert_eq!(o.timeouts.for_step(&to_hdmi()), MONITOR_STEP);

        // Dueña la otra: se le pide a ella.
        let (o, _, peer) = mac(FakeSteps::default(), FakeSteps::default());
        let m = FakeMonitors::new(Some(Machine::B), vec![]);
        o.set_monitors(m.clone());
        o.dispatch(&to_hdmi()).await.unwrap();
        assert_eq!(peer.steps(), [to_hdmi()]);
        assert!(m.switched().is_empty());
        // Y la otra no lo hace por esta si el dueño es esta.
        let e = o.execute_for_peer(&to_hdmi()).await.unwrap_err();
        assert!(e.contains("no le corresponde"), "{e}");

        // Sin dueño (o sin monitores) se dice por qué.
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        let e = o.dispatch(&to_hdmi()).await.unwrap_err();
        assert!(e.contains(NO_OWNER), "{e}");
        o.set_monitors(FakeMonitors::new(None, vec![]));
        let e = o.dispatch(&to_hdmi()).await.unwrap_err();
        assert!(e.contains(NO_OWNER), "{e}");
    }

    #[tokio::test]
    async fn a_switch_without_verdict_goes_back_to_the_input_before() {
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        let m = FakeMonitors::new(
            Some(Machine::A),
            vec![Err(format!("{} en 12 s", tandem_monitor::NO_VERDICT))],
        );
        o.set_monitors(m.clone());
        let mut state = both_on();
        let plan = Plan::switch_monitor("m", MonitorInput::Hdmi);
        let err = o.run(1, &mut state, &plan).await.unwrap_err();
        assert!(matches!(err, TxnError::Failed { .. }));
        assert_eq!(
            m.switched(),
            [MonitorInput::Hdmi, MonitorInput::DisplayPort]
        );
    }

    #[tokio::test]
    async fn a_failed_verdict_is_not_undone() {
        let (o, _, _) = mac(FakeSteps::default(), FakeSteps::default());
        let m = FakeMonitors::new(Some(Machine::A), vec![Err("sigue en DP".into())]);
        o.set_monitors(m.clone());
        let mut state = both_on();
        let plan = Plan::switch_monitor("m", MonitorInput::Hdmi);
        o.run(1, &mut state, &plan).await.unwrap_err();
        assert_eq!(m.switched(), [MonitorInput::Hdmi]);
    }

    #[tokio::test]
    async fn a_later_failure_undoes_the_switch() {
        let (o, _, _) = mac(
            FakeSteps::default(),
            FakeSteps {
                fail_on: vec![PC_OFF],
                ..Default::default()
            },
        );
        let m = FakeMonitors::new(Some(Machine::A), vec![]);
        o.set_monitors(m.clone());
        let mut state = both_on();
        let plan = Plan {
            steps: vec![to_hdmi(), PC_OFF],
        };
        o.run(1, &mut state, &plan).await.unwrap_err();
        assert_eq!(
            m.switched(),
            [MonitorInput::Hdmi, MonitorInput::DisplayPort]
        );
        assert!(state.on.b, "la energía no se deshace ni cambia");
    }

    // QA ronda 2: si la respuesta del apagado se perdía, la Mac lo daba por fallido aunque
    // la PC ya se estuviera apagando.
    #[tokio::test]
    async fn lost_reply_to_power_off_is_taken_as_done() {
        let (o, _, peer) = mac(
            FakeSteps::default(),
            FakeSteps {
                lost_reply_on: vec![PC_OFF],
                ..Default::default()
            },
        );
        let mut state = both_on();
        let plan = Plan::power_off(&state, Machine::B);
        let report = o.run(1, &mut state, &plan).await.unwrap();
        assert_eq!(report.applied, plan.steps);
        assert!(!state.on.b);
        assert_eq!(peer.steps(), [PC_OFF]);

        // Un apagado que falla con certeza queda como estaba.
        let (o, _, _) = mac(
            FakeSteps::default(),
            FakeSteps {
                fail_on: vec![PC_OFF],
                ..Default::default()
            },
        );
        let mut state = both_on();
        let before = state.clone();
        o.run(2, &mut state, &plan).await.unwrap_err();
        assert_eq!(state, before);
    }

    #[tokio::test(start_paused = true)]
    async fn a_power_off_that_hangs_is_taken_as_done() {
        let (o, _, _) = mac(
            FakeSteps::default(),
            FakeSteps {
                delays: vec![(PC_OFF, Duration::from_secs(3600))],
                ..Default::default()
            },
        );
        let mut state = both_on();
        let plan = Plan::power_off(&state, Machine::B);
        let start = tokio::time::Instant::now();
        o.run(3, &mut state, &plan).await.unwrap();
        assert_eq!(start.elapsed(), Duration::from_secs(10));
        assert!(!state.on.b);
    }
}
