//! Topología de pantallas de macOS (Quartz Display Services), en código portable.
//!
//! [`QuartzApi`] son las primitivas del sistema: listar, activar/desactivar y espejo.
//! La implementación real vive en `crate::macos::quartz`; aquí está [`MacTopology`], el
//! backend de [`DisplayTopology`] para la Mac, que se prueba en Linux con
//! [`fake::FakeQuartz`].
//!
//! Reglas que impone [`MacTopology`] (DESIGN §8 y §8.1):
//! - Nunca deja a la Mac sin ninguna pantalla activa.
//! - Recuerda las pantallas que desactivó: macOS puede dejar de listarlas y sin su id no
//!   habría forma de devolverlas.
//! - Antes de desactivar una pantalla deshace los espejos que la involucran.
//!
//! El modo de la integrada (y su caída al espejo sin la API privada) es lógica común:
//! [`crate::topology::set_builtin`].

pub mod fake;

use std::collections::BTreeMap;
use std::fmt;

use crate::ddc::{Edid, edid};
use crate::topology::{Display, DisplayTopology, TopologyError};

pub use crate::topology::Bounds;

/// `CGDirectDisplayID`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisplayId(pub u32);

impl fmt::Display for DisplayId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Una pantalla tal como la describe CoreGraphics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgDisplay {
    pub id: DisplayId,
    /// Nombre del producto según el EDID (por ejemplo `LC34G55T`), si el sistema lo da.
    pub name: Option<String>,
    /// Palabra de fabricante del EDID (`0x4C2D` = `SAM`).
    pub vendor: u32,
    /// Código de producto del EDID.
    pub model: u32,
    pub serial: u32,
    /// Tamaño físico en mm (`CGDisplayScreenSize`); `None` si el sistema no lo sabe.
    pub size_mm: Option<(u32, u32)>,
    pub builtin: bool,
    pub main: bool,
    /// Forma parte del escritorio. `false` = desactivada (fantasma quitado).
    pub enabled: bool,
    /// Pantalla de la que es espejo, si lo es.
    pub mirror_of: Option<DisplayId>,
    pub bounds: Bounds,
}

impl From<CgDisplay> for Display<DisplayId> {
    fn from(d: CgDisplay) -> Self {
        // CoreGraphics da los campos del EDID ya separados; sin fabricante no hay EDID.
        let edid = (d.vendor != 0).then(|| Edid {
            manufacturer: edid::manufacturer(d.vendor as u16),
            product_code: d.model as u16,
            serial_number: d.serial,
            name: d.name.clone(),
            serial_text: None,
            size_cm: d.size_mm.and_then(|(w, h)| {
                let cm = |mm: u32| u8::try_from((mm + 5) / 10).ok().filter(|c| *c > 0);
                Some((cm(w)?, cm(h)?))
            }),
        });
        let name = match d.name {
            Some(n) => n,
            None if d.builtin => "pantalla integrada".into(),
            None => format!("{:04x}:{:04x}", d.vendor, d.model),
        };
        Display {
            id: d.id,
            name,
            device_path: String::new(),
            edid,
            builtin: d.builtin,
            main: d.main,
            active: d.enabled,
            mirror_of: d.mirror_of,
            // CoreGraphics no dice por qué cable sale.
            output: None,
            // Una desactivada conserva el último rectángulo; ya no está en el escritorio.
            bounds: d.enabled.then_some(d.bounds),
        }
    }
}

/// Primitivas de Quartz Display Services. Cada llamada es una configuración completa.
pub trait QuartzApi {
    /// Pantallas conectadas (`CGGetOnlineDisplayList`). Puede omitir las desactivadas.
    fn online(&mut self) -> Result<Vec<CgDisplay>, TopologyError>;
    /// `CGSConfigureDisplayEnabled` (privada).
    fn set_enabled(&mut self, id: DisplayId, enabled: bool) -> Result<(), TopologyError>;
    /// `CGConfigureDisplayMirrorOfDisplay`; `None` deja de hacer espejo.
    fn set_mirror(&mut self, id: DisplayId, master: Option<DisplayId>)
    -> Result<(), TopologyError>;
}

/// Topología de la Mac sobre una [`QuartzApi`].
pub struct MacTopology<A> {
    api: A,
    /// Pantallas que desactivamos nosotros, con su última foto conocida.
    disabled: BTreeMap<DisplayId, CgDisplay>,
}

