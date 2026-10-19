//! La ida y vuelta del asistente (T-089): pasar un monitor a la otra entrada para que el
//! usuario diga qué ve, y volver.
//!
//! Es solo un `SwitchInput`: no pasa por el cambio de modo, así que no toca ningún
//! escritorio (ni `Attach` ni `Detach`) ni el resto de los monitores, aunque el estado esté
//! sin confirmar. Mientras la PC conserve su salida, el canal DDC sigue vivo con el monitor
//! en la otra entrada (DESIGN §8), y de ahí sale la vuelta.
//!
//! La vuelta no depende de la UI: al mandar la ida, el agente arma un plazo
//! ([`PROBE_HOLD`]) y, si nadie la pidió antes, devuelve el monitor él solo, con
//! reintentos. Pedir la vuelta (`to == back`) la adelanta. Un cambio de modo que mande la
//! entrada de ese monitor cancela el plazo: lo que decida ese plan manda.

use std::collections::BTreeMap;
use std::sync::{Arc, MutexGuard, PoisonError};
use std::time::Duration;

use tandem_core::{Config, Machine, MonitorConfig, MonitorId, Plan, Step};
use tandem_proto::Reply;

use super::{Agent, BUSY, Busy};

/// Cuánto puede quedarse un monitor en la entrada de prueba antes de que el agente lo
/// devuelva. Más que la espera de la respuesta del usuario en el asistente (20 s).
pub const PROBE_HOLD: Duration = Duration::from_secs(30);
/// Intentos de la vuelta automática y espera entre ellos.
const RETURN_TRIES: u32 = 4;
const RETURN_GAP: Duration = Duration::from_secs(3);

/// Los plazos de vuelta armados, por monitor, con un número de generación: armar de nuevo
/// o cancelar invalida el plazo anterior.
#[derive(Debug, Default)]
pub(super) struct Returns {
    next: u64,
    armed: BTreeMap<MonitorId, u64>,
}

fn switch_plan(m: &MonitorConfig, from: Machine, to: Machine, by: Machine) -> Option<Plan> {
    Some(Plan {
        steps: vec![Step::SwitchInput {
            monitor: m.id.clone(),
            from,
            to,
            input: m.input_for(to)?,
            by,
        }],
    })
}

impl Agent {
    fn returns(&self) -> MutexGuard<'_, Returns> {
        self.returns.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn arm_return(&self, monitor: &MonitorId) -> u64 {
        let mut r = self.returns();
        r.next += 1;
        let generation = r.next;
        r.armed.insert(monitor.clone(), generation);
        generation
    }

    /// Un cambio de modo manda la entrada de `monitor`: la vuelta programada ya no aplica.
    pub(super) fn disarm_return(&self, monitor: &MonitorId) {
        self.returns().armed.remove(monitor);
    }

    fn is_armed(&self, monitor: &MonitorId, generation: u64) -> bool {
        self.returns().armed.get(monitor) == Some(&generation)
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
        let Some(by) = self.abilities().switcher(monitor, &self.state()) else {
            return Reply::Rejected(format!(
                "ninguna máquina puede cambiar la entrada de {}",
                m.label
            ));
        };
        let returning = to == back;
        let (go, ret) = if returning {
            (
                switch_plan(&m, to.other(), back, by),
                switch_plan(&m, to.other(), back, by),
            )
        } else {
            (switch_plan(&m, back, to, by), switch_plan(&m, to, back, by))
        };
        let (Some(go), Some(ret)) = (go, ret) else {
            return Reply::Rejected(format!("{} no está cableado a esa máquina", m.label));
        };
        let Some(txn) = self.orch.try_begin() else {
            return Reply::Rejected(BUSY.into());
        };
        let busy = Busy(self.clone());
        let agent = self.clone();
        if returning {
            // La vuelta pedida adelanta al plazo; si falla, el plazo se arma otra vez.
            self.disarm_return(monitor);
            tokio::spawn(async move {
                if !agent.run_unchanged(txn, &desk, &go, busy).await {
                    let generation = agent.arm_return(&m.id);
                    agent.spawn_return(m.id.clone(), generation, RETURN_GAP, desk, ret);
                }
            });
            return Reply::Accepted { txn };
        }
        let generation = self.arm_return(monitor);
        tracing::info!(txn, %monitor, ?to, ?back, "ida de prueba; vuelve sola en {PROBE_HOLD:?}");
        tokio::spawn(async move {
            let ok = agent.run_unchanged(txn, &desk, &go, busy).await;
            // Si la ida falló pudo haber llegado igual (incierta): se devuelve ya.
            let delay = if ok { PROBE_HOLD } else { Duration::ZERO };
            agent.spawn_return(m.id.clone(), generation, delay, desk, ret);
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

    /// La vuelta automática: después de `delay`, y mientras el plazo siga armado, manda
    /// `back` con reintentos.
    fn spawn_return(
        self: &Arc<Self>,
        monitor: MonitorId,
        generation: u64,
        delay: Duration,
        desk: Config,
        back: Plan,
    ) {
        let agent = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            for attempt in 1..=RETURN_TRIES {
                if !agent.is_armed(&monitor, generation) {
                    return;
                }
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
                    if !agent.is_armed(&monitor, generation) {
                        return;
                    }
                    if agent.run_unchanged(txn, &desk, &back, busy).await {
                        let mut r = agent.returns();
                        if r.armed.get(&monitor) == Some(&generation) {
                            r.armed.remove(&monitor);
                        }
                        return;
                    }
                }
                tracing::warn!(%monitor, attempt, "la vuelta de la prueba no salió; se reintenta");
                tokio::time::sleep(RETURN_GAP).await;
            }
            tracing::error!(%monitor, "la vuelta automática de la prueba no pudo: revisa el monitor");
        });
    }
}
