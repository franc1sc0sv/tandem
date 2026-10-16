//! Doble de prueba de [`QuartzApi`]: pantallas en memoria con el comportamiento de macOS
//! que importa a la lógica (desactivadas que desaparecen de la lista, espejo, API privada
//! ausente, fallos programados).

use std::collections::VecDeque;

use super::{Bounds, CgDisplay, DisplayId, QuartzApi};
use crate::topology::TopologyError;

/// Llamada registrada, para comprobar el orden en las pruebas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    SetEnabled(DisplayId, bool),
    SetMirror(DisplayId, Option<DisplayId>),
}

#[derive(Debug, Clone)]
pub struct FakeQuartz {
    pub displays: Vec<CgDisplay>,
    /// Si es `true`, `online` omite las desactivadas (lo que se espera de macOS real).
    pub hide_disabled: bool,
    /// `false` simula un macOS sin `CGSConfigureDisplayEnabled`.
    pub private_api: bool,
    /// Resultado de las próximas llamadas que cambian algo; `false` = falla. Vacío = todo bien.
    pub script: VecDeque<bool>,
    pub calls: Vec<Call>,
}

/// La pantalla integrada de una MacBook (id 1, principal).
pub fn builtin() -> CgDisplay {
    CgDisplay {
        id: DisplayId(1),
        name: Some("Color LCD".into()),
        vendor: 0x0610,
        model: 0xA050,
        serial: 0,
        builtin: true,
        main: true,
        enabled: true,
        mirror_of: None,
        bounds: Bounds {
            x: 0,
            y: 0,
            width: 1512,
            height: 982,
        },
    }
}

/// Un monitor externo activo con el nombre de EDID dado.
pub fn external(id: u32, name: &str) -> CgDisplay {
    CgDisplay {
        id: DisplayId(id),
        name: Some(name.into()),
        vendor: 0x4C2D,
        model: 0x7000 + id,
        serial: id,
        builtin: false,
        main: false,
        enabled: true,
        mirror_of: None,
        bounds: Bounds {
            x: 1512 * id as i32,
            y: 0,
            width: 3440,
            height: 1440,
        },
    }
}

/// El error que daría CoreGraphics (1000 = `kCGErrorFailure`, 1001 = `kCGErrorIllegalArgument`).
fn os(op: &'static str, code: i32) -> TopologyError {
    TopologyError::Os {
        op,
        message: format!("CGError {code}"),
    }
}

impl FakeQuartz {
    pub fn new(displays: Vec<CgDisplay>) -> Self {
        Self {
            displays,
            hide_disabled: true,
            private_api: true,
            script: VecDeque::new(),
            calls: Vec::new(),
        }
    }

    /// La MacBook con el 34" y el 32" conectados por HDMI.
    pub fn desk() -> Self {
        Self::new(vec![
            builtin(),
            external(2, "LC34G55T"),
            external(3, "Odyssey G5"),
        ])
    }

    pub fn get(&self, id: DisplayId) -> Option<&CgDisplay> {
        self.displays.iter().find(|d| d.id == id)
    }

    pub fn enabled_count(&self) -> usize {
        self.displays.iter().filter(|d| d.enabled).count()
    }

    fn step(&mut self, op: &'static str, call: Call) -> Result<(), TopologyError> {
        self.calls.push(call);
        if self.script.pop_front().unwrap_or(true) {
            Ok(())
        } else {
            Err(os(op, 1000))
        }
    }

    fn display_mut(&mut self, id: DisplayId) -> Result<&mut CgDisplay, TopologyError> {
        self.displays
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| os("CGBeginDisplayConfiguration", 1001))
    }

    /// macOS mueve la principal si la desactivada lo era.
    fn fix_main(&mut self) {
        if self.displays.iter().any(|d| d.main && d.enabled) {
            return;
        }
        for d in &mut self.displays {
            d.main = false;
        }
        if let Some(d) = self.displays.iter_mut().find(|d| d.enabled) {
            d.main = true;
        }
    }
}

impl QuartzApi for FakeQuartz {
    fn online(&mut self) -> Result<Vec<CgDisplay>, TopologyError> {
        Ok(self
            .displays
            .iter()
            .filter(|d| d.enabled || !self.hide_disabled)
            .cloned()
            .collect())
    }

    fn set_enabled(&mut self, id: DisplayId, enabled: bool) -> Result<(), TopologyError> {
        if !self.private_api {
            self.calls.push(Call::SetEnabled(id, enabled));
            return Err(TopologyError::PrivateApiUnavailable);
        }
        self.step("CGSConfigureDisplayEnabled", Call::SetEnabled(id, enabled))?;
        let d = self.display_mut(id)?;
        d.enabled = enabled;
        if !enabled {
            d.main = false;
        }
        self.fix_main();
        Ok(())
    }

    fn set_mirror(
        &mut self,
        id: DisplayId,
        master: Option<DisplayId>,
    ) -> Result<(), TopologyError> {
        self.step(
            "CGConfigureDisplayMirrorOfDisplay",
            Call::SetMirror(id, master),
        )?;
        if let Some(m) = master
            && !self.get(m).is_some_and(|d| d.enabled)
        {
            return Err(os("CGConfigureDisplayMirrorOfDisplay", 1001));
        }
        self.display_mut(id)?.mirror_of = master;
        Ok(())
    }
}
