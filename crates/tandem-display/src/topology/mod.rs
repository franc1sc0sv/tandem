//! Topología de pantallas: qué monitores conectados forman parte del escritorio.
//!
//! Quitar un monitor del escritorio es lo que lo vuelve "fantasma" (DESIGN §8): la máquina
//! que no lo está mostrando lo saca para que no reciba ventanas ni cursor. Un solo trait
//! portable, [`DisplayTopology`], con un backend por sistema:
//!
//! - Windows: `crate::windows::topology::WindowsTopology` (`SetDisplayConfig`), con la
//!   decisión de qué rutas tocar en [`plan`] y su doble [`fake::FakeTopology`].
//! - macOS: [`crate::quartz::MacTopology`] sobre las primitivas de Quartz, con su doble
//!   `FakeQuartz`.
//!
//! Lo que se hace con un monitor de la configuración ([`attach`], [`detach`],
//! [`set_builtin`]) se escribe una sola vez aquí, sobre el trait.

pub mod fake;
pub mod plan;
#[cfg(test)]
mod props;

use std::fmt;

use tandem_core::{BuiltinDisplay, MonitorConfig, MonitorId};

use crate::ddc::Edid;

pub use plan::PathSlot;

/// Salida de vídeo de un adaptador de Windows. Estable mientras no cambie el hardware ni el
/// driver; para identificar un monitor entre reinicios se usa su EDID.
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

/// Rectángulo en el escritorio global (origen arriba a la izquierda de la principal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Monitor conectado, activo o no en el escritorio. `Id` es el identificador opaco del
/// backend ([`TargetId`] en Windows, `CGDirectDisplayID` en macOS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Display<Id> {
    pub id: Id,
    /// Nombre que da el sistema (en Windows sale del EDID, por ejemplo `LC34G55T`).
    pub name: String,
    /// Ruta de interfaz del monitor (`\\?\DISPLAY#SAM7140#...`); vacía en macOS.
    pub device_path: String,
    pub edid: Option<Edid>,
    /// La pantalla integrada de una laptop.
    pub builtin: bool,
    /// La principal del escritorio (la de la barra de menú en macOS).
    pub main: bool,
    /// ¿Forma parte del escritorio? `false` = fantasma quitado.
    pub active: bool,
    /// Pantalla de la que es espejo, si lo es.
    pub mirror_of: Option<Id>,
    /// Dónde está en el escritorio, si el sistema lo dice (solo las activas tienen).
    pub bounds: Option<Bounds>,
}

impl<Id> Display<Id> {
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

    /// ¿Es el monitor configurado? El `edid_model` tiene que ser el modelo completo (sin
    /// distinguir mayúsculas): una búsqueda parcial confundiría `G5` con `LC34G55T`. La
    /// pantalla integrada nunca es un monitor de la configuración.
    pub fn is_monitor(&self, model: &str) -> bool {
        let model = model.trim();
        !self.builtin
            && [
                self.edid.as_ref().and_then(|e| e.name.as_deref()),
                Some(&self.name),
            ]
            .into_iter()
            .flatten()
            .any(|n| !n.trim().is_empty() && n.trim().eq_ignore_ascii_case(model))
    }
}

impl<Id: fmt::Display> fmt::Display for Display<Id> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.model())?;
        if let Some(e) = &self.edid {
            write!(f, " ({}", e.pnp_id())?;
            if let Some(serial) = &e.serial_text {
                write!(f, ", serie {serial}")?;
            }
            write!(f, ")")?;
        }
        write!(f, " en {}", self.id)?;
        if self.builtin {
            write!(f, " (integrada)")?;
        }
        if self.main {
            write!(f, " (principal)")?;
        }
        if let Some(m) = &self.mirror_of {
            write!(f, " [espejo de {m}]")?;
        }
        let state = if self.active { "activo" } else { "inactivo" };
        write!(f, " [{state}]")?;
        if let Some(b) = self.bounds {
            write!(f, " {}x{} en ({}, {})", b.width, b.height, b.x, b.y)?;
        }
        Ok(())
    }
}

