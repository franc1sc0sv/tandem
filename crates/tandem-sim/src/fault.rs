//! Injected faults: at a specific step of a machine, or at an instant.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_core::{Machine, Step};
use tandem_link::manager::LinkKind;

/// What happens to a step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaultAction {
    /// Fails without being applied, with this reason.
    Fail(String),
    /// Never finishes: the orchestrator's maximum time cuts it off.
    Hang,
    /// Takes this long and then is applied (if nobody cut it off first).
    Delay(Duration),
    /// The link drops at the start of the step; the step goes on normally.
    CutLink(LinkKind),
    /// The step is applied and the link drops before the answer: whoever asked
    /// does not know it was done.
    ApplyThenCut(LinkKind),
}

type Matcher = Arc<dyn Fn(&Step) -> bool + Send + Sync>;

/// A fault in the steps of a machine.
#[derive(Clone)]
pub struct Fault {
    machine: Machine,
    matcher: Matcher,
    action: FaultAction,
    /// How many more times it fires.
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

/// Faults for a run. Each fault fires once unless said otherwise,
/// so that a step that fails does not also make its inverse fail.
#[derive(Debug, Clone, Default)]
pub struct FaultPlan {
    pub(crate) faults: Vec<Fault>,
    /// Scheduled link drops, counted from when the plan is built.
    pub(crate) cuts: Vec<(Duration, LinkKind)>,
}

impl FaultPlan {
    pub fn new() -> Self {
        Self::default()
    }

    /// `action` the first time `machine` executes exactly `step`.
    pub fn on(self, machine: Machine, step: Step, action: FaultAction) -> Self {
        self.on_matching(machine, move |s| *s == step, action)
    }

    /// `action` the first time `machine` executes a step that satisfies `matcher`.
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

    /// The last fault added fires `times` times instead of once.
    pub fn times(mut self, times: u32) -> Self {
        if let Some(f) = self.faults.last_mut() {
            f.remaining = times;
        }
        self
    }

    /// `link` drops `after` the plan is built.
    pub fn cut_link_at(mut self, after: Duration, link: LinkKind) -> Self {
        self.cuts.push((after, link));
        self
    }
}

/// The pending faults, shared by the simulated steps of both machines.
#[derive(Debug, Clone, Default)]
pub(crate) struct Faults(Arc<Mutex<Vec<Fault>>>);

impl Faults {
    pub(crate) fn arm(&self, faults: Vec<Fault>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = faults;
    }

    /// The actions that touch this step, consuming each fault once.
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
                Machine::B,
                Step::PowerOff {
                    machine: Machine::B,
                },
                FaultAction::Hang,
            )
            .on_matching(
                Machine::A,
                |s| matches!(s, Step::PowerOn { .. }),
                FaultAction::Fail("x".into()),
            )
            .times(2);
        faults.arm(plan.faults);
        assert!(
            faults
                .take(
                    Machine::A,
                    &Step::PowerOff {
                        machine: Machine::B
                    }
                )
                .is_empty()
        );
        assert_eq!(
            faults.take(
                Machine::B,
                &Step::PowerOff {
                    machine: Machine::B
                }
            ),
            [FaultAction::Hang]
        );
        assert!(
            faults
                .take(
                    Machine::B,
                    &Step::PowerOff {
                        machine: Machine::B
                    }
                )
                .is_empty()
        );
        for _ in 0..2 {
            assert_eq!(
                faults.take(
                    Machine::A,
                    &Step::PowerOn {
                        machine: Machine::B
                    }
                ),
                [FaultAction::Fail("x".into())]
            );
        }
        assert!(
            faults
                .take(
                    Machine::A,
                    &Step::PowerOn {
                        machine: Machine::B
                    }
                )
                .is_empty()
        );
    }
}
