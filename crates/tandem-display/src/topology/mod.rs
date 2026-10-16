//! Topología de pantallas: qué monitores conectados forman parte del escritorio.
//!
//! Quitar un monitor del escritorio es lo que lo vuelve "fantasma" (DESIGN §8): la máquina
//! que no lo está mostrando lo saca para que no reciba ventanas ni cursor. La lógica de qué
//! rutas activar o desactivar es portable ([`plan`]); el backend real de Windows
//! (`crate::windows::topology`) solo traduce a `QueryDisplayConfig`/`SetDisplayConfig`.

pub mod fake;
pub mod plan;

use std::fmt;

use crate::ddc::Edid;

pub use plan::PathSlot;

/// Salida de vídeo de un adaptador. Estable mientras no cambie el hardware ni el driver;
/// para identificar un monitor entre reinicios se usa su EDID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TargetId {
    /// LUID del adaptador empaquetado en 64 bits.
    pub adapter: u64,
    pub target: u32,
}

impl fmt::Display for TargetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}/{}", self.adapter, self.target)
    }
}

/// Monitor conectado, activo o no en el escritorio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Display {
    pub target: TargetId,
    /// Nombre que da el sistema (en Windows sale del EDID, por ejemplo `LC34G55T`).
    pub name: String,
    /// Ruta de interfaz del monitor (`\\?\DISPLAY#SAM7140#...`).
    pub device_path: String,
    pub edid: Option<Edid>,
    /// ¿Forma parte del escritorio?
    pub active: bool,
}

impl Display {
    /// Nombre del modelo según el EDID, o el del sistema si no hay EDID.
    pub fn model(&self) -> &str {
        self.edid
            .as_ref()
            .and_then(|e| e.name.as_deref())
            .unwrap_or(&self.name)
    }

    /// ¿Coincide con un texto de búsqueda? Mismo criterio que `MonitorInfo::matches`, para
    /// que el mismo texto identifique el monitor en DDC y en la topología.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        let mut fields = vec![self.name.to_lowercase(), self.device_path.to_lowercase()];
        if let Some(e) = &self.edid {
            fields.push(e.pnp_id().to_lowercase());
            if let Some(name) = &e.name {
                fields.push(name.to_lowercase());
            }
            if let Some(serial) = &e.serial_text {
                fields.push(serial.to_lowercase());
            }
        }
        fields.iter().any(|f| f.contains(&q))
    }
}

impl fmt::Display for Display {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.model())?;
        if let Some(e) = &self.edid {
            write!(f, " ({}", e.pnp_id())?;
            if let Some(serial) = &e.serial_text {
                write!(f, ", serie {serial}")?;
            }
            write!(f, ")")?;
        }
        let state = if self.active { "activo" } else { "inactivo" };
        write!(f, " en {} [{state}]", self.target)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TopologyError {
    #[error("ningún monitor conectado coincide con «{0}»")]
    NoMatch(String),
    #[error("«{query}» coincide con varios monitores: {}", candidates.join(", "))]
    Ambiguous {
        query: String,
        candidates: Vec<String>,
    },
    #[error("el monitor {0} no está conectado")]
    NotConnected(TargetId),
    #[error("no queda una salida libre en el adaptador para activar {0}")]
    NoFreeSource(TargetId),
    /// Desactivar el único monitor activo dejaría la máquina sin escritorio visible.
    #[error("{0} es el único monitor activo; no se desactiva")]
    LastDisplay(TargetId),
    #[error("{op} falló: {message} ({code:#010X})")]
    Os {
        op: &'static str,
        code: u32,
        message: String,
    },
    #[error("la topología de pantallas no está disponible en esta plataforma")]
    Unsupported,
}

/// Lectura y cambio de qué monitores forman parte del escritorio.
///
/// `set_active` es idempotente: activar un monitor activo (o desactivar uno inactivo) no
/// hace nada y no es un error.
pub trait DisplayTopology {
    /// Monitores conectados, activos o no, ordenados por [`TargetId`].
    fn displays(&mut self) -> Result<Vec<Display>, TopologyError>;
    fn set_active(&mut self, target: TargetId, active: bool) -> Result<(), TopologyError>;
}

/// Busca un único monitor conectado que coincida con `query`.
pub fn find<'a>(displays: &'a [Display], query: &str) -> Result<&'a Display, TopologyError> {
    let hits: Vec<&Display> = displays.iter().filter(|d| d.matches(query)).collect();
    match hits.as_slice() {
        [] => Err(TopologyError::NoMatch(query.to_owned())),
        [one] => Ok(one),
        many => Err(TopologyError::Ambiguous {
            query: query.to_owned(),
            candidates: many.iter().map(|d| d.to_string()).collect(),
        }),
    }
}

/// Resultado de [`attach`]/[`detach`]: si hubo que cambiar algo y qué monitor era.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub display: Display,
    pub changed: bool,
}

/// Activa en el escritorio el monitor que coincida con `query`.
pub fn attach(topo: &mut impl DisplayTopology, query: &str) -> Result<Change, TopologyError> {
    set(topo, query, true)
}

/// Quita del escritorio el monitor que coincida con `query`.
pub fn detach(topo: &mut impl DisplayTopology, query: &str) -> Result<Change, TopologyError> {
    set(topo, query, false)
}

fn set(
    topo: &mut impl DisplayTopology,
    query: &str,
    active: bool,
) -> Result<Change, TopologyError> {
    let displays = topo.displays()?;
    let display = find(&displays, query)?.clone();
    let changed = display.active != active;
    if changed {
        topo.set_active(display.target, active)?;
    }
    Ok(Change { display, changed })
}

