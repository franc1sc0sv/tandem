//! La ida y vuelta del asistente (T-089): pasar un monitor a la otra entrada para que el
//! usuario diga qué ve, y volver.
//!
//! Es solo un `SwitchInput` ([`Plan::probe`]): no pasa por el cambio de modo, así que no
//! toca ningún escritorio (ni `Attach` ni `Detach`) ni el resto de los monitores, aunque el
//! estado esté sin confirmar. Mientras la PC conserve su salida, el canal DDC sigue vivo con
//! el monitor en la otra entrada (DESIGN §8), y de ahí sale la vuelta. El plan se niega si la
//! máquina destino pudo haber quitado el monitor de su escritorio (el fantasma).
//!
//! La vuelta no depende de la UI: al mandar la ida, el agente arma un plazo
//! ([`PROBE_HOLD`]) y, si nadie la pidió antes, devuelve el monitor él solo. Si no puede
//! (el enlace con la PC cayó) reintenta con espera creciente, otra vez cada vez que el enlace
//! vuelve, y avisa a la UI si pasa de [`GIVE_UP_AFTER`]. Pedir la vuelta (`to == back`) la
//! adelanta. Un cambio de modo que mande la entrada de ese monitor y salga bien cancela el
//! plazo; si falla, el plazo sigue armado.

use std::collections::BTreeMap;
use std::sync::{Arc, MutexGuard, PoisonError};
use std::time::Duration;

use tandem_core::{Config, Machine, MonitorId, Plan, Step};
use tandem_proto::{PowerEvent, Reply};
use tokio::time::Instant;

use super::{Agent, BUSY, Busy};

/// Cuánto puede quedarse un monitor en la entrada de prueba antes de que el agente lo
/// devuelva. Más que la espera de la respuesta del usuario en el asistente (20 s).
pub const PROBE_HOLD: Duration = Duration::from_secs(30);
/// Primera espera entre reintentos de la vuelta; se duplica hasta [`RETURN_MAX_GAP`].
const RETURN_GAP: Duration = Duration::from_secs(3);
const RETURN_MAX_GAP: Duration = Duration::from_secs(30);
/// Pasado este tiempo sin poder devolverlo se avisa a la UI (y se sigue intentando cada
/// vez que el enlace vuelve).
pub const GIVE_UP_AFTER: Duration = Duration::from_secs(120);

/// Una vuelta armada: su número de generación (armar de nuevo o cancelar invalida la
/// anterior) y con qué plan se hace.
#[derive(Debug, Clone)]
struct Job {
    generation: u64,
    desk: Config,
    back: Plan,
    /// Desde cuándo toca devolverlo (antes, el usuario sigue mirando la prueba).
    due: Instant,
}

#[derive(Debug, Default)]
pub(super) struct Returns {
    next: u64,
    armed: BTreeMap<MonitorId, Job>,
    /// Ya hay una tarea que reintenta al reconectar.
    watching: bool,
}

impl Agent {
    fn returns(&self) -> MutexGuard<'_, Returns> {
        self.returns.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn arm_return(&self, monitor: &MonitorId, desk: &Config, back: &Plan, wait: Duration) -> u64 {
        let mut r = self.returns();
        r.next += 1;
        let generation = r.next;
        r.armed.insert(
            monitor.clone(),
            Job {
                generation,
                desk: desk.clone(),
                back: back.clone(),
                due: Instant::now() + wait,
            },
        );
        generation
    }

    /// Un cambio de modo salió bien y mandó la entrada de `monitor`: la vuelta programada ya
    /// no aplica.
    pub(super) fn disarm_return(&self, monitor: &MonitorId) {
        self.returns().armed.remove(monitor);
    }

    fn is_armed(&self, monitor: &MonitorId, generation: u64) -> bool {
        self.returns()
            .armed
            .get(monitor)
            .is_some_and(|j| j.generation == generation)
    }

    /// Los monitores con una vuelta pendiente.
    pub fn pending_returns(&self) -> Vec<MonitorId> {
        self.returns().armed.keys().cloned().collect()
    }

    /// Pasa `monitor` a la entrada de `to` para probarlo, o lo devuelve a `back` si
    /// `to == back`. `back` es donde el usuario dice que está ahora.
    pub(super) fn probe_input(
        self: &Arc<Self>,
        monitor: &MonitorId,
        to: Machine,
        back: Machine,
    ) -> Reply {
        let desk = self.plan_desk();
        let Some(m) = desk.monitor(monitor).cloned() else {
            return Reply::Rejected(format!(
                "el monitor `{monitor}` no está en la configuración o no está conectado"
            ));
        };
        // Sin perfil DDC no hay vuelta automática: la prueba la haría el usuario con los
        // botones del monitor, y eso es el modo manual asistido, no esta.
        if m.ddc.is_none() {
            return Reply::Rejected(format!(
                "{} no tiene un perfil DDC verificado: calíbralo antes de probarlo",
                m.label
            ));
        }
        let (state, abilities) = (self.state(), self.abilities());
        let plan = |to: Machine| {
            Plan::probe(&desk, &state, monitor, to, back, &abilities)
                .map_err(|e| Reply::Rejected(format!("{}: {e}", m.label)))
        };
        let (go, ret) = match (plan(to), plan(back)) {
            (Ok(go), Ok(ret)) => (go, ret),
            (Err(r), _) | (_, Err(r)) => return r,
        };
        let returning = to == back;
        let Some(txn) = self.orch.try_begin() else {
            // La vuelta pedida justo cuando corre la automática: ya va en camino.
            if returning && self.returns().armed.contains_key(monitor) {
                return Reply::Done;
            }
            return Reply::Rejected(BUSY.into());
        };
        let busy = Busy(self.clone());
        let agent = self.clone();
        self.watch_reconnect();
        if returning {
            // La vuelta pedida adelanta al plazo; si falla, el plazo se arma otra vez.
            self.disarm_return(monitor);
            tokio::spawn(async move {
                if !agent.run_unchanged(txn, &desk, &go, busy).await {
                    let generation = agent.arm_return(&m.id, &desk, &ret, RETURN_GAP);
                    agent.spawn_return(m.id.clone(), generation, RETURN_GAP);
                }
            });
            return Reply::Accepted { txn };
        }
        let generation = self.arm_return(monitor, &desk, &ret, PROBE_HOLD);
        tracing::info!(txn, %monitor, ?to, ?back, "ida de prueba; vuelve sola en {PROBE_HOLD:?}");
        tokio::spawn(async move {
            let ok = agent.run_unchanged(txn, &desk, &go, busy).await;
            // Si la ida falló pudo haber llegado igual (incierta): se devuelve ya.
            let delay = if ok { PROBE_HOLD } else { Duration::ZERO };
            agent.spawn_return(m.id.clone(), generation, delay);
        });
        Reply::Accepted { txn }
    }

