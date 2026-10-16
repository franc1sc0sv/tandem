//! Fallos inyectados: en un paso concreto de una máquina, o en un instante.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_core::{Machine, Step};
use tandem_link::manager::LinkKind;

/// Qué le pasa a un paso.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaultAction {
    /// Falla sin aplicarse, con este motivo.
    Fail(String),
    /// No termina nunca: lo corta el tiempo máximo del orquestador.
    Hang,
    /// Tarda esto y después se aplica (si nadie lo cortó antes).
    Delay(Duration),
    /// Se cae el enlace al empezar el paso; el paso sigue normal.
    CutLink(LinkKind),
    /// El paso se aplica y el enlace se cae antes de la respuesta: quien lo pidió no
    /// sabe que se hizo.
    ApplyThenCut(LinkKind),
}

type Matcher = Arc<dyn Fn(&Step) -> bool + Send + Sync>;

/// Un fallo en los pasos de una máquina.
#[derive(Clone)]
pub struct Fault {
    machine: Machine,
    matcher: Matcher,
    action: FaultAction,
    /// Cuántas veces más se dispara.
    remaining: u32,
}

impl fmt::Debug for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fault")
            .field("machine", &self.machine)
            .field("action", &self.action)
            .field("remaining", &self.remaining)
            .finish_non_exhaustive()
    }
}

/// Fallos para una corrida. Cada fallo se dispara una vez salvo que se diga otra cosa,
/// así un paso que falla no hace fallar también a su inverso.
#[derive(Debug, Clone, Default)]
pub struct FaultPlan {
    pub(crate) faults: Vec<Fault>,
    /// Caídas de enlace programadas, contadas desde que se arma el plan.
    pub(crate) cuts: Vec<(Duration, LinkKind)>,
}

impl FaultPlan {
    pub fn new() -> Self {
        Self::default()
    }

    /// `action` la primera vez que `machine` ejecuta exactamente `step`.
    pub fn on(self, machine: Machine, step: Step, action: FaultAction) -> Self {
        self.on_matching(machine, move |s| *s == step, action)
    }

    /// `action` la primera vez que `machine` ejecuta un paso que cumple `matcher`.
    pub fn on_matching(
        mut self,
        machine: Machine,
        matcher: impl Fn(&Step) -> bool + Send + Sync + 'static,
        action: FaultAction,
    ) -> Self {
        self.faults.push(Fault {
            machine,
            matcher: Arc::new(matcher),
            action,
            remaining: 1,
        });
        self
    }

    /// El último fallo agregado se dispara `times` veces en vez de una.
    pub fn times(mut self, times: u32) -> Self {
        if let Some(f) = self.faults.last_mut() {
            f.remaining = times;
        }
        self
    }

    /// Se cae `link` a los `after` de armar el plan.
    pub fn cut_link_at(mut self, after: Duration, link: LinkKind) -> Self {
        self.cuts.push((after, link));
        self
    }
}

/// Los fallos pendientes, compartidos por los pasos simulados de las dos máquinas.
#[derive(Debug, Clone, Default)]
pub(crate) struct Faults(Arc<Mutex<Vec<Fault>>>);

impl Faults {
    pub(crate) fn arm(&self, faults: Vec<Fault>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = faults;
    }

    /// Las acciones que tocan a este paso, consumiendo una vez cada fallo.
    pub(crate) fn take(&self, machine: Machine, step: &Step) -> Vec<FaultAction> {
        let mut faults = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let mut actions = Vec::new();
        for f in faults.iter_mut() {
            if f.remaining > 0 && f.machine == machine && (f.matcher)(step) {
                f.remaining -= 1;
                actions.push(f.action.clone());
            }
        }
        faults.retain(|f| f.remaining > 0);
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_fault_fires_once_by_default() {
        let faults = Faults::default();
        let plan = FaultPlan::new()
            .on(
                Machine::Pc,
                Step::PowerOff {
                    machine: Machine::Pc,
                },
                FaultAction::Hang,
            )
            .on_matching(
                Machine::Mac,
                |s| matches!(s, Step::UpdateInputGeometry),
                FaultAction::Fail("x".into()),
            )
            .times(2);
        faults.arm(plan.faults);
        assert!(
            faults
                .take(
                    Machine::Mac,
                    &Step::PowerOff {
                        machine: Machine::Pc
                    }
                )
                .is_empty()
        );
        assert_eq!(
            faults.take(
                Machine::Pc,
                &Step::PowerOff {
                    machine: Machine::Pc
                }
            ),
            [FaultAction::Hang]
        );
        assert!(
            faults
                .take(
                    Machine::Pc,
                    &Step::PowerOff {
                        machine: Machine::Pc
                    }
                )
                .is_empty()
        );
        for _ in 0..2 {
            assert_eq!(
                faults.take(Machine::Mac, &Step::UpdateInputGeometry),
                [FaultAction::Fail("x".into())]
            );
        }
        assert!(
            faults
                .take(Machine::Mac, &Step::UpdateInputGeometry)
                .is_empty()
        );
    }
}
