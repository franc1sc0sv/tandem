//! Monitores simulados (T-110): los dobles de `monitor-ddc` conectados a las dos máquinas.
//!
//! Un solo [`FakeBus`] guarda los monitores físicos; cada máquina los ve por su propio
//! [`MachineBus`], que solo contesta mientras esa máquina está prendida y tiene cable hacia
//! el monitor, tarda lo que diga su latencia (en el reloj del doble) y anota cada pedido con
//! la máquina que lo hizo. Así se comprueba quién leyó y quién escribió.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use monitor_ddc::fake::{BusOp, FakeBus, FakeClock, FakeMonitorConfig};
use monitor_ddc::{Clock, DdcBus, DdcError, Edid, MonitorHandle, MonitorInfo, VcpReply};
use tandem_core::{Machine, MonitorInput};

use crate::world::{Power, World};

/// Un monitor del puesto y las máquinas que tienen cable hacia él, con su latencia DDC.
#[derive(Debug, Clone)]
pub struct MonitorSpec {
    pub config: FakeMonitorConfig,
    pub cables: Vec<(Machine, Duration)>,
    /// La entrada que dice el EDID que lee cada máquina (cada entrada tiene el suyo, T-111).
    /// Sin dato, el EDID no dice el tipo de interfaz.
    pub via: Vec<(Machine, MonitorInput)>,
}

#[derive(Debug, Default)]
struct Shared {
    /// Monitor → máquina → latencia. Sin entrada, esa máquina no tiene cable.
    cables: BTreeMap<MonitorHandle, BTreeMap<Machine, Duration>>,
    /// Monitor → máquina → la entrada que dice su EDID.
    via: BTreeMap<MonitorHandle, BTreeMap<Machine, MonitorInput>>,
    ops: Vec<(Machine, BusOp)>,
}

/// Los monitores físicos del puesto.
#[derive(Debug, Clone)]
pub struct SimMonitors {
    fake: FakeBus,
    clock: FakeClock,
    handles: Vec<MonitorHandle>,
    ids: Vec<String>,
    shared: Arc<Mutex<Shared>>,
}

