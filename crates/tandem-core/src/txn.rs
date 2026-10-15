//! Ejecución transaccional de un [`Plan`].
//!
//! Los pasos se aplican en orden. Si uno falla, los ya aplicados se deshacen en
//! orden inverso y el estado vuelve al de partida. Los timeouts por paso son
//! responsabilidad del [`Executor`] (el agente envuelve cada paso con uno).

use std::future::Future;

use crate::model::Config;
use crate::plan::{Plan, Step};
use crate::state::State;

/// Quien sabe ejecutar pasos de verdad (DDC, topología, red, energía...).
pub trait Executor {
    fn execute(&mut self, step: &Step) -> impl Future<Output = Result<(), String>> + Send;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxnReport {
    pub applied: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TxnError {
    /// Falló un paso y todo se deshizo bien: el estado es el de partida.
    #[error("falló {step:?}: {error} (cambios deshechos)")]
    RolledBack { step: Step, error: String },
    /// Falló un paso y además falló deshacer algo: el estado real es incierto.
    #[error("falló {step:?}: {error}; además no se pudo deshacer: {undo_failures:?}")]
    Inconsistent {
        step: Step,
        error: String,
        undo_failures: Vec<(Step, String)>,
    },
}

/// Ejecuta `plan` sobre `state`. Solo modifica `state` con lo que de verdad quedó aplicado.
pub async fn run<E: Executor>(
    cfg: &Config,
    state: &mut State,
    plan: &Plan,
    exec: &mut E,
) -> Result<TxnReport, TxnError> {
    let snapshot = state.clone();
    let mut applied: Vec<Step> = Vec::new();

    for step in &plan.steps {
        match exec.execute(step).await {
            Ok(()) => {
                step.apply_to(state);
                applied.push(step.clone());
            }
            Err(error) => {
                let mut undo_failures = Vec::new();
                for done in applied.iter().rev() {
                    let Some(inv) = done.inverse(cfg) else {
                        continue;
                    };
                    match exec.execute(&inv).await {
                        Ok(()) => inv.apply_to(state),
                        Err(e) => undo_failures.push((inv, e)),
                    }
                }
                let step = step.clone();
                return if undo_failures.is_empty() {
                    // Los pasos sin inverso (encender la PC) dejan su efecto, y eso es lo correcto.
                    state.owner = snapshot.owner;
                    state.builtin_pref = snapshot.builtin_pref;
                    Err(TxnError::RolledBack { step, error })
                } else {
                    Err(TxnError::Inconsistent {
                        step,
                        error,
                        undo_failures,
                    })
                };
            }
        }
    }
    Ok(TxnReport { applied })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Machine, fixtures::desk};
    use crate::plan::Mode;

    /// Ejecutor de prueba: registra todo y falla en los pasos indicados.
    #[derive(Default)]
    struct Fake {
        log: Vec<Step>,
        fail_on: Vec<Step>,
    }

    impl Executor for Fake {
        fn execute(&mut self, step: &Step) -> impl Future<Output = Result<(), String>> + Send {
            self.log.push(step.clone());
            let r = if self.fail_on.contains(step) {
                Err("boom".to_owned())
            } else {
                Ok(())
            };
            async move { r }
        }
    }

    fn mode() -> Mode {
        Mode::Shared([("m34".into(), Machine::Pc)].into())
    }

    #[test]
    fn success_updates_state() {
        let cfg = desk();
        let mut s = State::initial(&cfg);
        let plan = Plan::to_mode(&cfg, &s, &mode()).unwrap();
        let mut exec = Fake::default();
        let report = pollster::block_on(run(&cfg, &mut s, &plan, &mut exec)).unwrap();
        assert_eq!(report.applied, plan.steps);
        assert!(s.pc_on);
        assert_eq!(s.owner[&"m34".into()], Machine::Pc);
    }

    #[test]
    fn failure_rolls_back_in_reverse() {
        let cfg = desk();
        let mut s = State::initial(&cfg);
        let plan = Plan::to_mode(&cfg, &s, &mode()).unwrap();
        // Falla quitar el 34" de la Mac: hay que deshacer el Attach y el cambio de entrada.
        let failing = plan
            .steps
            .iter()
            .find(|st| matches!(st, Step::Detach { .. }))
            .unwrap()
            .clone();
        let mut exec = Fake {
            fail_on: vec![failing.clone()],
            ..Default::default()
        };

        let err = pollster::block_on(run(&cfg, &mut s, &plan, &mut exec)).unwrap_err();
        assert_eq!(
            err,
            TxnError::RolledBack {
                step: failing,
                error: "boom".into()
            }
        );
        assert_eq!(
            s.owner[&"m34".into()],
            Machine::Mac,
            "el 34\" vuelve a la Mac"
        );
        assert!(s.pc_on, "la PC se queda prendida");

        let undo: Vec<_> = exec
            .log
            .iter()
            .skip(
                plan.steps
                    .iter()
                    .position(|st| matches!(st, Step::Detach { .. }))
                    .unwrap()
                    + 1,
            )
            .cloned()
            .collect();
        assert!(
            matches!(
                undo[0],
                Step::Detach {
                    machine: Machine::Pc,
                    ..
                }
            ),
            "primero se deshace el Attach"
        );
        assert!(
            matches!(
                undo[1],
                Step::SwitchInput {
                    to: Machine::Mac,
                    ..
                }
            ),
            "después la entrada"
        );
    }

    #[test]
    fn failed_undo_reports_inconsistent() {
        let cfg = desk();
        let mut s = State::initial(&cfg);
        let plan = Plan::to_mode(&cfg, &s, &mode()).unwrap();
        let back = Step::SwitchInput {
            monitor: "m34".into(),
            from: Machine::Pc,
            to: Machine::Mac,
            input: crate::model::Input::Hdmi,
        };
        let mut exec = Fake {
            fail_on: vec![Step::UpdateInputGeometry, back],
            ..Default::default()
        };
        let err = pollster::block_on(run(&cfg, &mut s, &plan, &mut exec)).unwrap_err();
        assert!(matches!(err, TxnError::Inconsistent { .. }));
    }
}