    /// Corre `plan` sin cambiar el estado guardado: la prueba solo mira el monitor en la
    /// otra entrada. Devuelve si salió bien.
    async fn run_unchanged(
        self: &Arc<Self>,
        txn: u64,
        desk: &Config,
        plan: &Plan,
        busy: Busy,
    ) -> bool {
        let original = self.state();
        let mut scratch = original.clone();
        let outcome = self.orch.run(txn, desk, &mut scratch, plan).await;
        if let Err(e) = &outcome {
            tracing::warn!(txn, "prueba de entrada fallida: {e}");
        }
        drop(busy);
        self.orch.finish(txn, &original, &outcome);
        outcome.is_ok()
    }

    /// La vuelta automática: después de `delay`, y mientras el plazo siga armado, manda el
    /// plan de vuelta con esperas crecientes hasta que salga. Pasado [`GIVE_UP_AFTER`] avisa a
    /// la UI y sigue, más espaciada.
    fn spawn_return(self: &Arc<Self>, monitor: MonitorId, generation: u64, delay: Duration) {
        let agent = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let started = Instant::now();
            let (mut gap, mut warned) = (RETURN_GAP, false);
            loop {
                let job = {
                    let r = agent.returns();
                    match r.armed.get(&monitor) {
                        Some(j) if j.generation == generation => j.clone(),
                        _ => return,
                    }
                };
                // Espera su turno: un cambio de modo en curso termina antes.
                let mut turn = None;
                for _ in 0..80 {
                    if let Some(txn) = agent.orch.try_begin() {
                        turn = Some(txn);
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                if let Some(txn) = turn {
                    let busy = Busy(agent.clone());
                    // Un cambio de modo pudo cancelarla mientras esperaba.
                    if !agent.is_armed(&monitor, generation) {
                        return;
                    }
                    if agent.run_unchanged(txn, &job.desk, &job.back, busy).await {
                        let mut r = agent.returns();
                        if r.armed
                            .get(&monitor)
                            .is_some_and(|j| j.generation == generation)
                        {
                            r.armed.remove(&monitor);
                        }
                        return;
                    }
                }
                tracing::warn!(%monitor, "la vuelta de la prueba no salió; se reintenta");
                if !warned && started.elapsed() >= GIVE_UP_AFTER {
                    warned = true;
                    agent.warn_stranded(&monitor, &job.back);
                }
                tokio::time::sleep(gap).await;
                gap = (gap * 2).min(RETURN_MAX_GAP);
            }
        });
    }

    /// Avisa a la UI de que `monitor` no pudo volver de la prueba y sigue en la otra entrada.
    fn warn_stranded(&self, monitor: &MonitorId, back: &Plan) {
        let by = back
            .steps
            .iter()
            .find_map(|s| match s {
                Step::SwitchInput { by, .. } => Some(*by),
                _ => None,
            })
            .unwrap_or(Machine::Pc);
        tracing::error!(%monitor, "la vuelta automática de la prueba no pudo: revisa el monitor");
        self.emit_power(PowerEvent::PeerLost {
            machine: by,
            reason: format!(
                "no se pudo devolver {monitor} a su entrada tras la prueba; sigue intentando"
            ),
            stranded: vec![monitor.clone()],
        });
    }

    /// Cada vez que vuelve el enlace con la otra máquina, las vueltas pendientes se
    /// reintentan de inmediato (una sola tarea por agente).
    fn watch_reconnect(self: &Arc<Self>) {
        {
            let mut r = self.returns();
            if r.watching {
                return;
            }
            r.watching = true;
        }
        let weak = Arc::downgrade(self);
        let mut link = self.orch.peer_connected();
        tokio::spawn(async move {
            while link.changed().await.is_ok() {
                if !*link.borrow_and_update() {
                    continue;
                }
                let Some(agent) = weak.upgrade() else { return };
                let pending: Vec<(MonitorId, Job)> = agent
                    .returns()
                    .armed
                    .iter()
                    .filter(|(_, j)| j.due <= Instant::now())
                    .map(|(m, j)| (m.clone(), j.clone()))
                    .collect();
                for (monitor, job) in pending {
                    // Una generación nueva retira la tarea que estaba esperando.
                    let generation =
                        agent.arm_return(&monitor, &job.desk, &job.back, Duration::ZERO);
                    agent.spawn_return(monitor, generation, Duration::ZERO);
                }
            }
        });
    }
}
