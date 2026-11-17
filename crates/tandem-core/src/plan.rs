use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::model::{Arrangement, Machine};
use crate::monitor::MonitorInput;
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
    /// Cambiar la entrada del monitor `monitor` (su id, `<modelo>-<hash>`) y esperar el
    /// veredicto del firmware. Lo hace la máquina dueña del monitor (DECISIONS D13), sea cual
    /// sea la que orquesta.
    SwitchMonitor {
        monitor: String,
        input: MonitorInput,
    },
    /// Paso de escena: que el monitor `monitor` muestre `machine` (DESIGN §5). Es el paso
    /// abstracto de T-100; T-111 lo traduce a [`Step::SwitchMonitor`] con la entrada de esa
    /// máquina. Lo hace la máquina que aplica la escena.
    ShowMachine { monitor: String, machine: Machine },
    /// Paso de escena: el acomodo de los escritorios (`None`: el de omisión). Es un cambio
    /// del puesto compartido.
    Arrange { layout: Option<Arrangement> },
    /// Paso de escena: el motor de entrada toma la geometría que dejan los pasos anteriores.
    Geometry,
}

impl Step {
    /// Refleja en `state` el efecto del paso ya aplicado.
    pub fn apply_to(&self, state: &mut State) {
        match self {
            Step::PowerOn { machine } => state.on.set(*machine, true),
            Step::PowerOff { machine } => state.on.set(*machine, false),
            // La entrada real la cuenta el firmware (eventos del dueño), no el estado.
            Step::SwitchMonitor { .. } => {}
            // La escena activa se anota al terminar la transacción, no paso a paso.
            Step::ShowMachine { .. } | Step::Arrange { .. } | Step::Geometry => {}
        }
    }

    /// Si es un paso de escena: lo hace la máquina que la aplica, nunca la otra a pedido.
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

    /// Cambiar la entrada de un monitor: un solo paso, con veredicto y vuelta atrás.
    pub fn switch_monitor(monitor: impl Into<String>, input: MonitorInput) -> Plan {
        Plan {
            steps: vec![Step::SwitchMonitor {
                monitor: monitor.into(),
                input,
            }],
        }
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
