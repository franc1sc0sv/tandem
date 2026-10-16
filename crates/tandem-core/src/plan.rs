use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::model::{Config, Input, Machine, MonitorId};
use crate::state::{BuiltinDisplay, State};

/// Modo de trabajo elegido por el usuario.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode", content = "assign")]
pub enum Mode {
    SoloMac,
    /// Reparto explícito; los monitores que no aparecen se quedan en la Mac.
    Shared(BTreeMap<MonitorId, Machine>),
    SoloPc,
}

impl Mode {
    /// Todos los monitores en `machine`.
    pub fn solo(machine: Machine) -> Self {
        match machine {
            Machine::Mac => Mode::SoloMac,
            Machine::Pc => Mode::SoloPc,
        }
    }

    pub fn assignment(&self, cfg: &Config) -> BTreeMap<MonitorId, Machine> {
        cfg.monitors
            .iter()
            .map(|m| {
                let owner = match self {
                    Mode::SoloMac => Machine::Mac,
                    Mode::SoloPc => Machine::Pc,
                    Mode::Shared(a) => a.get(&m.id).copied().unwrap_or(Machine::Mac),
                };
                (m.id.clone(), owner)
            })
            .collect()
    }
}

/// Lo que cada máquina sabe hacer, según lo que anunció (DESIGN §3.7): quién cambia la
/// entrada de cada monitor y a quién se puede encender. Los planes salen de aquí, no
/// del nombre de la máquina.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Abilities {
    /// Las máquinas que pueden cambiar la entrada de cada monitor, en orden de preferencia.
    pub switchers: BTreeMap<MonitorId, Vec<Machine>>,
    /// Las máquinas que la otra puede encender (Wake-on-LAN).
    pub wakeable: BTreeSet<Machine>,
}

impl Abilities {
    /// El puesto de hoy (DESIGN §8): la PC cambia todas las entradas por DDC y las dos
    /// se pueden despertar. Sirve de base para las pruebas.
    pub fn pc_switches(cfg: &Config) -> Self {
        Abilities {
            switchers: cfg
                .monitors
                .iter()
                .map(|m| (m.id.clone(), vec![Machine::Pc]))
                .collect(),
            wakeable: BTreeSet::from([Machine::Mac, Machine::Pc]),
        }
    }

    /// Quién cambia la entrada de `monitor`: la primera que pueda y esté encendida; si
    /// ninguna lo está, la primera (habrá que encenderla).
    pub fn switcher(&self, monitor: &MonitorId, state: &State) -> Option<Machine> {
        let all = self.switchers.get(monitor)?;
        all.iter()
            .copied()
            .find(|m| state.is_on(*m))
            .or_else(|| all.first().copied())
    }

    pub fn can_wake(&self, machine: Machine) -> bool {
        self.wakeable.contains(&machine)
    }
}

/// Un paso atómico. Lleva el estado anterior para poder deshacerse solo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "step")]
pub enum Step {
    /// Encender `machine` desde la otra (Wake-on-LAN) y esperar a que su agente conteste.
    PowerOn { machine: Machine },
    /// Apagado limpio de `machine`.
    PowerOff { machine: Machine },
    /// Cambiar la entrada del monitor para que muestre `to`. Lo hace `by`, la máquina
    /// que puede (por DDC o pidiéndoselo al usuario).
    SwitchInput {
        monitor: MonitorId,
        from: Machine,
        to: Machine,
        input: Input,
        by: Machine,
    },
    /// Agregar el monitor al escritorio de `machine`.
    Attach {
        machine: Machine,
        monitor: MonitorId,
    },
    /// Quitar el monitor del escritorio de `machine` (evita el monitor fantasma).
    Detach {
        machine: Machine,
        monitor: MonitorId,
    },
    SetBuiltin {
        from: BuiltinDisplay,
        to: BuiltinDisplay,
    },
    /// Recalcular la geometría de pantallas del motor de teclado y mouse.
    UpdateInputGeometry,
}

impl Step {
    /// El paso que deshace a este, si tiene sentido deshacerlo.
    ///
    /// Encender o apagar una máquina no se deshace: dejarla prendida es inofensivo y
    /// apagarla a mitad de un fallo empeoraría las cosas.
    pub fn inverse(&self, cfg: &Config) -> Option<Step> {
        Some(match self {
            Step::PowerOn { .. } | Step::PowerOff { .. } => return None,
            Step::SwitchInput {
                monitor,
                from,
                to,
                by,
                ..
            } => Step::SwitchInput {
                monitor: monitor.clone(),
                from: *to,
                to: *from,
                input: cfg.monitor(monitor)?.input_for(*from)?,
                by: *by,
            },
            Step::Attach { machine, monitor } => Step::Detach {
                machine: *machine,
                monitor: monitor.clone(),
            },
            Step::Detach { machine, monitor } => Step::Attach {
                machine: *machine,
                monitor: monitor.clone(),
            },
            Step::SetBuiltin { from, to } => Step::SetBuiltin {
                from: *to,
                to: *from,
            },
            Step::UpdateInputGeometry => Step::UpdateInputGeometry,
        })
    }

