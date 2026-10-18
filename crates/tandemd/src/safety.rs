//! Seguridad de pantallas (T-087, DESIGN §8): ninguna máquina deja a un monitor sin salida
//! activa por un estado que no comprobó.
//!
//! Las reglas, de las que dependen las pruebas:
//! - Al arrancar, sincronizar, escanear o pasar por el asistente **no se quita nada** de
//!   ningún escritorio. Lo único que quita una salida es el paso `Detach` de un plan.
//! - Un plan solo pone `Detach` de un monitor después de su `SwitchInput`
//!   ([`tandem_core::Plan`]); si algo falla, la vuelta atrás vuelve a poner las salidas.
//! - Esta máquina anota en un libro ([`DetachLedger`]) lo que ella misma quitó. Si el
//!   agente arranca con algo anotado y el estado no está confirmado (siempre, tras un
//!   arranque), lo reactiva ([`restore_on_boot`]): un cierre a medias de un cambio de modo
//!   no puede dejar un monitor en negro.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use tandem_core::{Config, MonitorId};
use tandem_display::desktop::DesktopControl;

/// Las salidas que esta máquina quitó de su escritorio y todavía no volvió a poner.
#[derive(Debug, Default)]
pub struct DetachLedger {
    path: Option<PathBuf>,
    ids: Mutex<BTreeSet<String>>,
}

impl DetachLedger {
    /// Sin archivo: solo en memoria (pruebas).
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Con el libro guardado en `path` (JSON); si no existe o está dañado, vacío.
    pub fn open(path: PathBuf) -> Self {
        let ids = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<BTreeSet<String>>(&b).ok())
            .unwrap_or_default();
        DetachLedger {
            path: Some(path),
            ids: Mutex::new(ids),
        }
    }

    fn change(&self, f: impl FnOnce(&mut BTreeSet<String>) -> bool) {
        let mut ids = self.ids.lock().unwrap_or_else(PoisonError::into_inner);
        if f(&mut ids) {
            self.save(&ids);
        }
    }

    fn save(&self, ids: &BTreeSet<String>) {
        let Some(path) = &self.path else { return };
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec(ids)?)?;
            std::fs::rename(&tmp, path)
        })();
        if let Err(e) = result {
            // Sin libro no hay recuperación al arrancar, pero tampoco se frena el cambio.
            tracing::warn!("no se pudo guardar {}: {e}", path.display());
        }
    }

    /// Esta máquina acaba de quitar `monitor` de su escritorio.
    pub fn detached(&self, monitor: &MonitorId) {
        self.change(|ids| ids.insert(monitor.0.clone()));
    }

    /// Esta máquina volvió a poner `monitor` (o ya no existe).
    pub fn attached(&self, monitor: &MonitorId) {
        self.change(|ids| ids.remove(&monitor.0));
    }

    /// Lo que sigue quitado.
    pub fn pending(&self) -> Vec<MonitorId> {
        self.ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|id| MonitorId(id.clone()))
            .collect()
    }
}

/// Lo que pasó al reactivar.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Restored {
    /// Salidas que se volvieron a poner.
    pub attached: Vec<MonitorId>,
    /// Las que no se pudieron (siguen anotadas, con el motivo).
    pub failed: Vec<(MonitorId, String)>,
}

/// Al arrancar: vuelve a poner en el escritorio lo que esta máquina había quitado, salvo
/// que el estado ya esté confirmado (entonces lo quitado es lo que corresponde).
///
/// Un monitor anotado que ya no está en el puesto se intenta igual con el nombre que tenía
/// en `desk`; si no se lo encuentra, se olvida: no hay nada que reactivar.
pub fn restore_on_boot(
    ledger: &DetachLedger,
    desktop: &dyn DesktopControl,
    desk: &Config,
    state_confirmed: bool,
) -> Restored {
    let mut out = Restored::default();
    if state_confirmed {
        return out;
    }
    for id in ledger.pending() {
        let Some(monitor) = desk.monitor(&id) else {
            ledger.attached(&id);
            continue;
        };
        match desktop.attach(monitor) {
            Ok(()) => {
                ledger.attached(&id);
                tracing::warn!(%id, "salida reactivada al arrancar: el cambio de modo anterior no terminó");
                out.attached.push(id);
            }
            Err(e) => {
                tracing::error!(%id, "no se pudo reactivar la salida al arrancar: {e}");
                out.failed.push((id, e.to_string()));
            }
        }
    }
    out
}