/// Monitor quitado del escritorio que vuelve solo al soltar el guardia.
///
/// Es la "vuelta automática" de las pruebas en hardware: si la prueba falla a la mitad, el
/// monitor no se queda fuera del escritorio. Si ya estaba inactivo, no lo reactiva.
pub struct Detached<'a, T: DisplayTopology> {
    topo: &'a mut T,
    target: TargetId,
    restore: bool,
}

impl<'a, T: DisplayTopology> Detached<'a, T> {
    pub fn new(topo: &'a mut T, query: &str) -> Result<Self, TopologyError> {
        let change = detach(topo, query)?;
        Ok(Self {
            topo,
            target: change.display.target,
            restore: change.changed,
        })
    }

    pub fn target(&self) -> TargetId {
        self.target
    }

    /// Reactiva ya y devuelve el resultado, en vez de esperar al `Drop`.
    pub fn restore(mut self) -> Result<(), TopologyError> {
        self.restore_now()
    }

    fn restore_now(&mut self) -> Result<(), TopologyError> {
        if !std::mem::take(&mut self.restore) {
            return Ok(());
        }
        self.topo.set_active(self.target, true)
    }
}

impl<T: DisplayTopology> Drop for Detached<'_, T> {
    fn drop(&mut self) {
        // En Drop no hay a quién devolver el error; `restore` existe para quien lo necesite.
        let _ = self.restore_now();
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeTopology;
    use super::*;
    use crate::ddc::edid;

    fn topo() -> FakeTopology {
        FakeTopology::two_monitors()
    }

    fn active_names(t: &mut FakeTopology) -> Vec<String> {
        t.displays()
            .unwrap()
            .into_iter()
            .filter(|d| d.active)
            .map(|d| d.model().to_owned())
            .collect()
    }

    #[test]
    fn find_by_model_pnp_serial_and_path() {
        let mut t = topo();
        let ds = t.displays().unwrap();
        assert_eq!(find(&ds, "lc34g55t").unwrap().model(), "LC34G55T");
        assert_eq!(find(&ds, "S32").unwrap().model(), "S32BG85");
        assert_eq!(find(&ds, "H4ZR").unwrap().model(), "LC34G55T");
        assert_eq!(find(&ds, "UID4352").unwrap().model(), "LC34G55T");
    }

    #[test]
    fn find_reports_no_match_and_ambiguity() {
        let mut t = topo();
        let ds = t.displays().unwrap();
        assert_eq!(
            find(&ds, "dell"),
            Err(TopologyError::NoMatch("dell".into()))
        );
        // Los dos son SAM7140 en el doble.
        match find(&ds, "SAM") {
            Err(TopologyError::Ambiguous { candidates, .. }) => assert_eq!(candidates.len(), 2),
            other => panic!("se esperaba ambigüedad: {other:?}"),
        }
    }

    #[test]
    fn detach_then_attach_round_trips() {
        let mut t = topo();
        let off = detach(&mut t, "LC34G55T").unwrap();
        assert!(off.changed);
        assert_eq!(active_names(&mut t), ["S32BG85"]);
        let on = attach(&mut t, "LC34G55T").unwrap();
        assert!(on.changed);
        assert_eq!(active_names(&mut t), ["LC34G55T", "S32BG85"]);
    }

    #[test]
    fn attach_and_detach_are_idempotent() {
        let mut t = topo();
        assert!(!attach(&mut t, "LC34G55T").unwrap().changed);
        assert!(detach(&mut t, "LC34G55T").unwrap().changed);
        assert!(!detach(&mut t, "LC34G55T").unwrap().changed);
        assert_eq!(t.applied, 1);
    }

    #[test]
    fn last_active_display_is_never_detached() {
        let mut t = topo();
        detach(&mut t, "S32BG85").unwrap();
        let err = detach(&mut t, "LC34G55T").unwrap_err();
        assert!(matches!(err, TopologyError::LastDisplay(_)), "{err}");
        assert_eq!(active_names(&mut t), ["LC34G55T"]);
    }

    #[test]
    fn guard_restores_on_drop_and_on_explicit_restore() {
        let mut t = topo();
        {
            let g = Detached::new(&mut t, "LC34G55T").unwrap();
            assert_eq!(g.target().target, 1);
        }
        assert_eq!(active_names(&mut t), ["LC34G55T", "S32BG85"]);

        let g = Detached::new(&mut t, "S32BG85").unwrap();
        g.restore().unwrap();
        assert_eq!(active_names(&mut t), ["LC34G55T", "S32BG85"]);
        assert_eq!(t.applied, 4);
    }

    #[test]
    fn guard_does_not_reactivate_a_monitor_that_was_already_off() {
        let mut t = topo();
        detach(&mut t, "S32BG85").unwrap();
        drop(Detached::new(&mut t, "S32BG85").unwrap());
        assert_eq!(active_names(&mut t), ["LC34G55T"]);
    }

    #[test]
    fn os_failure_is_reported_and_leaves_topology_unchanged() {
        let mut t = topo();
        t.fail_next = true;
        let err = detach(&mut t, "LC34G55T").unwrap_err();
        assert!(matches!(err, TopologyError::Os { .. }), "{err}");
        assert_eq!(active_names(&mut t), ["LC34G55T", "S32BG85"]);
    }

    #[test]
    fn display_text_shows_identity_and_state() {
        let d = Display {
            target: TargetId {
                adapter: 0x1_0000_0002,
                target: 4,
            },
            name: "Monitor".into(),
            device_path: String::new(),
            edid: edid::parse(&edid::tests::sample("LC34G55T", "H4ZR")).ok(),
            active: false,
        };
        assert_eq!(
            d.to_string(),
            "LC34G55T (SAM7140, serie H4ZR) en 0x100000002/4 [inactivo]"
        );
        let plain = Display { edid: None, ..d };
        assert_eq!(plain.model(), "Monitor");
    }
}
