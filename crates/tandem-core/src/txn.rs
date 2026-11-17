//! Ejecución transaccional de un [`Plan`].
//!
//! Los pasos se aplican en orden y se corta en el primero que falla. Entonces se deshace
//! lo ya aplicado, del último al primero, con [`Executor::undo`]: cada ejecutor decide qué
//! sabe deshacer. Los de energía no se deshacen (dejar prendida una máquina es inofensivo y
//! apagarla a mitad de un fallo empeoraría las cosas), así que el estado queda con lo que de
//! verdad se hizo; un cambio de entrada de monitor sí vuelve a la entrada de antes. Los
//! timeouts por paso son responsabilidad del [`Executor`] (el agente envuelve cada paso
//! con uno).

use std::future::Future;

use crate::plan::{Plan, Step};
use crate::state::State;

/// Quien sabe ejecutar pasos de verdad (red, energía...).
pub trait Executor {
    fn execute(&mut self, step: &Step) -> impl Future<Output = Result<(), String>> + Send;

    /// Deshace un paso ya aplicado, porque uno posterior falló. Por omisión no hay nada que
    /// deshacer (energía).
    fn undo(&mut self, step: &Step) -> impl Future<Output = Result<(), String>> + Send {
        let _ = step;
        async { Ok(()) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxnReport {
    pub applied: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TxnError {
    /// Falló un paso: lo anterior quedó hecho y lo siguiente no se intentó.
    #[error("falló {step:?}: {error}")]
    Failed { step: Step, error: String },
}

/// Ejecuta `plan` sobre `state`. Solo modifica `state` con lo que de verdad quedó aplicado.
pub async fn run<E: Executor>(
    state: &mut State,
    plan: &Plan,
    exec: &mut E,
) -> Result<TxnReport, TxnError> {
    let mut applied: Vec<Step> = Vec::new();
    for step in &plan.steps {
        match exec.execute(step).await {
            Ok(()) => {
                step.apply_to(state);
                applied.push(step.clone());
            }
            Err(mut error) => {
                // Vuelta atrás, del último al primero. Un fallo al deshacer no corta los
                // demás: se suma al motivo para que la UI lo diga.
                for done in applied.iter().rev() {
                    if let Err(e) = exec.undo(done).await {
                        error.push_str(&format!("; no se pudo volver atrás: {e}"));
                    }
                }
                return Err(TxnError::Failed {
                    step: step.clone(),
                    error,
                });
            }
        }
    }
    Ok(TxnReport { applied })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Machine;
    use crate::plan::{Abilities, Plan};

    /// Ejecutor de prueba: registra todo y falla en los pasos indicados.
    #[derive(Default)]
    struct Fake {
        log: Vec<Step>,
        fail_on: Vec<Step>,
        undone: Vec<Step>,
        undo_fails: bool,
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

        fn undo(&mut self, step: &Step) -> impl Future<Output = Result<(), String>> + Send {
            self.undone.push(step.clone());
            let r = if self.undo_fails {
                Err("atascado".to_owned())
            } else {
                Ok(())
            };
            async move { r }
        }
    }

    fn switch(input: crate::MonitorInput) -> Step {
        Step::SwitchMonitor {
            monitor: "m".into(),
            input,
        }
    }

    #[test]
    fn a_failure_undoes_what_was_applied_last_first() {
        use crate::MonitorInput::{DisplayPort, Hdmi};
        let on = Step::PowerOn {
            machine: Machine::B,
        };
        let plan = Plan {
            steps: vec![on.clone(), switch(Hdmi), switch(DisplayPort)],
        };
        let mut exec = Fake {
            fail_on: vec![switch(DisplayPort)],
            ..Default::default()
        };
        let mut s = State::initial(Machine::A);
        pollster::block_on(run(&mut s, &plan, &mut exec)).unwrap_err();
        assert_eq!(exec.undone, [switch(Hdmi), on]);
        // Sin fallo no se deshace nada.
        let mut exec = Fake::default();
        pollster::block_on(run(&mut s, &plan, &mut exec)).unwrap();
        assert!(exec.undone.is_empty());
    }

    #[test]
    fn a_failed_undo_is_told_in_the_error() {
        let plan = Plan {
            steps: vec![
                switch(crate::MonitorInput::Hdmi),
                Step::PowerOff {
                    machine: Machine::B,
                },
            ],
        };
        let mut exec = Fake {
            fail_on: vec![plan.steps[1].clone()],
            undo_fails: true,
            ..Default::default()
        };
        let mut s = State::initial(Machine::A);
        let TxnError::Failed { error, .. } =
            pollster::block_on(run(&mut s, &plan, &mut exec)).unwrap_err();
        assert_eq!(error, "boom; no se pudo volver atrás: atascado");
    }

    #[test]
    fn success_updates_state() {
        let mut s = State::initial(Machine::A);
        let plan = Plan::power_on(&s, Machine::B, &Abilities::both_wake()).unwrap();
        let mut exec = Fake::default();
        let report = pollster::block_on(run(&mut s, &plan, &mut exec)).unwrap();
        assert_eq!(report.applied, plan.steps);
        assert!(s.on.b);
    }

    #[test]
    fn a_failed_step_leaves_the_state_as_it_was() {
        let mut s = State::initial(Machine::A);
        s.on.b = true;
        let plan = Plan::power_off(&s, Machine::B);
        let mut exec = Fake {
            fail_on: plan.steps.clone(),
            ..Default::default()
        };
        let before = s.clone();
        let err = pollster::block_on(run(&mut s, &plan, &mut exec)).unwrap_err();
        assert_eq!(
            err,
            TxnError::Failed {
                step: Step::PowerOff {
                    machine: Machine::B
                },
                error: "boom".into()
            }
        );
        assert_eq!(s, before);
        assert_eq!(exec.log, plan.steps, "nada que deshacer");
    }

    #[test]
    fn steps_after_a_failure_are_not_tried() {
        let mut s = State::initial(Machine::A);
        let on = Step::PowerOn {
            machine: Machine::B,
        };
        let off = Step::PowerOff {
            machine: Machine::B,
        };
        let plan = Plan {
            steps: vec![on.clone(), off.clone()],
        };
        let mut exec = Fake {
            fail_on: vec![off],
            ..Default::default()
        };
        pollster::block_on(run(&mut s, &plan, &mut exec)).unwrap_err();
        assert!(s.on.b, "lo hecho antes del fallo se queda");
        let mut exec = Fake {
            fail_on: vec![on],
            ..Default::default()
        };
        let mut s = State::initial(Machine::A);
        pollster::block_on(run(&mut s, &plan, &mut exec)).unwrap_err();
        assert_eq!(exec.log.len(), 1);
    }
}
