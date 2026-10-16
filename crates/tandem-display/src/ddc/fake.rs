//! Doble de prueba de [`DdcBus`]: monitores en memoria con fallos programables.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::{DdcBus, DdcError, MonitorInfo, VcpValue};

#[derive(Debug, Clone, Default)]
pub struct FakeMonitor {
    pub info: Option<MonitorInfo>,
    pub capabilities: String,
    pub vcp: BTreeMap<u8, VcpValue>,
}

/// Escritura registrada: (monitor, código, valor, ¿la aceptó?).
pub type Write = (usize, u8, u32, bool);

#[derive(Debug, Default)]
pub struct FakeDdc {
    infos: Vec<MonitorInfo>,
    monitors: Vec<FakeMonitor>,
    /// Resultado de las próximas escrituras, en orden; `false` = falla. Vacío = todo bien.
    pub write_script: VecDeque<bool>,
    pub writes: Vec<Write>,
}

/// Error que devuelve el doble cuando el guion pide fallar. Imita el `0xC0262589` real.
pub fn flaky_error(op: &'static str) -> DdcError {
    DdcError::Os {
        op,
        code: 0xC026_2589,
        message: "checksum DDC/CI inválido (simulado)".into(),
    }
}

impl FakeDdc {
    pub fn new(monitors: Vec<FakeMonitor>) -> Self {
        let infos = monitors
            .iter()
            .enumerate()
            .map(|(index, m)| {
                let mut info = m.info.clone().unwrap_or_else(|| MonitorInfo {
                    index,
                    description: format!("Monitor simulado {index}"),
                    device: format!(r"\\.\DISPLAY{}", index + 1),
                    edid: None,
                });
                info.index = index;
                info
            })
            .collect();
        Self {
            infos,
            monitors,
            ..Self::default()
        }
    }

    pub fn value(&self, monitor: usize, code: u8) -> Option<u32> {
        self.monitors
            .get(monitor)
            .and_then(|m| m.vcp.get(&code))
            .map(|v| v.current)
    }

    fn monitor(&mut self, index: usize) -> Result<&mut FakeMonitor, DdcError> {
        self.monitors
            .get_mut(index)
            .ok_or(DdcError::NoSuchMonitor(index))
    }
}

impl DdcBus for FakeDdc {
    fn monitors(&self) -> &[MonitorInfo] {
        &self.infos
    }

    fn capabilities(&mut self, monitor: usize) -> Result<String, DdcError> {
        Ok(self.monitor(monitor)?.capabilities.clone())
    }

    fn get_vcp(&mut self, monitor: usize, code: u8) -> Result<VcpValue, DdcError> {
        self.monitor(monitor)?
            .vcp
            .get(&code)
            .copied()
            .ok_or_else(|| flaky_error("GetVCPFeatureAndVCPFeatureReply"))
    }

    fn set_vcp(&mut self, monitor: usize, code: u8, value: u32) -> Result<(), DdcError> {
        self.monitor(monitor)?;
        let ok = self.write_script.pop_front().unwrap_or(true);
        self.writes.push((monitor, code, value, ok));
        if !ok {
            return Err(flaky_error("SetVCPFeature"));
        }
        let entry = self.monitor(monitor)?.vcp.entry(code).or_insert(VcpValue {
            current: 0,
            maximum: 0xFFFF,
        });
        entry.current = value;
        Ok(())
    }
}

/// Un [`FakeDdc`] compartido: cada [`SharedFakeDdc::open`] da un bus nuevo sobre los
/// mismos monitores, como abre el bus el backend real en cada cambio. Sirve para ver
/// desde la prueba lo que se escribió.
#[derive(Debug, Clone)]
pub struct SharedFakeDdc {
    infos: Vec<MonitorInfo>,
    inner: Arc<Mutex<FakeDdc>>,
}

impl SharedFakeDdc {
    pub fn new(fake: FakeDdc) -> Self {
        Self {
            infos: fake.monitors().to_vec(),
            inner: Arc::new(Mutex::new(fake)),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, FakeDdc> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn writes(&self) -> Vec<Write> {
        self.lock().writes.clone()
    }

    /// Una fábrica de buses para `DdcSwitcher`.
    pub fn factory(&self) -> crate::switcher::BusFactory {
        let me = self.clone();
        Arc::new(move || Ok(Box::new(me.clone()) as Box<dyn DdcBus>))
    }
}

impl DdcBus for SharedFakeDdc {
    fn monitors(&self) -> &[MonitorInfo] {
        &self.infos
    }

    fn capabilities(&mut self, monitor: usize) -> Result<String, DdcError> {
        self.lock().capabilities(monitor)
    }

    fn get_vcp(&mut self, monitor: usize, code: u8) -> Result<VcpValue, DdcError> {
        self.lock().get_vcp(monitor, code)
    }

    fn set_vcp(&mut self, monitor: usize, code: u8, value: u32) -> Result<(), DdcError> {
        self.lock().set_vcp(monitor, code, value)
    }
}
