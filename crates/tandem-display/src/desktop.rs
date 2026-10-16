//! El escritorio de una máquina: agregar o quitar un monitor (el fantasma, DESIGN §8)
//! y el modo de la pantalla integrada de la MacBook (§8.1).
//!
//! [`TopologyDesktop`] lo implementa sobre cualquier [`DisplayTopology`], sin código de
//! plataforma: la elección del backend real de cada sistema está solo en [`native`]. Las
//! llamadas pueden bloquear: quien las use desde código asíncrono las corre fuera del
//! runtime.

use std::sync::{Arc, Mutex, PoisonError};

use tandem_core::{BuiltinDisplay, Machine, MonitorConfig};

use crate::topology::{self, Bounds, DisplayTopology, EdidKey, PlacedScreen};

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
    /// Las pantallas del escritorio con su lugar ([`topology::placed_screens`]).
    fn screens(&self, monitors: &[MonitorConfig]) -> Result<Vec<PlacedScreen>, DesktopError> {
        let _ = monitors;
        Err(DesktopError(
            "este escritorio no informa sus pantallas".to_owned(),
        ))
    }
    /// Dónde está el monitor con ese EDID, si está en el escritorio ([`topology::locate`]).
    fn locate(&self, key: &EdidKey) -> Result<Option<Bounds>, DesktopError> {
        let _ = key;
        Err(DesktopError(
            "este escritorio no informa sus pantallas".to_owned(),
        ))
    }
}

/// Escritorio sobre una [`DisplayTopology`], buscando el monitor por su `edid_model`
/// ([`topology::find_monitor`]).
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

    fn with<R>(
        &self,
        f: impl FnOnce(&mut T) -> Result<R, topology::TopologyError>,
    ) -> Result<R, topology::TopologyError> {
        let mut topo = self.topo.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut topo)
    }

    fn set(&self, monitor: &MonitorConfig, active: bool) -> Result<(), DesktopError> {
        self.with(|t| {
            if active {
                topology::attach(t, monitor)
            } else {
                topology::detach(t, monitor)
            }
        })
        .map(drop)
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

    /// Si `Off` cae al espejo (macOS sin la API privada), el paso cuenta como hecho: la
    /// integrada deja de ser un escritorio aparte, que es lo que importa al modo.
    fn set_builtin(&self, mode: BuiltinDisplay) -> Result<(), DesktopError> {
        self.with(|t| topology::set_builtin(t, mode))
            .map(drop)
            .map_err(|e| DesktopError(e.to_string()))
    }

    fn screens(&self, monitors: &[MonitorConfig]) -> Result<Vec<PlacedScreen>, DesktopError> {
        self.with(|t| t.displays())
            .map(|d| topology::placed_screens(&d, monitors))
            .map_err(|e| DesktopError(e.to_string()))
    }

    fn locate(&self, key: &EdidKey) -> Result<Option<Bounds>, DesktopError> {
        self.with(|t| t.displays())
            .map(|d| topology::locate(&d, key))
            .map_err(|e| DesktopError(e.to_string()))
    }
}

/// El escritorio real de `machine`, si este sistema es el de esa máquina: la topología de
/// Windows en la PC y la de Quartz en la Mac.
pub fn native(machine: Machine) -> Option<Arc<dyn DesktopControl>> {
    match machine {
        #[cfg(windows)]
        Machine::Pc => Some(Arc::new(TopologyDesktop::new(
            crate::windows::topology::WindowsTopology,
        ))),
        #[cfg(target_os = "macos")]
        Machine::Mac => Some(Arc::new(TopologyDesktop::new(
            crate::macos::MacDisplays::new(crate::macos::CgApi::new()),
        ))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quartz::MacTopology;
    use crate::quartz::fake::FakeQuartz;
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
    fn locates_a_monitor_by_edid_on_the_desktop() {
        let mut topo = FakeTopology::two_monitors();
        for (i, d) in topo.monitors.values_mut().enumerate() {
            d.bounds = Some(Bounds {
                x: i as i32 * 3440,
                y: 0,
                width: 3440,
                height: 1440,
            });
        }
        let d = TopologyDesktop::new(topo);
        let key = EdidKey {
            manufacturer: "SAM".into(),
            product_code: 0x7140,
            serial_number: 0,
            name: Some("S32BG85".into()),
        };
        assert_eq!(d.locate(&key).unwrap().map(|b| b.x), Some(3440));
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

    #[test]
    fn the_mac_desktop_uses_the_same_logic() {
        let d = TopologyDesktop::new(MacTopology::new(FakeQuartz::desk()));
        let m34 = request("m34", Machine::Mac).monitor;
        d.detach(&m34).unwrap();
        d.set_builtin(BuiltinDisplay::Off).unwrap();
        // Sin pantalla integrada ni el 34", el último externo no se quita.
        let mut last = m34.clone();
        last.id = "m32".into();
        last.edid_model = Some("Odyssey G5".into());
        let e = d.detach(&last).unwrap_err();
        assert_eq!(e.0, "m32: #3 es la última pantalla activa; no se desactiva");

        d.set_builtin(BuiltinDisplay::Extend).unwrap();
        d.attach(&m34).unwrap();
        let mut topo = d.into_inner();
        assert_eq!(topo.api().enabled_count(), 3);
        assert!(topo.disabled_ids().is_empty());

        // Sin la API privada, `Off` cae al espejo y el paso no falla.
        topo.api_mut().private_api = false;
        let d = TopologyDesktop::new(topo);
        d.set_builtin(BuiltinDisplay::Off).unwrap();
        let topo = d.into_inner();
        let b = topo.api().get(crate::quartz::DisplayId(1)).unwrap();
        assert!(b.enabled && b.mirror_of.is_some());
    }

    #[test]
    fn the_desktop_reports_its_screens() {
        let d = TopologyDesktop::new(MacTopology::new(FakeQuartz::desk()));
        let mut m34 = request("m34", Machine::Mac).monitor;
        m34.edid_model = Some("LC34G55T".into());
        let screens = d.screens(&[m34]).unwrap();
        let m34_pc = || {
            let mut m = request("m34", Machine::Pc).monitor;
            m.edid_model = Some("LC34G55T".into());
            m
        };
        assert!(screens.iter().any(|s| s.monitor.is_none()), "la integrada");
        assert!(screens.iter().any(|s| s.monitor == Some("m34".into())));
        // El doble de Windows no da el lugar de sus monitores: no se inventa ninguno.
        let d = TopologyDesktop::new(FakeTopology::two_monitors());
        assert!(d.screens(&[m34_pc()]).unwrap().is_empty());
    }

    #[test]
    fn native_backend_only_on_its_own_system() {
        assert_eq!(native(Machine::Pc).is_some(), cfg!(windows));
        assert_eq!(native(Machine::Mac).is_some(), cfg!(target_os = "macos"));
    }
}
