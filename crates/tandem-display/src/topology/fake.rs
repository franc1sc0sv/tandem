//! Doble de prueba de [`DisplayTopology`]: rutas en memoria con el mismo modelo que
//! Windows, usando el mismo planificador que el backend real.

use std::collections::BTreeMap;

use super::plan::{self, PathSlot};
use super::{Display, DisplayTopology, TargetId, TopologyError};

#[derive(Debug, Default)]
pub struct FakeTopology {
    pub paths: Vec<PathSlot>,
    /// Identidad de cada target conectado (el `active` se ignora: sale de las rutas).
    pub monitors: BTreeMap<TargetId, Display>,
    /// Si es `true`, el próximo cambio falla como lo haría `SetDisplayConfig`.
    pub fail_next: bool,
    /// Cambios aplicados con éxito.
    pub applied: usize,
}

impl FakeTopology {
    /// Un adaptador con dos fuentes y dos monitores activos: el 34" en el target 1 y el
    /// 32" en el target 2, como la PC del diseño.
    pub fn two_monitors() -> Self {
        let mut t = Self::default();
        for (target, name, serial, uid) in [
            (1, "LC34G55T", "H4ZR900123", "UID4352"),
            (2, "S32BG85", "HNTW500456", "UID4353"),
        ] {
            for source in 0..2 {
                t.paths.push(PathSlot {
                    adapter: 1,
                    source,
                    target,
                    active: source + 1 == target,
                    available: true,
                });
            }
            let id = TargetId { adapter: 1, target };
            t.monitors.insert(
                id,
                Display {
                    target: id,
                    name: name.into(),
                    device_path: format!(r"\\?\DISPLAY#SAM7140#5&2a1b3c&0&{uid}#{{e6f07b5f}}"),
                    edid: Some(sample_edid(name, serial)),
                    active: false,
                },
            );
        }
        t
    }
}

fn sample_edid(name: &str, serial: &str) -> crate::ddc::Edid {
    crate::ddc::Edid {
        manufacturer: "SAM".into(),
        product_code: 0x7140,
        serial_number: 0,
        name: Some(name.into()),
        serial_text: Some(serial.into()),
    }
}

impl DisplayTopology for FakeTopology {
    fn displays(&mut self) -> Result<Vec<Display>, TopologyError> {
        Ok(plan::targets(&self.paths)
            .into_iter()
            .map(|(target, active)| {
                let mut d = self.monitors.get(&target).cloned().unwrap_or(Display {
                    target,
                    name: format!("Monitor simulado {target}"),
                    device_path: String::new(),
                    edid: None,
                    active,
                });
                d.active = active;
                d
            })
            .collect())
    }

    fn set_active(&mut self, target: TargetId, active: bool) -> Result<(), TopologyError> {
        let (on, off) = if active {
            (plan::plan_attach(&self.paths, target)?, Vec::new())
        } else {
            (None, plan::plan_detach(&self.paths, target)?)
        };
        if on.is_none() && off.is_empty() {
            return Ok(());
        }
        if std::mem::take(&mut self.fail_next) {
            return Err(TopologyError::Os {
                op: "SetDisplayConfig",
                code: 87,
                message: "parámetro incorrecto (simulado)".into(),
            });
        }
        if let Some(i) = on {
            self.paths[i].active = true;
        }
        for i in off {
            self.paths[i].active = false;
        }
        self.applied += 1;
        Ok(())
    }
}
