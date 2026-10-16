//! Topología de pantallas de macOS (Quartz Display Services), en código portable.
//!
//! [`QuartzApi`] son las primitivas del sistema: listar, activar/desactivar y espejo.
//! La implementación real vive en `crate::macos::quartz`; aquí está la lógica que decide
//! qué llamar ([`MacTopology`]) y que se prueba en Linux con [`fake::FakeQuartz`].
//!
//! Reglas que impone [`MacTopology`] (DESIGN §8 y §8.1):
//! - Nunca deja a la Mac sin ninguna pantalla activa.
//! - Recuerda las pantallas que desactivó: macOS puede dejar de listarlas y sin su id no
//!   habría forma de devolverlas.
//! - Si la API privada para apagar la pantalla integrada no está, cae al espejo.

pub mod fake;

use std::collections::BTreeMap;
use std::fmt;

use tandem_core::{BuiltinDisplay, MonitorConfig};

/// `CGDirectDisplayID`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisplayId(pub u32);

impl fmt::Display for DisplayId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Rectángulo en puntos del escritorio global (origen arriba a la izquierda de la principal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Una pantalla tal como la ve macOS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Display {
    pub id: DisplayId,
    /// Nombre del producto según el EDID (por ejemplo `LC34G55T`), si el sistema lo da.
    pub name: Option<String>,
    pub vendor: u32,
    pub model: u32,
    pub serial: u32,
    pub builtin: bool,
    pub main: bool,
    /// Forma parte del escritorio. `false` = desactivada (fantasma quitado).
    pub enabled: bool,
    /// Pantalla de la que es espejo, si lo es.
    pub mirror_of: Option<DisplayId>,
    pub bounds: Bounds,
}

impl Display {
    /// Nombre para mensajes: el del EDID o, si falta, fabricante y modelo en hexadecimal.
    pub fn label(&self) -> String {
        match &self.name {
            Some(n) => n.clone(),
            None if self.builtin => "pantalla integrada".into(),
            None => format!("{:04x}:{:04x}", self.vendor, self.model),
        }
    }

    /// ¿Es el monitor configurado? Se compara el modelo del EDID sin distinguir mayúsculas.
    pub fn matches(&self, monitor: &MonitorConfig) -> bool {
        if self.builtin {
            return false;
        }
        match (&self.name, &monitor.edid_model) {
            (Some(name), Some(model)) => name.trim().eq_ignore_ascii_case(model.trim()),
            _ => false,
        }
    }
}

impl fmt::Display for Display {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.id, self.label())?;
        if self.builtin {
            write!(f, " (integrada)")?;
        }
        if self.main {
            write!(f, " (principal)")?;
        }
        if !self.enabled {
            write!(f, " [desactivada]")?;
        }
        if let Some(m) = self.mirror_of {
            write!(f, " [espejo de {m}]")?;
        }
        let b = self.bounds;
        write!(f, " {}x{} en ({}, {})", b.width, b.height, b.x, b.y)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TopologyError {
    #[error("no se encontró el monitor `{0}` entre las pantallas de la Mac")]
    NotFound(String),
    #[error("hay varias pantallas que coinciden con `{0}`")]
    Ambiguous(String),
    #[error("la Mac no tiene pantalla integrada")]
    NoBuiltin,
    #[error("no hay un monitor externo activo del que hacer espejo")]
    NoMirrorTarget,
    #[error("{0} es la última pantalla activa: la Mac se quedaría sin imagen")]
    LastDisplay(DisplayId),
    /// `CGSConfigureDisplayEnabled` no existe en esta versión de macOS.
    #[error("la API privada para desactivar pantallas no está disponible")]
    PrivateApiUnavailable,
    #[error("{op} falló (CGError {code})")]
    Os { op: &'static str, code: i32 },
    #[error("la topología de macOS no está disponible en esta plataforma")]
    Unsupported,
}

/// Primitivas de Quartz Display Services. Cada llamada es una configuración completa.
pub trait QuartzApi {
    /// Pantallas conectadas (`CGGetOnlineDisplayList`). Puede omitir las desactivadas.
    fn online(&mut self) -> Result<Vec<Display>, TopologyError>;
    /// `CGSConfigureDisplayEnabled` (privada).
    fn set_enabled(&mut self, id: DisplayId, enabled: bool) -> Result<(), TopologyError>;
    /// `CGConfigureDisplayMirrorOfDisplay`; `None` deja de hacer espejo.
    fn set_mirror(&mut self, id: DisplayId, master: Option<DisplayId>)
    -> Result<(), TopologyError>;
}

/// Cómo quedó la pantalla integrada tras [`MacTopology::set_builtin`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinApplied {
    /// Se aplicó el modo pedido.
    Exact(BuiltinDisplay),
    /// Se pidió `Off`, la API privada no está y quedó en espejo (DESIGN §8.1).
    MirrorFallback,
}

/// Topología de la Mac sobre una [`QuartzApi`].
pub struct MacTopology<A> {
    api: A,
    /// Pantallas que desactivamos nosotros, con su última foto conocida.
    disabled: BTreeMap<DisplayId, Display>,
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

    /// Todas las pantallas: las que lista macOS más las que desactivamos y ya no lista.
    pub fn displays(&mut self) -> Result<Vec<Display>, TopologyError> {
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

    /// El monitor externo de la configuración, identificado por el modelo del EDID.
    pub fn find(&mut self, monitor: &MonitorConfig) -> Result<Display, TopologyError> {
        let mut hits = self.displays()?.into_iter().filter(|d| d.matches(monitor));
        let first = hits
            .next()
            .ok_or_else(|| TopologyError::NotFound(monitor.id.to_string()))?;
        if hits.next().is_some() {
            return Err(TopologyError::Ambiguous(monitor.id.to_string()));
        }
        Ok(first)
    }

    /// La pantalla integrada de la MacBook.
    pub fn builtin(&mut self) -> Result<Display, TopologyError> {
        self.displays()?
            .into_iter()
            .find(|d| d.builtin)
            .ok_or(TopologyError::NoBuiltin)
    }

    /// Quita el monitor del escritorio de macOS (el "fantasma"). Idempotente.
    pub fn detach(&mut self, monitor: &MonitorConfig) -> Result<(), TopologyError> {
        let target = self.find(monitor)?;
        self.disable(target)
    }

    /// Devuelve el monitor al escritorio de macOS. Idempotente.
    pub fn attach(&mut self, monitor: &MonitorConfig) -> Result<(), TopologyError> {
        let target = self.find(monitor)?;
        self.enable(&target)
    }

    /// Aplica un modo a la pantalla integrada.
    pub fn set_builtin(&mut self, mode: BuiltinDisplay) -> Result<BuiltinApplied, TopologyError> {
        let builtin = self.builtin()?;
        match mode {
            BuiltinDisplay::Extend => {
                self.enable(&builtin)?;
                self.unmirror(builtin.id)?;
            }
            BuiltinDisplay::Mirror => self.mirror_builtin(&builtin)?,
            BuiltinDisplay::Off => match self.disable(builtin.clone()) {
                Ok(()) => {}
                Err(TopologyError::PrivateApiUnavailable) => {
                    self.mirror_builtin(&builtin)?;
                    return Ok(BuiltinApplied::MirrorFallback);
                }
                Err(e) => return Err(e),
            },
        }
        Ok(BuiltinApplied::Exact(mode))
    }

    /// Vuelve a activar todo lo que desactivamos. Para la vuelta automática y al salir.
    ///
    /// Intenta con todas aunque alguna falle y devuelve el primer error.
    pub fn restore_all(&mut self) -> Result<(), TopologyError> {
        let pending: Vec<Display> = self.disabled.values().cloned().collect();
        let mut first_err = None;
        for d in pending {
            if let Err(e) = self.enable(&d) {
                first_err.get_or_insert(e);
            }
        }
        first_err.map_or(Ok(()), Err)
    }

    /// Ids que desactivamos y siguen pendientes de devolver.
    pub fn disabled_ids(&self) -> Vec<DisplayId> {
        self.disabled.keys().copied().collect()
    }

    fn enable(&mut self, d: &Display) -> Result<(), TopologyError> {
        let current = self.displays()?.into_iter().find(|x| x.id == d.id);
        if current.as_ref().is_some_and(|c| c.enabled) {
            self.disabled.remove(&d.id);
            return Ok(());
        }
        self.api.set_enabled(d.id, true)?;
        self.disabled.remove(&d.id);
        Ok(())
    }

    fn disable(&mut self, target: Display) -> Result<(), TopologyError> {
        let all = self.displays()?;
        let current = all.iter().find(|d| d.id == target.id).unwrap_or(&target);
        if !current.enabled {
            return Ok(());
        }
        if !all.iter().any(|d| d.enabled && d.id != target.id) {
            return Err(TopologyError::LastDisplay(target.id));
        }
        // Lo que refleja a esta pantalla se quedaría sin imagen: pasa a extendida.
        for d in all.iter().filter(|d| d.mirror_of == Some(target.id)) {
            self.api.set_mirror(d.id, None)?;
        }
        // Una pantalla desactivada no puede seguir en un grupo de espejo.
        if current.mirror_of.is_some() {
            self.api.set_mirror(target.id, None)?;
        }
        let snapshot = current.clone();
        self.api.set_enabled(target.id, false)?;
        self.disabled.insert(target.id, snapshot);
        Ok(())
    }

    fn unmirror(&mut self, id: DisplayId) -> Result<(), TopologyError> {
        let mirroring = self
            .displays()?
            .iter()
            .any(|d| d.id == id && d.mirror_of.is_some());
        if mirroring {
            self.api.set_mirror(id, None)?;
        }
        Ok(())
    }

    /// Espejo del monitor externo principal (o del primero activo si la principal es la
    /// integrada).
    fn mirror_builtin(&mut self, builtin: &Display) -> Result<(), TopologyError> {
        self.enable(builtin)?;
        let all = self.displays()?;
        let externals = || {
            all.iter()
                .filter(|d| !d.builtin && d.enabled && d.mirror_of.is_none())
        };
        let master = externals()
            .find(|d| d.main)
            .or_else(|| externals().next())
            .ok_or(TopologyError::NoMirrorTarget)?
            .id;
        let current = all.iter().find(|d| d.id == builtin.id);
        if current.and_then(|c| c.mirror_of) == Some(master) {
            return Ok(());
        }
        self.api.set_mirror(builtin.id, Some(master))
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
