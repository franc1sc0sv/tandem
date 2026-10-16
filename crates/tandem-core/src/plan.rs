use std::collections::BTreeMap;

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

/// Un paso atómico. Lleva el estado anterior para poder deshacerse solo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "step")]
pub enum Step {
    PowerOnPc,
    PowerOffPc,
    /// Cambiar la entrada del monitor para que muestre `to`.
    SwitchInput {
        monitor: MonitorId,
        from: Machine,
        to: Machine,
        input: Input,
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
    /// Encender o apagar la PC no se deshace: dejarla prendida es inofensivo y
    /// apagarla a mitad de un fallo empeoraría las cosas.
    pub fn inverse(&self, cfg: &Config) -> Option<Step> {
        Some(match self {
            Step::PowerOnPc | Step::PowerOffPc => return None,
            Step::SwitchInput {
                monitor, from, to, ..
            } => Step::SwitchInput {
                monitor: monitor.clone(),
                from: *to,
                to: *from,
                input: cfg.monitor(monitor)?.input_for(*from)?,
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
            Step::PowerOnPc => state.pc_on = true,
            Step::PowerOffPc => state.pc_on = false,
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
}

impl Plan {
    /// Pasos para llevar el puesto desde `from` hasta el reparto de `mode`.
    ///
    /// Orden: encender la PC si hace falta → por cada monitor que se mueve, cambiar la
    /// entrada, agregarlo en su nueva máquina y quitarlo de la anterior → pantalla
    /// integrada → geometría del motor de entrada.
    ///
    /// La PC hace falta si va a mostrar algo y también si hay que cambiar alguna
    /// entrada, porque las cambia ella (DESIGN §8): tras un corte de corriente puede
    /// haber monitores en su entrada con la PC apagada.
    pub fn to_mode(cfg: &Config, from: &State, mode: &Mode) -> Result<Plan, PlanError> {
        let target = mode.assignment(cfg);
        let mut steps = Vec::new();

        let mut next = from.clone();
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
            steps.push(Step::SwitchInput {
                monitor: m.id.clone(),
                from: cur,
                to,
                input,
            });
            steps.push(Step::Attach {
                machine: to,
                monitor: m.id.clone(),
            });
            steps.push(Step::Detach {
                machine: cur,
                monitor: m.id.clone(),
            });
            next.owner.insert(m.id.clone(), to);
        }

        if !from.pc_on && (!steps.is_empty() || target.values().any(|m| *m == Machine::Pc)) {
            steps.insert(0, Step::PowerOnPc);
        }

        let (before, after) = (from.builtin_effective(cfg), next.builtin_effective(cfg));
        if before != after {
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

    /// Apagado limpio: todo a la Mac primero, para que nunca se quede sin pantallas.
    ///
    /// Si la PC estaba apagada pero hubo que encenderla para devolver un monitor, se
    /// vuelve a apagar al final.
    pub fn shutdown_pc(cfg: &Config, from: &State) -> Result<Plan, PlanError> {
        let mut plan = Plan::to_mode(cfg, from, &Mode::SoloMac)?;
        if from.pc_on || plan.steps.first() == Some(&Step::PowerOnPc) {
            plan.steps.push(Step::PowerOffPc);
        }
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixtures::desk;

    fn shared_34_to_pc() -> Mode {
        Mode::Shared([("m34".into(), Machine::Pc)].into())
    }

    #[test]
    fn same_mode_is_empty_plan() {
        let cfg = desk();
        let s = State::initial(&cfg);
        assert!(
            Plan::to_mode(&cfg, &s, &Mode::SoloMac)
                .unwrap()
                .steps
                .is_empty()
        );
    }

    #[test]
    fn shared_from_cold_powers_on_first() {
        let cfg = desk();
        let s = State::initial(&cfg);
        let plan = Plan::to_mode(&cfg, &s, &shared_34_to_pc()).unwrap();
        assert_eq!(
            plan.steps,
            vec![
                Step::PowerOnPc,
                Step::SwitchInput {
                    monitor: "m34".into(),
                    from: Machine::Mac,
                    to: Machine::Pc,
                    input: Input::DisplayPort
                },
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
        let cfg = desk();
        let mut s = State::initial(&cfg);
        s.pc_on = true;
        let plan = Plan::to_mode(&cfg, &s, &Mode::SoloPc).unwrap();
        assert!(!plan.steps.contains(&Step::PowerOnPc));
        assert!(plan.steps.contains(&Step::SetBuiltin {
            from: BuiltinDisplay::Off,
            to: BuiltinDisplay::Extend
        }));
        assert_eq!(plan.steps.last(), Some(&Step::UpdateInputGeometry));
    }

    #[test]
    fn shutdown_returns_everything_to_mac_before_power_off() {
        let cfg = desk();
        let mut s = State::initial(&cfg);
        s.pc_on = true;
        s.owner.insert("m34".into(), Machine::Pc);
        let plan = Plan::shutdown_pc(&cfg, &s).unwrap();
        assert!(matches!(
            plan.steps.first(),
            Some(Step::SwitchInput {
                to: Machine::Mac,
                ..
            })
        ));
        assert_eq!(plan.steps.last(), Some(&Step::PowerOffPc));
    }

    /// Tras un corte de corriente: el 34" sigue en la entrada de la PC, apagada.
    fn stranded_34() -> State {
        let mut s = State::initial(&desk());
        s.owner.insert("m34".into(), Machine::Pc);
        s
    }

    #[test]
    fn back_to_mac_with_the_pc_off_powers_it_on_first() {
        let cfg = desk();
        let plan = Plan::to_mode(&cfg, &stranded_34(), &Mode::SoloMac).unwrap();
        assert_eq!(plan.steps.first(), Some(&Step::PowerOnPc));
        assert!(plan.steps.contains(&Step::SwitchInput {
            monitor: "m34".into(),
            from: Machine::Pc,
            to: Machine::Mac,
            input: Input::Hdmi
        }));
    }

    #[test]
    fn staying_on_the_pc_with_the_pc_off_only_powers_it_on() {
        let cfg = desk();
        let plan = Plan::to_mode(&cfg, &stranded_34(), &shared_34_to_pc()).unwrap();
        assert_eq!(plan.steps, vec![Step::PowerOnPc, Step::UpdateInputGeometry]);
    }

    #[test]
    fn shutdown_with_a_stranded_monitor_powers_on_and_off_again() {
        let cfg = desk();
        let plan = Plan::shutdown_pc(&cfg, &stranded_34()).unwrap();
        assert_eq!(plan.steps.first(), Some(&Step::PowerOnPc));
        assert_eq!(plan.steps.last(), Some(&Step::PowerOffPc));
        // Con todo en la Mac y la PC apagada no hay nada que hacer.
        let off = State::initial(&cfg);
        assert!(Plan::shutdown_pc(&cfg, &off).unwrap().steps.is_empty());
    }

    #[test]
    fn switch_inverse_resolves_input() {
        let cfg = desk();
        let step = Step::SwitchInput {
            monitor: "m34".into(),
            from: Machine::Mac,
            to: Machine::Pc,
            input: Input::DisplayPort,
        };
        assert_eq!(
            step.inverse(&cfg),
            Some(Step::SwitchInput {
                monitor: "m34".into(),
                from: Machine::Pc,
                to: Machine::Mac,
                input: Input::Hdmi
            })
        );
    }
}