/// Errores de topología de todos los backends. Los identificadores van como texto porque
/// cada backend tiene el suyo.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TopologyError {
    #[error("ningún monitor conectado coincide con «{0}»")]
    NoMatch(String),
    #[error("«{query}» coincide con varios monitores: {}", candidates.join(", "))]
    Ambiguous {
        query: String,
        candidates: Vec<String>,
    },
    #[error("falta `edid_model` en la configuración para encontrarlo")]
    MissingModel,
    #[error("el monitor {0} no está conectado")]
    NotConnected(String),
    #[error("no queda una salida libre en el adaptador para activar {0}")]
    NoFreeSource(String),
    /// Desactivar la única pantalla activa dejaría la máquina sin imagen.
    #[error("{0} es la última pantalla activa; no se desactiva")]
    LastDisplay(String),
    #[error("esta máquina no tiene pantalla integrada")]
    NoBuiltin,
    #[error("no hay un monitor externo activo del que hacer espejo")]
    NoMirrorTarget,
    /// `CGSConfigureDisplayEnabled` no existe en esta versión de macOS.
    #[error("la API privada para desactivar pantallas no está disponible")]
    PrivateApiUnavailable,
    #[error("{op} falló: {message}")]
    Os { op: &'static str, message: String },
    #[error("{0} no está disponible en esta plataforma")]
    Unsupported(&'static str),
}

/// Lectura y cambio de qué monitores forman parte del escritorio.
///
/// `set_active` es idempotente: activar un monitor activo (o desactivar uno inactivo) no
/// hace nada y no es un error. Ningún backend desactiva la última pantalla activa
/// ([`TopologyError::LastDisplay`]).
pub trait DisplayTopology {
    type Id: Copy + Eq + Ord + fmt::Debug + fmt::Display;

    /// Monitores conectados, activos o no, ordenados por id.
    fn displays(&mut self) -> Result<Vec<Display<Self::Id>>, TopologyError>;

    fn set_active(&mut self, id: Self::Id, active: bool) -> Result<(), TopologyError>;

    /// Pone `id` en espejo de `master`, o lo vuelve extendido con `None`. Capacidad
    /// opcional: el escritorio de la PC no la usa.
    fn set_mirror(&mut self, id: Self::Id, master: Option<Self::Id>) -> Result<(), TopologyError> {
        let _ = (id, master);
        Err(TopologyError::Unsupported("el espejo de pantallas"))
    }
}

/// Busca un único monitor conectado que coincida con un texto libre (diagnóstico).
pub fn find<'a, Id: fmt::Display>(
    displays: &'a [Display<Id>],
    query: &str,
) -> Result<&'a Display<Id>, TopologyError> {
    one(displays.iter().filter(|d| d.matches(query)), query)
}

/// El monitor de la configuración, por su `edid_model`. Es la única forma en que la lógica
/// de modos encuentra un monitor, en las dos máquinas.
pub fn find_monitor<'a, Id: fmt::Display>(
    displays: &'a [Display<Id>],
    monitor: &MonitorConfig,
) -> Result<&'a Display<Id>, TopologyError> {
    let model = monitor
        .edid_model
        .as_deref()
        .ok_or(TopologyError::MissingModel)?;
    one(displays.iter().filter(|d| d.is_monitor(model)), model)
}

/// Una pantalla del escritorio, para la geometría del teclado y mouse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedScreen {
    /// El monitor de la configuración; `None` es la pantalla integrada.
    pub monitor: Option<MonitorId>,
    pub bounds: Bounds,
}

/// Las pantallas que forman parte del escritorio, con su lugar. Las que son espejo de
/// otra no cuentan (el cursor no las recorre aparte) y un monitor externo que no es de
/// la configuración (o no tiene `edid_model`) tampoco: Tandem no sabe dónde está.
pub fn placed_screens<Id>(
    displays: &[Display<Id>],
    monitors: &[MonitorConfig],
) -> Vec<PlacedScreen> {
    displays
        .iter()
        .filter(|d| d.active && d.mirror_of.is_none())
        .filter_map(|d| {
            let bounds = d.bounds?;
            let monitor = if d.builtin {
                None
            } else {
                let m = monitors.iter().find(|m| {
                    m.edid_model
                        .as_deref()
                        .is_some_and(|model| d.is_monitor(model))
                })?;
                Some(m.id.clone())
            };
            Some(PlacedScreen { monitor, bounds })
        })
        .collect()
}

fn one<'a, Id: fmt::Display>(
    hits: impl Iterator<Item = &'a Display<Id>>,
    query: &str,
) -> Result<&'a Display<Id>, TopologyError> {
    let hits: Vec<&Display<Id>> = hits.collect();
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
pub struct Change<Id> {
    pub display: Display<Id>,
    pub changed: bool,
}

