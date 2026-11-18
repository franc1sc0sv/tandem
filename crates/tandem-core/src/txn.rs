//! Transactional execution of a [`Plan`].
//!
//! Steps are applied in order and it stops at the first one that fails. Then what was already
//! applied is undone, from last to first, with [`Executor::undo`]: each executor decides what it
//! knows how to undo. Power steps are not undone (leaving a machine on is harmless and
//! shutting it down midway through a failure would make things worse), so the state keeps what was
//! really done; a monitor input change does go back to the previous input. The
//! per-step timeouts are the [`Executor`]'s responsibility (the agent wraps each step
//! with one).

use std::future::Future;

use crate::plan::{Plan, Step};
use crate::state::State;

/// Whoever knows how to really run steps (network, power...).
pub trait Executor {
    fn execute(&mut self, step: &Step) -> impl Future<Output = Result<(), String>> + Send;

    /// Undoes an already applied step, because a later one failed. By default there is nothing to
    /// undo (power).
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
    /// A step failed: what came before was done and what came after was not attempted.
    #[error("{step:?} failed: {error}")]
    Failed { step: Step, error: String },
}

/// Runs `plan` on `state`. Only modifies `state` with what was really applied.
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
                // Rollback, from last to first. A failure while undoing does not cut the
                // others: it is added to the reason so the UI can say so.
                for done in applied.iter().rev() {
                    if let Err(e) = exec.undo(done).await {
                        error.push_str(&format!("; could not roll back: {e}"));
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

    /// Test executor: records everything and fails at the indicated steps.
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
                Err("stuck".to_owned())
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
        // Without a failure nothing is undone.
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
        assert_eq!(error, "boom; could not roll back: stuck");
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
        assert_eq!(exec.log, plan.steps, "nothing to undo");
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
        assert!(s.on.b, "what was done before the failure stays");
        let mut exec = Fake {
            fail_on: vec![on],
            ..Default::default()
        };
        let mut s = State::initial(Machine::A);
        pollster::block_on(run(&mut s, &plan, &mut exec)).unwrap_err();
        assert_eq!(exec.log.len(), 1);
    }
}