impl SimMonitors {
    pub fn new(specs: &[MonitorSpec]) -> Self {
        let clock = FakeClock::new();
        let fake = FakeBus::new(clock.clone());
        let mut shared = Shared::default();
        let (mut handles, mut ids) = (Vec::new(), Vec::new());
        for spec in specs {
            let h = fake.add(spec.config.clone());
            shared
                .cables
                .insert(h, spec.cables.iter().copied().collect());
            shared.via.insert(h, spec.via.iter().copied().collect());
            handles.push(h);
            ids.push(
                Edid::parse(&spec.config.edid)
                    .map(|e| e.monitor_id().to_string())
                    .unwrap_or_default(),
            );
        }
        SimMonitors {
            fake,
            clock,
            handles,
            ids,
            shared: Arc::new(Mutex::new(shared)),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// El bus de los monitores físicos (para mirar la verdad o tocar el menú del monitor).
    pub fn fake(&self) -> &FakeBus {
        &self.fake
    }

    pub fn clock(&self) -> FakeClock {
        self.clock.clone()
    }

    /// El handle del monitor `i` (en el orden de los [`MonitorSpec`]).
    pub fn handle(&self, i: usize) -> MonitorHandle {
        self.handles[i]
    }

    /// El id que Tandem le da al monitor `i`.
    pub fn id(&self, i: usize) -> &str {
        &self.ids[i]
    }

    /// Lo que ve `machine`.
    pub fn bus_of(&self, machine: Machine, world: World) -> MachineBus {
        MachineBus {
            machine,
            world,
            monitors: self.clone(),
        }
    }

    /// Cada pedido a `handle`, con la máquina que lo hizo.
    pub fn ops_for(&self, handle: MonitorHandle) -> Vec<(Machine, BusOp)> {
        self.lock()
            .ops
            .iter()
            .filter(|(_, op)| op.monitor() == handle)
            .cloned()
            .collect()
    }

    /// Las escrituras (`Set`) a `handle`, con quién las hizo.
    pub fn sets_to(&self, handle: MonitorHandle) -> Vec<(Machine, u8, u16)> {
        self.ops_for(handle)
            .into_iter()
            .filter_map(|(m, op)| match op {
                BusOp::Set { code, value, .. } => Some((m, code, value)),
                _ => None,
            })
            .collect()
    }

    pub fn clear_ops(&self) {
        self.lock().ops.clear();
    }

    /// Desconecta (o conecta, con `Some(latencia)`) el cable de `machine` al monitor `i`.
    pub fn set_cable(&self, i: usize, machine: Machine, latency: Option<Duration>) {
        let h = self.handles[i];
        let mut s = self.lock();
        let cables = s.cables.entry(h).or_default();
        match latency {
            Some(l) => cables.insert(machine, l),
            None => cables.remove(&machine),
        };
    }
}

/// Los monitores vistos desde una máquina.
#[derive(Debug, Clone)]
pub struct MachineBus {
    machine: Machine,
    world: World,
    monitors: SimMonitors,
}

impl MachineBus {
    /// La latencia de esta máquina hacia `h`, o `None` si no hay canal (apagada o sin cable).
    fn channel(&self, h: MonitorHandle) -> Option<Duration> {
        if self.world.power_of(self.machine) != Power::On {
            return None;
        }
        self.monitors
            .lock()
            .cables
            .get(&h)
            .and_then(|c| c.get(&self.machine))
            .copied()
    }

    fn record(&self, op: BusOp) {
        self.monitors.lock().ops.push((self.machine, op));
    }

    /// Un pedido por el cable: tarda su latencia y llega al monitor, o se pierde.
    fn wire<T>(
        &self,
        h: MonitorHandle,
        code: u8,
        op: BusOp,
        f: impl FnOnce(&FakeBus) -> Result<T, DdcError>,
    ) -> Result<T, DdcError> {
        let Some(latency) = self.channel(h) else {
            return Err(DdcError::NoReply { code });
        };
        self.record(op);
        self.monitors.clock.sleep(latency);
        f(&self.monitors.fake)
    }
}

impl DdcBus for MachineBus {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, DdcError> {
        if self.world.power_of(self.machine) != Power::On {
            return Err(DdcError::NoChannel("la máquina está apagada".into()));
        }
        let (cables, via) = {
            let s = self.monitors.lock();
            (s.cables.clone(), s.via.clone())
        };
        Ok(self
            .monitors
            .fake
            .monitors()?
            .into_iter()
            .filter(|m| {
                cables
                    .get(&m.handle)
                    .is_some_and(|c| c.contains_key(&self.machine))
            })
            .map(|mut m| {
                // El EDID de la entrada a la que va el cable de esta máquina.
                let input = via.get(&m.handle).and_then(|v| v.get(&self.machine));
                if let (Some(&input), Some(edid)) = (input, &m.edid) {
                    let mut raw = edid.raw().to_vec();
                    tandem_monitor::set_edid_input(&mut raw, Some(input));
                    m.edid = Edid::parse(&raw).ok();
                }
                m
            })
            .collect())
    }

    fn get_vcp(&self, monitor: MonitorHandle, code: u8) -> Result<VcpReply, DdcError> {
        self.wire(monitor, code, BusOp::Get { monitor, code }, |f| {
            f.get_vcp(monitor, code)
        })
    }

    fn set_vcp(&self, monitor: MonitorHandle, code: u8, value: u16) -> Result<(), DdcError> {
        let op = BusOp::Set {
            monitor,
            code,
            value,
        };
        self.wire(monitor, code, op, |f| f.set_vcp(monitor, code, value))
    }

    fn capabilities(&self, monitor: MonitorHandle) -> Result<String, DdcError> {
        self.wire(monitor, 0xF3, BusOp::Capabilities { monitor }, |f| {
            f.capabilities(monitor)
        })
    }
}

#[cfg(test)]
mod tests {
    use monitor_ddc::fake::Personality;

    use super::*;

    #[test]
    fn each_machine_sees_only_its_cables_while_it_is_on() {
        let world = World::new(); // A prendida, B apagada
        let sim = SimMonitors::new(&[MonitorSpec {
            config: FakeMonitorConfig::new(Personality::Standard),
            cables: vec![(Machine::A, Duration::from_millis(30))],
            via: vec![(Machine::A, MonitorInput::Hdmi)],
        }]);
        let a = sim.bus_of(Machine::A, world.clone());
        let b = sim.bus_of(Machine::B, world.clone());
        let seen = a.monitors().unwrap();
        assert_eq!(seen.len(), 1);
        let edid = seen[0].edid.as_ref().unwrap().raw();
        assert_eq!(tandem_monitor::edid_input(edid), Some(MonitorInput::Hdmi));
        assert!(b.monitors().is_err(), "B está apagada");
        let before = sim.clock().now();
        a.get_vcp(sim.handle(0), 0xEA).unwrap();
        assert_eq!(sim.clock().now() - before, Duration::from_millis(30));
        world.lock().boot_done(Machine::B);
        assert!(b.monitors().unwrap().is_empty(), "sin cable");
        assert!(b.get_vcp(sim.handle(0), 0xEA).is_err());
        assert_eq!(sim.ops_for(sim.handle(0)).len(), 1);
        assert!(sim.id(0).starts_with("lc34g55t-"));
    }
}
