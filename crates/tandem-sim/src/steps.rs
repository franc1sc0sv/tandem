//! Each machine's steps over the simulated desk: the [`LocalSteps`] the simulation's
//! agents use instead of the platform backends.

use std::time::Duration;

use tandem_core::{Machine, Step};
use tandem_link::manager::LinkKind;
use tandemd::orchestrator::{LocalSteps, StepFuture, describe};
use tokio::sync::watch;

use crate::fault::{FaultAction, Faults};
use crate::net::SimNet;
use crate::world::{Power, World};

/// Simulated hardware timings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardwareTimes {
    /// From the Wake-on-LAN until Windows finished starting.
    pub boot: Duration,
    /// From the Wake-on-LAN until the Mac came out of sleep.
    pub wake_mac: Duration,
    /// From the shutdown order until the machine turns off.
    pub shutdown: Duration,
}

impl Default for HardwareTimes {
    fn default() -> Self {
        HardwareTimes {
            boot: Duration::from_secs(45),
            wake_mac: Duration::from_secs(8),
            shutdown: Duration::from_secs(3),
        }
    }
}

impl HardwareTimes {
    /// How long `machine` takes to be ready after the Wake-on-LAN (or the button).
    pub fn boot_of(&self, machine: Machine) -> Duration {
        match machine {
            Machine::B => self.boot,
            Machine::A => self.wake_mac,
        }
    }
}

/// The simulated backends of a machine (power).
pub struct SimSteps {
    machine: Machine,
    world: World,
    net: SimNet,
    faults: Faults,
    times: HardwareTimes,
    /// Whether its link manager sees the other machine's agent.
    peer_up: watch::Receiver<bool>,
}

impl SimSteps {
    pub(crate) fn new(
        machine: Machine,
        world: World,
        net: SimNet,
        faults: Faults,
        times: HardwareTimes,
        peer_up: watch::Receiver<bool>,
    ) -> Self {
        SimSteps {
            machine,
            world,
            net,
            faults,
            times,
            peer_up,
        }
    }

    async fn apply(&self, step: &Step) -> Result<(), String> {
        match step {
            Step::PowerOn { machine } => self.power_on(*machine).await,
            Step::PowerOff { machine } => {
                let (world, net, after, machine) = (
                    self.world.clone(),
                    self.net.clone(),
                    self.times.shutdown,
                    *machine,
                );
                // It answers before shutting down, like `shutdown /s /t 0` after replying.
                tokio::spawn(async move {
                    tokio::time::sleep(after).await;
                    world.lock().power_off(machine);
                    net.drop_all();
                });
                Ok(())
            }
            // Monitors go through the simulated DDC bus (`crate::monitors`) and scene steps
            // through their double (`crate::scenes`), not through here.
            Step::SwitchMonitor { .. }
            | Step::ShowMachine { .. }
            | Step::Arrange { .. }
            | Step::Geometry
            | Step::DisplayOn { .. }
            | Step::MonitorPower { .. } => Err(format!("{}: not a power step", describe(step))),
        }
    }

    /// Wake-on-LAN and wait for `machine`'s agent to answer over some link.
    ///
    /// The magic packet only goes out over Ethernet (DESIGN §6): without that cable the other
    /// does not find out and the step times out.
    async fn power_on(&self, machine: Machine) -> Result<(), String> {
        let ethernet = self.net.is_up(LinkKind::Ethernet);
        let send_wol = {
            let mut w = self.world.lock();
            let off = w.power_of(machine) == Power::Off && ethernet;
            if off {
                w.set_power(machine, Power::Booting);
            }
            off
        };
        if send_wol {
            let (world, boot) = (self.world.clone(), self.times.boot_of(machine));
            tokio::spawn(async move {
                tokio::time::sleep(boot).await;
                world.lock().boot_done(machine);
            });
        }
        let mut up = self.peer_up.clone();
        up.wait_for(|up| *up)
            .await
            .map(|_| ())
            .map_err(|_| "the simulation ended".to_owned())
    }
}

impl LocalSteps for SimSteps {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
        Box::pin(async move {
            let actions = {
                let mut w = self.world.lock();
                w.calls.push((self.machine, step.clone()));
                if w.power_of(self.machine) != Power::On {
                    let v = format!(
                        "{} received «{}» while {:?}",
                        self.machine,
                        describe(step),
                        w.power_of(self.machine)
                    );
                    w.violations.push(v.clone());
                    return Err(v);
                }
                drop(w);
                self.faults.take(self.machine, step)
            };
            let mut cut_after = Vec::new();
            for action in actions {
                match action {
                    FaultAction::Fail(reason) => return Err(reason),
                    FaultAction::Hang => std::future::pending::<()>().await,
                    FaultAction::Delay(d) => tokio::time::sleep(d).await,
                    FaultAction::CutLink(kind) => self.net.cut(kind),
                    FaultAction::ApplyThenCut(kind) => cut_after.push(kind),
                }
            }
            self.apply(step).await?;
            for kind in cut_after {
                self.net.cut(kind);
            }
            Ok(())
        })
    }
}