/// Activa en el escritorio el monitor de la configuración.
pub fn attach<T: DisplayTopology>(
    topo: &mut T,
    monitor: &MonitorConfig,
) -> Result<Change<T::Id>, TopologyError> {
    set(topo, monitor, true)
}

/// Quita del escritorio el monitor de la configuración.
pub fn detach<T: DisplayTopology>(
    topo: &mut T,
    monitor: &MonitorConfig,
) -> Result<Change<T::Id>, TopologyError> {
    set(topo, monitor, false)
}

fn set<T: DisplayTopology>(
    topo: &mut T,
    monitor: &MonitorConfig,
    active: bool,
) -> Result<Change<T::Id>, TopologyError> {
    let displays = topo.displays()?;
    let display = find_monitor(&displays, monitor)?.clone();
    let changed = display.active != active;
    if changed {
        topo.set_active(display.id, active)?;
    }
    Ok(Change { display, changed })
}

/// Cómo quedó la pantalla integrada tras [`set_builtin`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinApplied {
    /// Se aplicó el modo pedido.
    Exact(BuiltinDisplay),
    /// Se pidió `Off`, el sistema no sabe desactivarla y quedó en espejo (DESIGN §8.1).
    MirrorFallback,
}

/// La pantalla integrada.
pub fn builtin<T: DisplayTopology>(topo: &mut T) -> Result<Display<T::Id>, TopologyError> {
    topo.displays()?
        .into_iter()
        .find(|d| d.builtin)
        .ok_or(TopologyError::NoBuiltin)
}

/// Aplica un modo a la pantalla integrada. En una máquina sin integrada, `NoBuiltin`.
pub fn set_builtin<T: DisplayTopology>(
    topo: &mut T,
    mode: BuiltinDisplay,
) -> Result<BuiltinApplied, TopologyError> {
    let b = builtin(topo)?;
    match mode {
        BuiltinDisplay::Extend => {
            if !b.active {
                topo.set_active(b.id, true)?;
            }
            if current(topo, b.id)?.is_some_and(|d| d.mirror_of.is_some()) {
                topo.set_mirror(b.id, None)?;
            }
        }
        BuiltinDisplay::Mirror => mirror_builtin(topo, &b)?,
        BuiltinDisplay::Off if !b.active => {}
        BuiltinDisplay::Off => match topo.set_active(b.id, false) {
            Ok(()) => {}
            Err(TopologyError::PrivateApiUnavailable) => {
                mirror_builtin(topo, &b)?;
                return Ok(BuiltinApplied::MirrorFallback);
            }
            Err(e) => return Err(e),
        },
    }
    Ok(BuiltinApplied::Exact(mode))
}

fn current<T: DisplayTopology>(
    topo: &mut T,
    id: T::Id,
) -> Result<Option<Display<T::Id>>, TopologyError> {
    Ok(topo.displays()?.into_iter().find(|d| d.id == id))
}

/// Espejo del monitor externo principal (o del primero activo si la principal es la
/// integrada).
fn mirror_builtin<T: DisplayTopology>(
    topo: &mut T,
    b: &Display<T::Id>,
) -> Result<(), TopologyError> {
    if !b.active {
        topo.set_active(b.id, true)?;
    }
    let all = topo.displays()?;
    let externals = || {
        all.iter()
            .filter(|d| !d.builtin && d.active && d.mirror_of.is_none())
    };
    let master = externals()
        .find(|d| d.main)
        .or_else(|| externals().next())
        .ok_or(TopologyError::NoMirrorTarget)?
        .id;
    let now = all.iter().find(|d| d.id == b.id);
    if now.and_then(|d| d.mirror_of) == Some(master) {
        return Ok(());
    }
    topo.set_mirror(b.id, Some(master))
}

/// Monitor quitado del escritorio que vuelve solo al soltar el guardia.
///
/// Es la "vuelta automática" de las pruebas en hardware: si la prueba falla a la mitad, el
/// monitor no se queda fuera del escritorio. Si ya estaba inactivo, no lo reactiva.
pub struct Detached<'a, T: DisplayTopology> {
    topo: &'a mut T,
    id: T::Id,
    restore: bool,
}

impl<'a, T: DisplayTopology> Detached<'a, T> {
    pub fn new(topo: &'a mut T, monitor: &MonitorConfig) -> Result<Self, TopologyError> {
        let change = detach(topo, monitor)?;
        Ok(Self {
            topo,
            id: change.display.id,
            restore: change.changed,
        })
    }