    /// Refleja en `state` el efecto del paso ya aplicado.
    pub fn apply_to(&self, state: &mut State) {
        match self {
            Step::PowerOn { machine } => state.on.set(*machine, true),
            Step::PowerOff { machine } => state.on.set(*machine, false),
            Step::SwitchInput { monitor, to, .. } => {
                state.owner.insert(monitor.clone(), *to);
            }
            Step::SetBuiltin { .. }
            | Step::Attach { .. }
            | Step::Detach { .. }
            | Step::UpdateInputGeometry => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Plan {
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("el monitor `{0}` no está en la configuración")]
    UnknownMonitor(MonitorId),
    #[error("el monitor `{monitor}` no está cableado a {machine}")]
    NotWired {
        monitor: MonitorId,
        machine: Machine,
    },
    #[error("ninguna de las dos máquinas puede cambiar la entrada de `{0}`")]
    NoSwitcher(MonitorId),
    #[error("{0} está apagada y la otra máquina no puede encenderla")]
    CannotWake(Machine),
}

impl Plan {
    /// Pasos para llevar el puesto desde `from` hasta el reparto de `mode`.
    ///
    /// Orden: encender lo que haga falta → por cada monitor que se mueve, cambiar la
    /// entrada, agregarlo en su nueva máquina y quitarlo de la anterior → pantalla
    /// integrada → geometría del motor de entrada.
    ///
    /// Hace falta encendida cada máquina que va a mostrar algo y la que cambia cada
    /// entrada (`abilities`): tras un corte de corriente puede haber monitores en la
    /// entrada de una máquina apagada. Lo que tocaría el escritorio de una máquina que
    /// sigue apagada no se hace: no hay escritorio que tocar.
    pub fn to_mode(
        cfg: &Config,
        from: &State,
        mode: &Mode,
        abilities: &Abilities,
    ) -> Result<Plan, PlanError> {
        let target = mode.assignment(cfg);
        let mut needed: BTreeSet<Machine> = target.values().copied().collect();
        let mut moves = Vec::new();
        for m in &cfg.monitors {
            let cur = *from
                .owner
                .get(&m.id)
                .ok_or_else(|| PlanError::UnknownMonitor(m.id.clone()))?;
            let to = target[&m.id];
            if cur == to {
                continue;
            }
            let input = m.input_for(to).ok_or_else(|| PlanError::NotWired {
                monitor: m.id.clone(),
                machine: to,
            })?;
            let by = abilities
                .switcher(&m.id, from)
                .ok_or_else(|| PlanError::NoSwitcher(m.id.clone()))?;
            needed.insert(by);
            moves.push((m.id.clone(), cur, to, input, by));
        }

        let mut steps = Vec::new();
        let mut on = from.on;
        // Sin nada que mover ni mostrar, no se enciende nada.
        if !moves.is_empty() || target.values().any(|m| !from.is_on(*m)) {
            for machine in needed {
                if from.is_on(machine) {
                    continue;
                }
                if !abilities.can_wake(machine) {
                    return Err(PlanError::CannotWake(machine));
                }
                steps.push(Step::PowerOn { machine });
                on.set(machine, true);
            }
        }

        let mut next = from.clone();
        next.on = on;
        for (monitor, cur, to, input, by) in moves {
            steps.push(Step::SwitchInput {
                monitor: monitor.clone(),
                from: cur,
                to,
                input,
                by,
            });
            if on.is_on(to) {
                steps.push(Step::Attach {
                    machine: to,
                    monitor: monitor.clone(),
                });
            }
            if on.is_on(cur) {
                steps.push(Step::Detach {
                    machine: cur,
                    monitor: monitor.clone(),
                });
            }
            next.owner.insert(monitor, to);
        }

        let (before, after) = (from.builtin_effective(cfg), next.builtin_effective(cfg));
        if before != after && on.is_on(Machine::Mac) {
            steps.push(Step::SetBuiltin {
                from: before,
                to: after,
            });
        }
        if !steps.is_empty() {
            steps.push(Step::UpdateInputGeometry);
        }
        Ok(Plan { steps })
    }

    /// Apagado limpio de `machine`: todo a la otra primero, para que nunca se quede
    /// nadie sin pantallas.
    ///
    /// Si `machine` estaba apagada pero hubo que encenderla para devolver un monitor, se
    /// vuelve a apagar al final.
    pub fn power_off(
        cfg: &Config,
        from: &State,
        machine: Machine,
        abilities: &Abilities,
    ) -> Result<Plan, PlanError> {
        let mut plan = Plan::to_mode(cfg, from, &Mode::solo(machine.other()), abilities)?;
        if from.is_on(machine) || plan.steps.contains(&Step::PowerOn { machine }) {
            plan.steps.push(Step::PowerOff { machine });
        }
        Ok(plan)
    }

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixtures::desk;

    fn shared_34_to_pc() -> Mode {
        Mode::Shared([("m34".into(), Machine::Pc)].into())
    }

    fn abilities() -> Abilities {
        Abilities::pc_switches(&desk())
    }

    fn plan(from: &State, mode: &Mode) -> Vec<Step> {
        Plan::to_mode(&desk(), from, mode, &abilities())
            .unwrap()
            .steps
    }

    /// Visto desde la Mac, con la PC apagada.
    fn mac_alone() -> State {
        State::initial(&desk(), Machine::Mac)
    }

    /// Visto desde la PC, con la Mac apagada (o suspendida) y todo en la PC.
    fn pc_alone() -> State {
        let mut s = State::initial(&desk(), Machine::Pc);
        for o in s.owner.values_mut() {
            *o = Machine::Pc;
        }
        s
    }

    fn switch(monitor: &str, from: Machine, to: Machine, input: Input) -> Step {
        Step::SwitchInput {
            monitor: monitor.into(),
            from,
            to,
            input,
            by: Machine::Pc,
        }
    }

    #[test]
    fn same_mode_is_empty_plan() {
        assert!(plan(&mac_alone(), &Mode::SoloMac).is_empty());
        assert!(plan(&pc_alone(), &Mode::SoloPc).is_empty());
    }

    #[test]
    fn shared_from_cold_powers_on_first() {
        assert_eq!(
            plan(&mac_alone(), &shared_34_to_pc()),
            vec![
                Step::PowerOn {
                    machine: Machine::Pc
                },
                switch("m34", Machine::Mac, Machine::Pc, Input::DisplayPort),
                Step::Attach {
                    machine: Machine::Pc,
                    monitor: "m34".into()
                },
                Step::Detach {
                    machine: Machine::Mac,
                    monitor: "m34".into()
                },
                Step::UpdateInputGeometry,
            ]
        );
    }

    #[test]
    fn solo_pc_turns_builtin_on() {
        let mut s = mac_alone();
        s.on.pc = true;
        let steps = plan(&s, &Mode::SoloPc);
        assert!(!steps.iter().any(|s| matches!(s, Step::PowerOn { .. })));
        assert!(steps.contains(&Step::SetBuiltin {
            from: BuiltinDisplay::Off,
            to: BuiltinDisplay::Extend
        }));
        assert_eq!(steps.last(), Some(&Step::UpdateInputGeometry));
    }

    #[test]
    fn shutdown_returns_everything_to_mac_before_power_off() {
        let mut s = mac_alone();
        s.on.pc = true;
        s.owner.insert("m34".into(), Machine::Pc);
        let plan = Plan::power_off(&desk(), &s, Machine::Pc, &abilities()).unwrap();
        assert!(matches!(
            plan.steps.first(),
            Some(Step::SwitchInput {
                to: Machine::Mac,
                ..
            })
        ));
        assert_eq!(
            plan.steps.last(),
            Some(&Step::PowerOff {
                machine: Machine::Pc
            })
        );
    }

    /// Tras un corte de corriente: el 34" sigue en la entrada de la PC, apagada.
    fn stranded_34() -> State {
        let mut s = mac_alone();
        s.owner.insert("m34".into(), Machine::Pc);
        s
    }

    #[test]
    fn back_to_mac_with_the_pc_off_powers_it_on_first() {
        let steps = plan(&stranded_34(), &Mode::SoloMac);
        assert_eq!(
            steps.first(),
            Some(&Step::PowerOn {
                machine: Machine::Pc
            })
        );
        assert!(steps.contains(&switch("m34", Machine::Pc, Machine::Mac, Input::Hdmi)));
    }

    #[test]
    fn staying_on_the_pc_with_the_pc_off_only_powers_it_on() {
        assert_eq!(
            plan(&stranded_34(), &shared_34_to_pc()),
            vec![
                Step::PowerOn {
                    machine: Machine::Pc
                },
                Step::UpdateInputGeometry
            ]
        );
    }

    #[test]
    fn shutdown_with_a_stranded_monitor_powers_on_and_off_again() {
        let cfg = desk();
        let plan = Plan::power_off(&cfg, &stranded_34(), Machine::Pc, &abilities()).unwrap();
        assert_eq!(
            plan.steps.first(),
            Some(&Step::PowerOn {
                machine: Machine::Pc
            })
        );
        assert_eq!(
            plan.steps.last(),
            Some(&Step::PowerOff {
                machine: Machine::Pc
            })
        );
        // Con todo en la Mac y la PC apagada no hay nada que hacer.
        assert!(
            Plan::power_off(&cfg, &mac_alone(), Machine::Pc, &abilities())
                .unwrap()
                .steps
                .is_empty()
        );
    }

    #[test]
    fn switch_inverse_resolves_input_and_keeps_who_switches() {
        let step = switch("m34", Machine::Mac, Machine::Pc, Input::DisplayPort);
        assert_eq!(
            step.inverse(&desk()),
            Some(switch("m34", Machine::Pc, Machine::Mac, Input::Hdmi))
        );
    }

    // La PC como principal: la Mac suspendida o apagada (DESIGN §3.7).

    #[test]
    fn with_the_mac_off_the_pc_takes_the_monitors_without_waking_it() {
        let mut s = State::initial(&desk(), Machine::Pc);
        // Lo último que se supo: todo en la Mac.
        assert!(!s.is_on(Machine::Mac));
        let steps = plan(&s, &Mode::SoloPc);
        assert!(!steps.iter().any(|s| matches!(s, Step::PowerOn { .. })));
        // Nada en el escritorio de la Mac, que está apagada; ni su pantalla integrada.
        assert!(!steps.iter().any(|s| matches!(
            s,
            Step::Detach {
                machine: Machine::Mac,
                ..
            } | Step::SetBuiltin { .. }
        )));
        assert!(steps.contains(&switch(
            "m32",
            Machine::Mac,
            Machine::Pc,
            Input::DisplayPort
        )));
        s.owner.values_mut().for_each(|o| *o = Machine::Pc);
        assert!(plan(&s, &Mode::SoloPc).is_empty());
    }

    #[test]
    fn a_monitor_for_the_sleeping_mac_wakes_it_first() {
        let steps = plan(
            &pc_alone(),
            &Mode::Shared([("m32".into(), Machine::Mac)].into()),
        );
        assert_eq!(
            steps.first(),
            Some(&Step::PowerOn {
                machine: Machine::Mac
            })
        );
        assert!(steps.contains(&Step::Attach {
            machine: Machine::Mac,
            monitor: "m32".into()
        }));
    }

    #[test]
    fn a_machine_that_cannot_be_woken_is_a_plan_error() {
        let mut a = abilities();
        a.wakeable.remove(&Machine::Mac);
        let e = Plan::to_mode(&desk(), &pc_alone(), &Mode::SoloMac, &a).unwrap_err();
        assert_eq!(e, PlanError::CannotWake(Machine::Mac));
        assert_eq!(
            Plan::power_on(&pc_alone(), Machine::Mac, &a).unwrap_err(),
            PlanError::CannotWake(Machine::Mac)
        );
    }

    #[test]
    fn the_pc_can_shut_down_the_mac_after_taking_its_monitors() {
        let mut s = pc_alone();
        s.on.mac = true;
        s.owner.insert("m32".into(), Machine::Mac);
        let plan = Plan::power_off(&desk(), &s, Machine::Mac, &abilities()).unwrap();
        assert_eq!(
            plan.steps.first(),
            Some(&switch(
                "m32",
                Machine::Mac,
                Machine::Pc,
                Input::DisplayPort
            ))
        );
        assert_eq!(
            plan.steps.last(),
            Some(&Step::PowerOff {
                machine: Machine::Mac
            })
        );
    }

    #[test]
    fn the_switcher_that_is_on_is_preferred() {
        let cfg = desk();
        let mut a = abilities();
        a.switchers
            .insert("m34".into(), vec![Machine::Pc, Machine::Mac]);
        // Con la PC apagada, la Mac cambia el 34" sola y no hace falta encender la PC.
        let steps = Plan::to_mode(&cfg, &stranded_34(), &Mode::SoloMac, &a)
            .unwrap()
            .steps;
        assert!(!steps.iter().any(|s| matches!(s, Step::PowerOn { .. })));
        assert!(steps.contains(&Step::SwitchInput {
            monitor: "m34".into(),
            from: Machine::Pc,
            to: Machine::Mac,
            input: Input::Hdmi,
            by: Machine::Mac,
        }));
        a.switchers.remove(&MonitorId::from("m34"));
        assert_eq!(
            Plan::to_mode(&cfg, &stranded_34(), &Mode::SoloMac, &a).unwrap_err(),
            PlanError::NoSwitcher("m34".into())
        );
    }
}