/// Antes de borrar `monitors` del puesto: los que esta máquina había quitado de su
/// escritorio se vuelven a poner, porque después ya no habría con qué encontrarlos.
pub fn reattach_removed(
    ledger: &DetachLedger,
    desktop: &dyn DesktopControl,
    monitors: &[tandem_core::MonitorConfig],
) {
    let pending = ledger.pending();
    for m in monitors.iter().filter(|m| pending.contains(&m.id)) {
        match desktop.attach(m) {
            Ok(()) => ledger.attached(&m.id),
            Err(e) => {
                tracing::warn!(monitor = %m.id, "no se pudo reactivar al quitarlo del puesto: {e}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tandem_core::MonitorConfig;
    use tandem_display::desktop::DesktopError;

    use super::*;
    use crate::config::fixtures;

    #[derive(Default)]
    struct Recorder {
        calls: Mutex<Vec<String>>,
        fail: bool,
    }

    impl DesktopControl for Recorder {
        fn attach(&self, m: &MonitorConfig) -> Result<(), DesktopError> {
            self.calls.lock().unwrap().push(format!("attach {}", m.id));
            if self.fail {
                Err(DesktopError("sin salida libre".into()))
            } else {
                Ok(())
            }
        }
        fn detach(&self, m: &MonitorConfig) -> Result<(), DesktopError> {
            self.calls.lock().unwrap().push(format!("detach {}", m.id));
            Ok(())
        }
        fn set_builtin(&self, _: tandem_core::BuiltinDisplay) -> Result<(), DesktopError> {
            Ok(())
        }
    }

    #[test]
    fn the_ledger_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("detached.json");
        let a = DetachLedger::open(path.clone());
        a.detached(&"m34".into());
        a.detached(&"m32".into());
        a.attached(&"m32".into());
        let b = DetachLedger::open(path);
        assert_eq!(b.pending(), [MonitorId::from("m34")]);
    }

    #[test]
    fn a_damaged_ledger_is_an_empty_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("detached.json");
        std::fs::write(&path, "no es json").unwrap();
        assert!(DetachLedger::open(path).pending().is_empty());
    }

    #[test]
    fn boot_reattaches_what_this_machine_detached() {
        let desk = fixtures::desk().desk;
        let ledger = DetachLedger::in_memory();
        ledger.detached(&"m34".into());
        let desktop = Recorder::default();
        let got = restore_on_boot(&ledger, &desktop, &desk, false);
        assert_eq!(got.attached, [MonitorId::from("m34")]);
        assert_eq!(*desktop.calls.lock().unwrap(), ["attach m34"]);
        assert!(ledger.pending().is_empty());
    }

    #[test]
    fn boot_never_detaches_anything() {
        let desk = fixtures::desk().desk;
        let ledger = DetachLedger::in_memory();
        ledger.detached(&"m34".into());
        let desktop = Recorder::default();
        restore_on_boot(&ledger, &desktop, &desk, false);
        assert!(
            desktop
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|c| c.starts_with("attach")),
        );
    }

    #[test]
    fn a_confirmed_state_leaves_things_as_they_are() {
        let desk = fixtures::desk().desk;
        let ledger = DetachLedger::in_memory();
        ledger.detached(&"m34".into());
        let desktop = Recorder::default();
        let got = restore_on_boot(&ledger, &desktop, &desk, true);
        assert_eq!(got, Restored::default());
        assert!(desktop.calls.lock().unwrap().is_empty());
        assert_eq!(ledger.pending().len(), 1);
    }

    #[test]
    fn a_failed_reattach_stays_in_the_ledger() {
        let desk = fixtures::desk().desk;
        let ledger = DetachLedger::in_memory();
        ledger.detached(&"m34".into());
        let desktop = Recorder {
            fail: true,
            ..Default::default()
        };
        let got = restore_on_boot(&ledger, &desktop, &desk, false);
        assert!(got.attached.is_empty());
        assert_eq!(got.failed.len(), 1);
        assert_eq!(ledger.pending().len(), 1);
    }

    #[test]
    fn removing_a_monitor_first_brings_its_output_back() {
        let desk = fixtures::desk().desk;
        let ledger = DetachLedger::in_memory();
        ledger.detached(&"m34".into());
        let desktop = Recorder::default();
        reattach_removed(&ledger, &desktop, &desk.monitors);
        assert_eq!(*desktop.calls.lock().unwrap(), ["attach m34"]);
        assert!(ledger.pending().is_empty());
    }

    #[test]
    fn a_monitor_that_left_the_desk_is_forgotten() {
        let desk = fixtures::desk().desk;
        let ledger = DetachLedger::in_memory();
        ledger.detached(&"fantasma".into());
        let desktop = Recorder::default();
        restore_on_boot(&ledger, &desktop, &desk, false);
        assert!(ledger.pending().is_empty());
        assert!(desktop.calls.lock().unwrap().is_empty());
    }
}