    pub fn id(&self) -> T::Id {
        self.id
    }

    /// Reactiva ya y devuelve el resultado, en vez de esperar al `Drop`.
    pub fn restore(mut self) -> Result<(), TopologyError> {
        self.restore_now()
    }

    fn restore_now(&mut self) -> Result<(), TopologyError> {
        if !std::mem::take(&mut self.restore) {
            return Ok(());
        }
        self.topo.set_active(self.id, true)
    }
}

impl<T: DisplayTopology> Drop for Detached<'_, T> {
    fn drop(&mut self) {
        // En Drop no hay a quién devolver el error; `restore` existe para quien lo necesite.
        let _ = self.restore_now();
    }
}

/// Monitor de la configuración con solo lo que usa la topología, para pruebas y
/// diagnóstico.
pub fn monitor_by_model(id: &str, model: &str) -> MonitorConfig {
    MonitorConfig {
        id: id.into(),
        label: id.into(),
        edid_model: Some(model.into()),
        wiring: Default::default(),
        ddc: None,
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

    fn m(model: &str) -> MonitorConfig {
        monitor_by_model("m", model)
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
    fn find_monitor_needs_the_whole_model() {
        let mut t = topo();
        let ds = t.displays().unwrap();
        assert_eq!(
            find_monitor(&ds, &m(" lc34g55t ")).unwrap().model(),
            "LC34G55T"
        );
        // Un pedazo del modelo no alcanza (en `find` sí).
        assert_eq!(
            find_monitor(&ds, &m("S32")),
            Err(TopologyError::NoMatch("S32".into()))
        );
        let mut none = m("x");
        none.edid_model = None;
        assert_eq!(find_monitor(&ds, &none), Err(TopologyError::MissingModel));
        assert_eq!(
            find_monitor(&ds, &m("")),
            Err(TopologyError::NoMatch("".into()))
        );
    }

    #[test]
    fn find_monitor_uses_the_system_name_without_edid() {
        let mut t = topo();
        for d in t.monitors.values_mut() {
            d.edid = None;
        }
        let ds = t.displays().unwrap();
        assert_eq!(find_monitor(&ds, &m("S32BG85")).unwrap().id.target, 2);
    }

    #[test]
    fn find_monitor_skips_the_builtin_and_reports_ambiguity() {
        let mut t = topo();
        let ds = t.displays().unwrap();
        let mut twins = ds.clone();
        twins[1].edid = ds[0].edid.clone();
        match find_monitor(&twins, &m("LC34G55T")) {
            Err(TopologyError::Ambiguous { candidates, .. }) => assert_eq!(candidates.len(), 2),
            other => panic!("se esperaba ambigüedad: {other:?}"),
        }
        twins[1].builtin = true;
        assert_eq!(find_monitor(&twins, &m("LC34G55T")).unwrap().id.target, 1);
    }

    #[test]
    fn detach_then_attach_round_trips() {
        let mut t = topo();
        let off = detach(&mut t, &m("LC34G55T")).unwrap();
        assert!(off.changed);
        assert_eq!(active_names(&mut t), ["S32BG85"]);
        let on = attach(&mut t, &m("LC34G55T")).unwrap();
        assert!(on.changed);
        assert_eq!(active_names(&mut t), ["LC34G55T", "S32BG85"]);
    }

    #[test]
    fn attach_and_detach_are_idempotent() {
        let mut t = topo();
        assert!(!attach(&mut t, &m("LC34G55T")).unwrap().changed);
        assert!(detach(&mut t, &m("LC34G55T")).unwrap().changed);
        assert!(!detach(&mut t, &m("LC34G55T")).unwrap().changed);
        assert_eq!(t.applied, 1);
    }

    #[test]
    fn last_active_display_is_never_detached() {
        let mut t = topo();
        detach(&mut t, &m("S32BG85")).unwrap();
        let err = detach(&mut t, &m("LC34G55T")).unwrap_err();
        assert!(matches!(err, TopologyError::LastDisplay(_)), "{err}");
        assert_eq!(active_names(&mut t), ["LC34G55T"]);
    }

    #[test]
    fn guard_restores_on_drop_and_on_explicit_restore() {
        let mut t = topo();
        {
            let g = Detached::new(&mut t, &m("LC34G55T")).unwrap();
            assert_eq!(g.id().target, 1);
        }
        assert_eq!(active_names(&mut t), ["LC34G55T", "S32BG85"]);

        let g = Detached::new(&mut t, &m("S32BG85")).unwrap();
        g.restore().unwrap();
        assert_eq!(active_names(&mut t), ["LC34G55T", "S32BG85"]);
        assert_eq!(t.applied, 4);
    }

    #[test]
    fn guard_does_not_reactivate_a_monitor_that_was_already_off() {
        let mut t = topo();
        detach(&mut t, &m("S32BG85")).unwrap();
        drop(Detached::new(&mut t, &m("S32BG85")).unwrap());
        assert_eq!(active_names(&mut t), ["LC34G55T"]);
    }

    #[test]
    fn os_failure_is_reported_and_leaves_topology_unchanged() {
        let mut t = topo();
        t.fail_next = true;
        let err = detach(&mut t, &m("LC34G55T")).unwrap_err();
        assert!(matches!(err, TopologyError::Os { .. }), "{err}");
        assert_eq!(active_names(&mut t), ["LC34G55T", "S32BG85"]);
    }

    #[test]
    fn a_machine_without_builtin_rejects_builtin_modes() {
        let mut t = topo();
        for mode in [
            BuiltinDisplay::Off,
            BuiltinDisplay::Mirror,
            BuiltinDisplay::Extend,
        ] {
            assert_eq!(set_builtin(&mut t, mode), Err(TopologyError::NoBuiltin));
        }
        assert_eq!(t.applied, 0);
    }

    #[test]
    fn mirror_is_unsupported_by_default() {
        let mut t = topo();
        let id = t.displays().unwrap()[0].id;
        assert_eq!(
            t.set_mirror(id, None),
            Err(TopologyError::Unsupported("el espejo de pantallas"))
        );
    }

    #[test]
    fn placed_screens_are_the_active_ones_of_the_config() {
        let bounds = |x| Bounds {
            x,
            y: 0,
            width: 100,
            height: 50,
        };
        let base = Display {
            id: 0u32,
            name: "LC34G55T".into(),
            device_path: String::new(),
            edid: None,
            builtin: false,
            main: false,
            active: true,
            mirror_of: None,
            bounds: Some(bounds(0)),
        };
        let displays = vec![
            base.clone(),
            Display {
                id: 1,
                name: "Integrada".into(),
                builtin: true,
                bounds: Some(bounds(100)),
                ..base.clone()
            },
            // Inactivo, espejo, sin lugar y desconocido: no cuentan.
            Display {
                id: 2,
                name: "S32BG85".into(),
                active: false,
                ..base.clone()
            },
            Display {
                id: 3,
                mirror_of: Some(0),
                ..base.clone()
            },
            Display {
                id: 4,
                bounds: None,
                ..base.clone()
            },
            Display {
                id: 5,
                name: "OTRO".into(),
                ..base.clone()
            },
        ];
        let mut m32 = monitor_by_model("m32", "S32BG85");
        m32.edid_model = None;
        let monitors = [monitor_by_model("m34", "LC34G55T"), m32];
        assert_eq!(
            placed_screens(&displays, &monitors),
            vec![
                PlacedScreen {
                    monitor: Some("m34".into()),
                    bounds: bounds(0)
                },
                PlacedScreen {
                    monitor: None,
                    bounds: bounds(100)
                },
            ]
        );
    }

    #[test]
    fn display_text_shows_identity_and_state() {
        let d = Display {
            id: TargetId {
                adapter: 0x1_0000_0002,
                target: 4,
            },
            name: "Monitor".into(),
            device_path: String::new(),
            edid: edid::parse(&edid::tests::sample("LC34G55T", "H4ZR")).ok(),
            builtin: false,
            main: false,
            active: false,
            mirror_of: None,
            bounds: None,
        };
        assert_eq!(
            d.to_string(),
            "LC34G55T (SAM7140, serie H4ZR) en 0x100000002/4 [inactivo]"
        );
        let plain = Display {
            edid: None,
            builtin: true,
            main: true,
            active: true,
            mirror_of: Some(d.id),
            bounds: Some(Bounds {
                x: -1512,
                y: 0,
                width: 1512,
                height: 982,
            }),
            ..d
        };
        assert_eq!(plain.model(), "Monitor");
        assert_eq!(
            plain.to_string(),
            "Monitor en 0x100000002/4 (integrada) (principal) [espejo de 0x100000002/4] \
             [activo] 1512x982 en (-1512, 0)"
        );
    }
}