impl<A: QuartzApi> MacTopology<A> {
    pub fn new(api: A) -> Self {
        Self {
            api,
            disabled: BTreeMap::new(),
        }
    }

    pub fn api(&self) -> &A {
        &self.api
    }

    pub fn api_mut(&mut self) -> &mut A {
        &mut self.api
    }

    /// Vuelve a activar todo lo que desactivamos. Para la vuelta automática y al salir.
    ///
    /// Intenta con todas aunque alguna falle y devuelve el primer error.
    pub fn restore_all(&mut self) -> Result<(), TopologyError> {
        let pending = self.disabled_ids();
        let mut first_err = None;
        for id in pending {
            if let Err(e) = self.enable(id) {
                first_err.get_or_insert(e);
            }
        }
        first_err.map_or(Ok(()), Err)
    }

    /// Ids que desactivamos y siguen pendientes de devolver.
    pub fn disabled_ids(&self) -> Vec<DisplayId> {
        self.disabled.keys().copied().collect()
    }

    /// Lo que lista macOS más las que desactivamos y ya no lista.
    fn snapshot(&mut self) -> Result<Vec<CgDisplay>, TopologyError> {
        let mut list = self.api.online()?;
        for (id, snapshot) in &self.disabled {
            if !list.iter().any(|d| d.id == *id) {
                let mut d = snapshot.clone();
                d.enabled = false;
                d.main = false;
                d.mirror_of = None;
                list.push(d);
            }
        }
        list.sort_by_key(|d| d.id);
        Ok(list)
    }

    fn enable(&mut self, id: DisplayId) -> Result<(), TopologyError> {
        let current = self.snapshot()?.into_iter().find(|x| x.id == id);
        if current.is_some_and(|c| c.enabled) {
            self.disabled.remove(&id);
            return Ok(());
        }
        self.api.set_enabled(id, true)?;
        self.disabled.remove(&id);
        Ok(())
    }

    fn disable(&mut self, id: DisplayId) -> Result<(), TopologyError> {
        let all = self.snapshot()?;
        let current = all
            .iter()
            .find(|d| d.id == id)
            .ok_or_else(|| TopologyError::NotConnected(id.to_string()))?;
        if !current.enabled {
            return Ok(());
        }
        if !all.iter().any(|d| d.enabled && d.id != id) {
            return Err(TopologyError::LastDisplay(id.to_string()));
        }
        // Lo que refleja a esta pantalla se quedaría sin imagen: pasa a extendida.
        for d in all.iter().filter(|d| d.mirror_of == Some(id)) {
            self.api.set_mirror(d.id, None)?;
        }
        // Una pantalla desactivada no puede seguir en un grupo de espejo.
        if current.mirror_of.is_some() {
            self.api.set_mirror(id, None)?;
        }
        let snapshot = current.clone();
        self.api.set_enabled(id, false)?;
        self.disabled.insert(id, snapshot);
        Ok(())
    }
}

impl<A: QuartzApi> DisplayTopology for MacTopology<A> {
    type Id = DisplayId;

    fn displays(&mut self) -> Result<Vec<Display<DisplayId>>, TopologyError> {
        Ok(self.snapshot()?.into_iter().map(Display::from).collect())
    }

    fn set_active(&mut self, id: DisplayId, active: bool) -> Result<(), TopologyError> {
        if active {
            self.enable(id)
        } else {
            self.disable(id)
        }
    }

    fn set_mirror(
        &mut self,
        id: DisplayId,
        master: Option<DisplayId>,
    ) -> Result<(), TopologyError> {
        self.api.set_mirror(id, master)
    }
}

/// Llama a [`MacTopology::restore_all`] al soltarse, también si hay pánico. Pensado para
/// las pruebas en hardware: el usuario no puede quedarse sin pantalla.
pub struct RestoreGuard<'a, A: QuartzApi>(pub &'a mut MacTopology<A>);

impl<A: QuartzApi> Drop for RestoreGuard<'_, A> {
    fn drop(&mut self) {
        let _ = self.0.restore_all();
    }
}

impl<A: QuartzApi> std::ops::Deref for RestoreGuard<'_, A> {
    type Target = MacTopology<A>;
    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<A: QuartzApi> std::ops::DerefMut for RestoreGuard<'_, A> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0
    }
}

#[cfg(test)]
mod tests;
