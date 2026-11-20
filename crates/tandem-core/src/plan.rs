use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::model::{Arrangement, Machine};
use crate::monitor::MonitorInput;
use crate::state::State;

/// What each machine can do, according to what it announced (DESIGN §3.7): which one can be
/// powered on. Plans come from here, not from the machine's name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Abilities {
    /// The machines the other one can power on (Wake-on-LAN).
    pub wakeable: BTreeSet<Machine>,
}

impl Abilities {
    /// Both can be woken. Serves as a base for the tests.
    pub fn both_wake() -> Self {
        Abilities {
            wakeable: BTreeSet::from([Machine::A, Machine::B]),
        }
    }

    pub fn can_wake(&self, machine: Machine) -> bool {
        self.wakeable.contains(&machine)
    }
}

/// An atomic step of a transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "step")]
pub enum Step {
    /// Power on `machine` from the other one (Wake-on-LAN) and wait for its agent to answer.
    PowerOn { machine: Machine },
    /// Clean shutdown of `machine`.
    PowerOff { machine: Machine },
    /// Switch the input of monitor `monitor` (its id, `<model>-<hash>`) and wait for the
    /// firmware's verdict. Done by the machine that owns the monitor (DECISIONS D13), whichever
    /// one orchestrates.
    SwitchMonitor {
        monitor: String,
        input: MonitorInput,
    },
    /// Scene step: make monitor `monitor` show `machine` (DESIGN §5). It is the
    /// abstract step of T-100; T-111 translates it to [`Step::SwitchMonitor`] with that
    /// machine's input. Done by the machine that applies the scene.
    ShowMachine { monitor: String, machine: Machine },
    /// Scene step: the desktop arrangement (`None`: the default). It is a change
    /// to the shared desk.
    Arrange { layout: Option<Arrangement> },
    /// Scene step: the input engine takes the geometry left by the previous steps.
    Geometry,
    /// `machine` turns its display of monitor `monitor` back on (T-341). Never in a plan: the
    /// orchestrator asks for it right before a [`Step::SwitchMonitor`] to that machine's input,
    /// so the monitor does not land on a dark input.
    DisplayOn { monitor: String, machine: Machine },
}

impl Step {
    /// Reflects in `state` the effect of the step already applied.
    pub fn apply_to(&self, state: &mut State) {
        match self {
            Step::PowerOn { machine } => state.on.set(*machine, true),
            Step::PowerOff { machine } => state.on.set(*machine, false),
            // The real input is reported by the firmware (owner's events), not by the state.
            Step::SwitchMonitor { .. } => {}
            // The active scene is recorded when the transaction ends, not step by step.
            Step::ShowMachine { .. } | Step::Arrange { .. } | Step::Geometry => {}
            Step::DisplayOn { .. } => {}
        }
    }

    /// Whether it is a scene step: done by the machine that applies it, never the other on request.
    pub fn is_scene(&self) -> bool {
        matches!(
            self,
            Step::ShowMachine { .. } | Step::Arrange { .. } | Step::Geometry
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Plan {
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error(
        "machine {0} is off and there is no way to power it on: the MAC of its network card (Wake-on-LAN) is missing; it is learned when pairing or written on the machine's page"
    )]
    CannotWake(Machine),
}

impl Plan {
    /// Power on `machine`: just wake it. Asking for what is already on is an empty plan.
    pub fn power_on(
        from: &State,
        machine: Machine,
        abilities: &Abilities,
    ) -> Result<Plan, PlanError> {
        if from.is_on(machine) {
            return Ok(Plan::default());
        }
        if !abilities.can_wake(machine) {
            return Err(PlanError::CannotWake(machine));
        }
        Ok(Plan {
            steps: vec![Step::PowerOn { machine }],
        })
    }

    /// Switch a monitor's input: a single step, with verdict and rollback.
    pub fn switch_monitor(monitor: impl Into<String>, input: MonitorInput) -> Plan {
        Plan {
            steps: vec![Step::SwitchMonitor {
                monitor: monitor.into(),
                input,
            }],
        }
    }

    /// Clean shutdown of `machine`. The keyboard and mouse return on their own to the one that remains: the
    /// input engine takes the machine being powered off out of the geometry as soon as the state says so.
    /// Shutting down what is already off is an empty plan.
    pub fn power_off(from: &State, machine: Machine) -> Plan {
        if !from.is_on(machine) {
            return Plan::default();
        }
        Plan {
            steps: vec![Step::PowerOff { machine }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_on_only_wakes_what_is_off() {
        let s = State::initial(Machine::A);
        assert_eq!(
            Plan::power_on(&s, Machine::B, &Abilities::both_wake())
                .unwrap()
                .steps,
            [Step::PowerOn {
                machine: Machine::B
            }]
        );
        assert!(
            Plan::power_on(&s, Machine::A, &Abilities::both_wake())
                .unwrap()
                .steps
                .is_empty()
        );
    }

    #[test]
    fn a_machine_that_cannot_be_woken_is_a_plan_error() {
        let mut a = Abilities::both_wake();
        a.wakeable.remove(&Machine::A);
        assert_eq!(
            Plan::power_on(&State::initial(Machine::B), Machine::A, &a).unwrap_err(),
            PlanError::CannotWake(Machine::A)
        );
    }

    #[test]
    fn power_off_only_shuts_down_what_is_on() {
        let mut s = State::initial(Machine::A);
        assert!(Plan::power_off(&s, Machine::B).steps.is_empty());
        s.on.b = true;
        assert_eq!(
            Plan::power_off(&s, Machine::B).steps,
            [Step::PowerOff {
                machine: Machine::B
            }]
        );
    }

    #[test]
    fn switching_a_monitor_is_one_step_and_leaves_power_alone() {
        let plan = Plan::switch_monitor("lc34g55t-0a1b2c3d", MonitorInput::Hdmi);
        assert_eq!(
            plan.steps,
            [Step::SwitchMonitor {
                monitor: "lc34g55t-0a1b2c3d".into(),
                input: MonitorInput::Hdmi
            }]
        );
        let mut s = State::initial(Machine::A);
        let before = s.clone();
        plan.steps[0].apply_to(&mut s);
        assert_eq!(s, before);
    }

    #[test]
    fn steps_update_who_is_on() {
        let mut s = State::initial(Machine::A);
        Step::PowerOn {
            machine: Machine::B,
        }
        .apply_to(&mut s);
        assert!(s.is_on(Machine::B));
        Step::PowerOff {
            machine: Machine::B,
        }
        .apply_to(&mut s);
        assert!(!s.is_on(Machine::B));
    }
}
