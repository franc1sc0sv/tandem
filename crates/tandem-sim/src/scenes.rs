//! El doble de los pasos de escena (T-100): qué máquina muestra cada monitor de escena y
//! cuántas veces se recalculó la geometría. T-111 conecta el paso abstracto a los monitores
//! reales; aquí cada cambio se aplica al instante salvo que el [`crate::FaultPlan`] diga otra
//! cosa (los fallos se buscan con el paso de escena y la máquina que la aplica).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tandem_core::{Machine, Step};
use tandemd::agent::SceneBackend;
use tandemd::orchestrator::{StepFuture, describe};

use crate::fault::{FaultAction, Faults};

#[derive(Debug, Default)]
struct Screens {
    shows: BTreeMap<String, Machine>,
    /// Cuántas veces cada máquina llevó el motor de entrada a la geometría nueva.
    geometry: BTreeMap<Machine, u32>,
    /// Cada cambio de monitor hecho, en orden: (quién lo pidió, monitor, máquina).
    log: Vec<(Machine, String, Machine)>,
}

/// Los monitores de escena del puesto, compartidos por las dos máquinas.
#[derive(Debug, Clone, Default)]
pub struct SceneScreens(Arc<Mutex<Screens>>);

impl SceneScreens {
    /// Monitores que muestran, al empezar, la máquina indicada.
    pub fn new(monitors: &[(&str, Machine)]) -> Self {
        let s = SceneScreens::default();
        s.lock().shows = monitors
            .iter()
            .map(|(m, machine)| ((*m).to_owned(), *machine))
            .collect();
        s
    }

    fn lock(&self) -> MutexGuard<'_, Screens> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Qué muestra cada monitor ahora.
    pub fn shows(&self) -> BTreeMap<String, Machine> {
        self.lock().shows.clone()
    }

    /// Los cambios de monitor hechos, en orden.
    pub fn log(&self) -> Vec<(Machine, String, Machine)> {
        self.lock().log.clone()
    }

    /// Cuántas veces `machine` recalculó la geometría por una escena.
    pub fn geometry_of(&self, machine: Machine) -> u32 {
        self.lock().geometry.get(&machine).copied().unwrap_or(0)
    }

    pub(crate) fn backend(&self, machine: Machine, faults: Faults) -> SimSceneBackend {
        SimSceneBackend {
            machine,
            screens: self.clone(),
            faults,
        }
    }
}

/// El [`SceneBackend`] simulado de una máquina.
#[derive(Debug)]
pub(crate) struct SimSceneBackend {
    machine: Machine,
    screens: SceneScreens,
    faults: Faults,
}

impl SimSceneBackend {
    /// Lo que dice el plan de fallos para `step`; `Ok(())` si se aplica.
    async fn faults(&self, step: &Step) -> Result<(), String> {
        for action in self.faults.take(self.machine, step) {
            match action {
                FaultAction::Fail(why) => return Err(format!("{}: {why}", describe(step))),
                FaultAction::Hang => std::future::pending::<()>().await,
                FaultAction::Delay(d) => tokio::time::sleep(d).await,
                // Los pasos de escena no van por la red.
                FaultAction::CutLink(_) | FaultAction::ApplyThenCut(_) => {}
            }
        }
        Ok(())
    }
}

impl SceneBackend for SimSceneBackend {
    fn shows(&self, monitor: &str) -> Result<Machine, String> {
        self.screens
            .lock()
            .shows
            .get(monitor)
            .copied()
            .ok_or_else(|| format!("el monitor {monitor} no existe"))
    }

    fn show<'a>(&'a self, monitor: &'a str, machine: Machine) -> StepFuture<'a> {
        Box::pin(async move {
            let step = Step::ShowMachine {
                monitor: monitor.to_owned(),
                machine,
            };
            self.faults(&step).await?;
            let mut s = self.screens.lock();
            if !s.shows.contains_key(monitor) {
                return Err(format!("el monitor {monitor} no existe"));
            }
            s.shows.insert(monitor.to_owned(), machine);
            s.log.push((self.machine, monitor.to_owned(), machine));
            Ok(())
        })
    }

    fn geometry(&self) -> StepFuture<'_> {
        Box::pin(async move {
            self.faults(&Step::Geometry).await?;
            *self
                .screens
                .lock()
                .geometry
                .entry(self.machine)
                .or_default() += 1;
            Ok(())
        })
    }
}
