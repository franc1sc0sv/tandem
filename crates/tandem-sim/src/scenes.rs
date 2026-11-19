//! The scene-step double (T-100): which machine shows each scene monitor and
//! how many times the geometry was recomputed. With real monitors on the desk
//! ([`crate::DeskBuilder::monitors`]) it is not used: the agents go to the
//! `monitor-ddc` doubles (T-111). Here every change is applied instantly unless the
//! [`crate::FaultPlan`] says otherwise (faults are looked up by the scene step and the
//! machine applying it).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tandem_core::{Machine, Step};
use tandemd::agent::SceneBackend;
use tandemd::orchestrator::{StepFuture, describe};

use crate::fault::{FaultAction, Faults};

#[derive(Debug, Default)]
struct Screens {
    shows: BTreeMap<String, Machine>,
    /// How many times each machine took the input engine to the new geometry.
    geometry: BTreeMap<Machine, u32>,
    /// Each monitor change made, in order: (who requested it, monitor, machine).
    log: Vec<(Machine, String, Machine)>,
}

/// The desk's scene monitors, shared by both machines.
#[derive(Debug, Clone, Default)]
pub struct SceneScreens(Arc<Mutex<Screens>>);

impl SceneScreens {
    /// Monitors that show the given machine at the start.
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

    /// What each monitor shows now.
    pub fn shows(&self) -> BTreeMap<String, Machine> {
        self.lock().shows.clone()
    }

    /// The monitor changes made, in order.
    pub fn log(&self) -> Vec<(Machine, String, Machine)> {
        self.lock().log.clone()
    }

    /// How many times `machine` recomputed the geometry because of a scene.
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

/// A machine's simulated [`SceneBackend`].
#[derive(Debug)]
pub(crate) struct SimSceneBackend {
    machine: Machine,
    screens: SceneScreens,
    faults: Faults,
}

impl SimSceneBackend {
    /// What the fault plan says for `step`; `Ok(())` if it is applied.
    async fn faults(&self, step: &Step) -> Result<(), String> {
        for action in self.faults.take(self.machine, step) {
            match action {
                FaultAction::Fail(why) => return Err(format!("{}: {why}", describe(step))),
                FaultAction::Hang => std::future::pending::<()>().await,
                FaultAction::Delay(d) => tokio::time::sleep(d).await,
                // Scene steps do not go over the network.
                FaultAction::CutLink(_) | FaultAction::ApplyThenCut(_) => {}
            }
        }
        Ok(())
    }
}

impl SceneBackend for SimSceneBackend {
    fn check(&self, monitor: &str, _machine: Machine) -> Result<(), String> {
        if self.screens.lock().shows.contains_key(monitor) {
            Ok(())
        } else {
            Err(format!("monitor {monitor} is not visible"))
        }
    }

    fn shows(&self, monitor: &str) -> Result<Machine, String> {
        self.screens
            .lock()
            .shows
            .get(monitor)
            .copied()
            .ok_or_else(|| format!("monitor {monitor} does not exist"))
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
                return Err(format!("monitor {monitor} does not exist"));
            }
            s.shows.insert(monitor.to_owned(), machine);
            s.log.push((self.machine, monitor.to_owned(), machine));
            Ok(())
        })
    }

    fn geometry<'a>(&'a self, _expected: &'a [(String, Machine)]) -> StepFuture<'a> {
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
