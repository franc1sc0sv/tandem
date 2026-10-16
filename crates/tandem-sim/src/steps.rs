//! Los pasos de cada máquina sobre el puesto simulado: el [`LocalSteps`] que usan
//! los agentes de la simulación en lugar de los backends de plataforma.

use std::time::Duration;

use tandem_core::{Config, Machine, Step};
use tandem_link::manager::LinkKind;
use tandemd::orchestrator::{LocalSteps, StepFuture, describe};
use tokio::sync::watch;

use crate::fault::{FaultAction, Faults};
use crate::net::SimNet;
use crate::world::{Power, World, code_for};

/// Tiempos del hardware simulado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardwareTimes {
    /// Desde el Wake-on-LAN hasta que Windows terminó de arrancar.
    pub boot: Duration,
    /// Desde el Wake-on-LAN hasta que la Mac salió del reposo.
    pub wake_mac: Duration,
    /// Desde la orden de apagado hasta que la máquina se apaga.
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
    /// Lo que tarda `machine` en estar lista tras el Wake-on-LAN (o el botón).
    pub fn boot_of(&self, machine: Machine) -> Duration {
        match machine {
            Machine::Pc => self.boot,
            Machine::Mac => self.wake_mac,
        }
    }
}

/// Los backends simulados de una máquina (pantallas, energía, entrada).
pub struct SimSteps {
    machine: Machine,
    cfg: Config,
    world: World,
    net: SimNet,
    faults: Faults,
    times: HardwareTimes,
    /// Si su gestor de enlaces ve al agente de la otra máquina.
    peer_up: watch::Receiver<bool>,
}

impl SimSteps {
    pub(crate) fn new(
        machine: Machine,
        cfg: Config,
        world: World,
        net: SimNet,
        faults: Faults,
        times: HardwareTimes,
        peer_up: watch::Receiver<bool>,
    ) -> Self {
        SimSteps {
            machine,
            cfg,
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
                // Contesta antes de apagarse, como `shutdown /s /t 0` tras responder.
                tokio::spawn(async move {
                    tokio::time::sleep(after).await;
                    world.lock().power_off(machine);
                    net.drop_all();
                });
                Ok(())
            }
            Step::SwitchInput { monitor, input, .. } => {
                let code = code_for(&self.cfg, monitor, *input)
                    .ok_or_else(|| format!("{monitor}: no está en la configuración"))?;
                self.world.lock().write_input(monitor, code)
            }
            Step::Attach { machine, monitor } => {
                self.world
                    .lock()
                    .desktops
                    .entry(*machine)
                    .or_default()
                    .insert(monitor.clone());
                Ok(())
            }
            Step::Detach { machine, monitor } => {
                self.world
                    .lock()
                    .desktops
                    .entry(*machine)
                    .or_default()
                    .remove(monitor);
                Ok(())
            }
            Step::SetBuiltin { to, .. } => {
                self.world.lock().builtin = *to;
                Ok(())
            }
            Step::UpdateInputGeometry => {
                let mut w = self.world.lock();
                let screens = w.screens(self.machine);
                w.geometry.insert(self.machine, screens);
                Ok(())
            }
        }
    }

    /// Wake-on-LAN y esperar a que el agente de `machine` conteste por algún enlace.
    ///
    /// El paquete mágico solo sale por Ethernet (DESIGN §6): sin ese cable la otra no
    /// se entera y el paso vence por tiempo.
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
            .map_err(|_| "la simulación terminó".to_owned())
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
                        "la {} recibió «{}» estando {:?}",
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
