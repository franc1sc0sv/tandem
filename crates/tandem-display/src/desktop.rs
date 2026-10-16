//! El escritorio de una máquina: agregar o quitar un monitor (el fantasma, DESIGN §8)
//! y el modo de la pantalla integrada de la MacBook (§8.1).
//!
//! Lo implementan los backends de topología de cada sistema: en Windows,
//! [`TopologyDesktop`] sobre `WindowsTopology` (`SetDisplayConfig`); en macOS, el de
//! T-012. Las llamadas pueden bloquear: quien las use desde código asíncrono las corre
//! fuera del runtime.

use std::sync::{Mutex, PoisonError};

use tandem_core::{BuiltinDisplay, MonitorConfig};

use crate::topology::{self, DisplayTopology};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DesktopError(pub String);

pub trait DesktopControl: Send + Sync {
    /// Agrega el monitor al escritorio de esta máquina.
    fn attach(&self, monitor: &MonitorConfig) -> Result<(), DesktopError>;
    /// Lo quita, para que ni las ventanas ni el cursor vayan a una pantalla que no se ve.
    fn detach(&self, monitor: &MonitorConfig) -> Result<(), DesktopError>;
    /// Solo tiene sentido en la Mac.
    fn set_builtin(&self, mode: BuiltinDisplay) -> Result<(), DesktopError>;
}

/// Escritorio sobre una [`DisplayTopology`], buscando el monitor por su `edid_model`
/// (el mismo texto que usa DDC).
#[derive(Debug)]
pub struct TopologyDesktop<T> {
    topo: Mutex<T>,
}

impl<T: DisplayTopology + Send> TopologyDesktop<T> {
    pub fn new(topo: T) -> Self {
        TopologyDesktop {
            topo: Mutex::new(topo),
        }
    }

    pub fn into_inner(self) -> T {
        self.topo
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn set(&self, monitor: &MonitorConfig, active: bool) -> Result<(), DesktopError> {
        let query = monitor.edid_model.as_deref().ok_or_else(|| {
            DesktopError(format!(
                "{}: falta `edid_model` en la configuración para encontrarlo",
                monitor.id
            ))
        })?;
        let mut topo = self.topo.lock().unwrap_or_else(PoisonError::into_inner);
        let r = if active {
            topology::attach(&mut *topo, query)
        } else {
            topology::detach(&mut *topo, query)
        };
        r.map(|_| ())
            .map_err(|e| DesktopError(format!("{}: {e}", monitor.id)))
    }
}

impl<T: DisplayTopology + Send> DesktopControl for TopologyDesktop<T> {
    fn attach(&self, monitor: &MonitorConfig) -> Result<(), DesktopError> {
        self.set(monitor, true)
    }

    fn detach(&self, monitor: &MonitorConfig) -> Result<(), DesktopError> {
        self.set(monitor, false)
    }

    fn set_builtin(&self, _mode: BuiltinDisplay) -> Result<(), DesktopError> {
        Err(DesktopError(
            "esta máquina no tiene pantalla integrada".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use tandem_core::Machine;

    use super::*;
    use crate::switcher::tests::request;
    use crate::topology::fake::FakeTopology;

    fn active(d: &TopologyDesktop<FakeTopology>) -> Vec<String> {
        let mut topo = d.topo.lock().unwrap();
        topo.displays()
            .unwrap()
            .into_iter()
            .filter(|x| x.active)
            .map(|x| x.model().to_owned())
            .collect()
    }

    #[test]
    fn detaches_and_attaches_by_edid_model() {
        let d = TopologyDesktop::new(FakeTopology::two_monitors());
        let m34 = request("m34", Machine::Pc).monitor;
        d.detach(&m34).unwrap();
        assert_eq!(active(&d), ["S32BG85"]);
        // Idempotente, como la topología.
        d.detach(&m34).unwrap();
        d.attach(&m34).unwrap();
        assert_eq!(active(&d), ["LC34G55T", "S32BG85"]);
    }

    #[test]
    fn errors_name_the_monitor() {
        let d = TopologyDesktop::new(FakeTopology::two_monitors());
        // El 32" del puesto de prueba no tiene `edid_model`.
        let m32 = request("m32", Machine::Pc).monitor;
        let e = d.detach(&m32).unwrap_err();
        assert!(e.0.starts_with("m32: falta `edid_model`"), "{e}");

        let mut m34 = request("m34", Machine::Pc).monitor;
        m34.edid_model = Some("NOEXISTE".into());
        let e = d.attach(&m34).unwrap_err();
        assert!(e.0.starts_with("m34: "), "{e}");

        let mut fake = d.into_inner();
        fake.fail_next = true;
        let d = TopologyDesktop::new(fake);
        assert!(d.detach(&request("m34", Machine::Pc).monitor).is_err());
        assert_eq!(active(&d), ["LC34G55T", "S32BG85"]);

        assert!(d.set_builtin(BuiltinDisplay::Extend).is_err());
    }
}
