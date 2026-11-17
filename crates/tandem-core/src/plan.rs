use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::model::Machine;
use crate::state::State;

/// Lo que cada máquina sabe hacer, según lo que anunció (DESIGN §3.7): a quién se puede
/// encender. Los planes salen de aquí, no del nombre de la máquina.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Abilities {
    /// Las máquinas que la otra puede encender (Wake-on-LAN).
    pub wakeable: BTreeSet<Machine>,
}

impl Abilities {
    /// Las dos se pueden despertar. Sirve de base para las pruebas.
    pub fn both_wake() -> Self {
        Abilities {
            wakeable: BTreeSet::from([Machine::A, Machine::B]),
        }
    }

    pub fn can_wake(&self, machine: Machine) -> bool {
        self.wakeable.contains(&machine)
    }
}

/// Un paso atómico de una transacción.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "step")]
pub enum Step {
    /// Encender `machine` desde la otra (Wake-on-LAN) y esperar a que su agente conteste.
    PowerOn { machine: Machine },
    /// Apagado limpio de `machine`.
    PowerOff { machine: Machine },
}

impl Step {
    /// Refleja en `state` el efecto del paso ya aplicado.
    pub fn apply_to(&self, state: &mut State) {
        match self {
            Step::PowerOn { machine } => state.on.set(*machine, true),
            Step::PowerOff { machine } => state.on.set(*machine, false),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Plan {
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("la máquina {0} está apagada y la otra no puede encenderla")]
    CannotWake(Machine),
}

impl Plan {
    /// Encender `machine`: solo despertarla. Pedir lo que ya está es un plan vacío.
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

    /// Apagado limpio de `machine`. El teclado y el mouse vuelven solos a la que queda: el
    /// motor de entrada saca de la geometría a la que se apaga en cuanto el estado lo dice.
    /// Apagar lo que ya está apagado es un plan vacío.
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
