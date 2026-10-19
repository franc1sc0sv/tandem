//! Ejecución transaccional de un [`Plan`].
//!
//! Los pasos se aplican en orden. Si uno falla, los ya aplicados se deshacen en
//! orden inverso y el estado vuelve al de partida; la geometría del motor de entrada
//! se recalcula al final de la vuelta atrás. Los timeouts por paso son
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
                // La geometría se calcula con lo que se ve al ejecutarla: se recalcula
                // una sola vez, al final, con todo ya deshecho. Si el paso que falló es
                // ella misma, pudo haber quedado hecha en una de las dos máquinas.
                let mut geometry = *step == Step::UpdateInputGeometry;
                // Orden de la vuelta atrás (T-089): primero se reactiva lo quitado, después
                // vuelven las entradas y por último se quitan las salidas que el plan había
                // agregado, y solo las de un monitor cuya entrada volvió: quitar la salida
                // mientras el monitor sigue en esa entrada lo dejaría en negro (DESIGN §8).
                let mut reactivate = Vec::new();
                let mut inputs = Vec::new();
                let mut deactivate = Vec::new();
                for done in applied.iter().rev() {
                    if *done == Step::UpdateInputGeometry {
                        geometry = true;
                        continue;
                    }
                    let Some(inv) = done.undo(cfg, &snapshot) else {
                        continue;
                    };
                    match inv {
                        Step::Attach { .. } => reactivate.push(inv),
                        Step::Detach { .. } => deactivate.push(inv),
                        _ => inputs.push(inv),
                    }
                }
                let mut stuck: Vec<crate::model::MonitorId> = Vec::new();
                for inv in reactivate.into_iter().chain(inputs) {
                    match exec.execute(&inv).await {
                        Ok(()) => inv.apply_to(state),
                        Err(e) => {
                            if let Step::SwitchInput { monitor, .. } = &inv {
                                stuck.push(monitor.clone());
                            }
                            undo_failures.push((inv, e));
                        }
                    }
                }
                for inv in deactivate {
                    if let Step::Detach { monitor, .. } = &inv
                        && stuck.contains(monitor)
                    {
                        continue;
                    }
                    match exec.execute(&inv).await {
                        Ok(()) => inv.apply_to(state),
                        Err(e) => undo_failures.push((inv, e)),
                    }
                }
                if geometry && let Err(e) = exec.execute(&Step::UpdateInputGeometry).await {
                    undo_failures.push((Step::UpdateInputGeometry, e));
                }
                let step = step.clone();
                return if undo_failures.is_empty() {
                    // Los pasos sin inverso (encender una máquina) dejan su efecto, y eso es lo correcto.
                    state.owner = snapshot.owner;
                    state.builtin_pref = snapshot.builtin_pref;
                    // Deshecho, se sabe lo mismo que antes: lo que estaba sin confirmar sigue así.
                    state.doubt = snapshot.doubt;
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
    use crate::plan::{Abilities, Mode};

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
        let mut s = State::initial(&cfg, Machine::Mac).confirmed();
        let plan = Plan::to_mode(&cfg, &s, &mode(), &Abilities::pc_switches(&cfg)).unwrap();
        let mut exec = Fake::default();
        let report = pollster::block_on(run(&cfg, &mut s, &plan, &mut exec)).unwrap();
        assert_eq!(report.applied, plan.steps);
        assert!(s.on.pc);
        assert_eq!(s.owner[&"m34".into()], Machine::Pc);
    }

    #[test]
    fn failure_rolls_back_in_reverse() {
        let cfg = desk();
        let mut s = State::initial(&cfg, Machine::Mac).confirmed();
        let plan = Plan::to_mode(&cfg, &s, &mode(), &Abilities::pc_switches(&cfg)).unwrap();
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
        assert!(s.on.pc, "la PC se queda prendida");

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
                Step::SwitchInput {
                    to: Machine::Mac,
                    ..
                }
            ),
            "primero la entrada"
        );
        assert!(
            matches!(
                undo[1],
                Step::Detach {
                    machine: Machine::Pc,
                    ..
                }
            ),
            "la salida de la PC se quita al final (T-089)"
        );
    }

    /// T-089: al fallar el último paso, la vuelta atrás quitaba la salida de la PC
    /// (inverso del `Attach`) antes de devolver la entrada: el monitor seguía en la PC y
    /// sin salida activa. Primero se reactiva, después se devuelve la entrada y por último
    /// se quita, y solo si la entrada volvió.
    #[test]
    fn rollback_never_removes_an_output_before_the_input_is_back() {
        let cfg = desk();
        let mut s = State::initial(&cfg, Machine::Mac).confirmed();
        let plan = Plan::to_mode(&cfg, &s, &mode(), &Abilities::pc_switches(&cfg)).unwrap();
        let detach_mac = Step::Detach {
            machine: Machine::Mac,
            monitor: "m34".into(),
        };
        let mut exec = Fake {
            fail_on: vec![detach_mac],
            ..Default::default()
        };
        pollster::block_on(run(&cfg, &mut s, &plan, &mut exec)).unwrap_err();
        let undo = &exec.log[plan
            .steps
            .iter()
            .position(|st| matches!(st, Step::Detach { .. }))
            .unwrap()
            + 1..];
        let at = |f: &dyn Fn(&Step) -> bool| undo.iter().position(f).unwrap();
        let switch_back = at(&|st| {
            matches!(
                st,
                Step::SwitchInput {
                    to: Machine::Mac,
                    ..
                }
            )
        });
        let drop_pc = at(&|st| {
            matches!(
                st,
                Step::Detach {
                    machine: Machine::Pc,
                    ..
                }
            )
        });
        assert!(switch_back < drop_pc, "{undo:?}");
    }

    /// Si la entrada no volvió, la salida de la PC se queda activa.
    #[test]
    fn rollback_keeps_the_output_when_the_input_cannot_come_back() {
        let cfg = desk();
        let mut s = State::initial(&cfg, Machine::Mac).confirmed();
        let plan = Plan::to_mode(&cfg, &s, &mode(), &Abilities::pc_switches(&cfg)).unwrap();
        let back = Step::SwitchInput {
            monitor: "m34".into(),
            from: Machine::Pc,
            to: Machine::Mac,
            input: crate::model::Input::Hdmi,
            by: Machine::Pc,
        };
        let mut exec = Fake {
            fail_on: vec![
                Step::Detach {
                    machine: Machine::Mac,
                    monitor: "m34".into(),
                },
                back,
            ],
            ..Default::default()
        };
        let err = pollster::block_on(run(&cfg, &mut s, &plan, &mut exec)).unwrap_err();
        assert!(matches!(err, TxnError::Inconsistent { .. }));
        let first_undo = plan
            .steps
            .iter()
            .position(|st| matches!(st, Step::Detach { .. }))
            .unwrap()
            + 1;
        assert!(
            !exec.log[first_undo..].iter().any(|st| matches!(
                st,
                Step::Detach {
                    machine: Machine::Pc,
                    ..
                }
            )),
            "{:?}",
            exec.log
        );
    }

    #[test]
    fn failed_undo_reports_inconsistent() {
        let cfg = desk();
        let mut s = State::initial(&cfg, Machine::Mac).confirmed();
        let plan = Plan::to_mode(&cfg, &s, &mode(), &Abilities::pc_switches(&cfg)).unwrap();
        let back = Step::SwitchInput {
            monitor: "m34".into(),
            from: Machine::Pc,
            to: Machine::Mac,
            input: crate::model::Input::Hdmi,
            by: Machine::Pc,
        };
        let mut exec = Fake {
            fail_on: vec![Step::UpdateInputGeometry, back],
            ..Default::default()
        };
        let err = pollster::block_on(run(&cfg, &mut s, &plan, &mut exec)).unwrap_err();
        assert!(matches!(err, TxnError::Inconsistent { .. }));
    }

    /// La geometría se calcula con lo que se ve en ese momento: al deshacer, se
    /// recalcula al final, con los monitores ya de vuelta, y no en su lugar en la lista.
    #[test]
    fn rollback_recomputes_geometry_last() {
        let cfg = desk();
        let mut s = State::initial(&cfg, Machine::Mac).confirmed();
        s.on.pc = true;
        s.owner.insert("m34".into(), Machine::Pc);
        let plan = Plan::power_off(&cfg, &s, Machine::Pc, &Abilities::pc_switches(&cfg)).unwrap();
        assert_eq!(
            plan.steps.last(),
            Some(&Step::PowerOff {
                machine: Machine::Pc
            })
        );
        let mut exec = Fake {
            fail_on: vec![Step::PowerOff {
                machine: Machine::Pc,
            }],
            ..Default::default()
        };
        let err = pollster::block_on(run(&cfg, &mut s, &plan, &mut exec)).unwrap_err();
        assert!(matches!(err, TxnError::RolledBack { .. }));
        let undo = &exec.log[plan.steps.len()..];
        assert_eq!(undo.last(), Some(&Step::UpdateInputGeometry), "{undo:?}");
        assert_eq!(
            undo.iter()
                .filter(|st| **st == Step::UpdateInputGeometry)
                .count(),
            1
        );
    }

    /// Si falla el paso de la geometría, pudo haberse hecho en una de las dos
    /// máquinas: la vuelta atrás la recalcula al final.
    #[test]
    fn failed_geometry_step_is_recomputed_after_rollback() {
        let cfg = desk();
        let mut s = State::initial(&cfg, Machine::Mac).confirmed();
        s.on.pc = true;
        let plan = Plan::to_mode(&cfg, &s, &mode(), &Abilities::pc_switches(&cfg)).unwrap();
        let mut exec = FailOnce {
            log: Vec::new(),
            fail: Some(Step::UpdateInputGeometry),
        };
        let err = pollster::block_on(run(&cfg, &mut s, &plan, &mut exec)).unwrap_err();
        assert!(matches!(err, TxnError::RolledBack { .. }), "{err:?}");
        assert_eq!(exec.log.last(), Some(&Step::UpdateInputGeometry));
        assert_eq!(s, {
            let mut want = State::initial(&cfg, Machine::Mac).confirmed();
            want.on.pc = true;
            want
        });
    }

    /// Falla solo la primera vez que ve el paso.
    struct FailOnce {
        log: Vec<Step>,
        fail: Option<Step>,
    }

    impl Executor for FailOnce {
        fn execute(&mut self, step: &Step) -> impl Future<Output = Result<(), String>> + Send {
            self.log.push(step.clone());
            let r = if self.fail.as_ref() == Some(step) {
                self.fail = None;
                Err("boom".to_owned())
            } else {
                Ok(())
            };
            async move { r }
        }
    }
}
