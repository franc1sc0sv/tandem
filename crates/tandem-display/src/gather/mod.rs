//! "Traer ventanas" (DESIGN §8.1): mover las ventanas que quedaron en la pantalla
//! integrada de la MacBook al monitor principal.
//!
//! [`WindowApi`] son las primitivas del sistema (listar ventanas, moverlas,
//! redimensionarlas). La implementación real usa la API de Accesibilidad y vive en
//! `crate::macos::ax`; aquí está la decisión de qué mover y adónde ([`plan`], [`gather`]),
//! que se prueba en Linux con [`fake::FakeWindows`].
//!
//! Reglas:
//! - Una ventana está en la pantalla con la que más se solapa (como la asigna macOS).
//! - Las minimizadas se dejan: viven en el Dock y la API no las mueve de forma fiable.
//! - El destino es el monitor externo principal o, si la principal es la integrada, el
//!   primer externo activo que no sea espejo.
//! - La ventana conserva su posición relativa y nunca crece; si no cabe, se encoge.
//! - Si una ventana falla, se sigue con las demás y se informa.

pub mod fake;

use std::fmt;

use crate::quartz::{Bounds, Display, DisplayId, TopologyError};

/// Ventana dentro del último listado de [`WindowApi::windows`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub usize);

impl fmt::Display for WindowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ventana {}", self.0)
    }
}

/// Una ventana de una aplicación, en puntos del escritorio global (las mismas
/// coordenadas que [`Display::bounds`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: WindowId,
    pub app: String,
    pub title: Option<String>,
    pub frame: Bounds,
    pub minimized: bool,
}

impl fmt::Display for Window {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.app)?;
        if let Some(t) = self.title.as_deref().filter(|t| !t.is_empty()) {
            write!(f, " «{t}»")?;
        }
        let b = self.frame;
        write!(f, " {}x{} en ({}, {})", b.width, b.height, b.x, b.y)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GatherError {
    #[error(
        "Tandem no tiene permiso de Accesibilidad (Ajustes del Sistema > Privacidad y seguridad > Accesibilidad)"
    )]
    PermissionDenied,
    #[error("la Mac no tiene pantalla integrada")]
    NoBuiltin,
    #[error("no hay un monitor externo activo al que llevar las ventanas")]
    NoTarget,
    #[error("{0} ya no existe")]
    UnknownWindow(WindowId),
    #[error("{op} falló (AXError {code})")]
    Os { op: &'static str, code: i32 },
    #[error("pantallas: {0}")]
    Topology(#[from] TopologyError),
    #[error("mover ventanas solo está disponible en macOS")]
    Unsupported,
}

/// Primitivas de ventanas del sistema.
pub trait WindowApi {
    /// Ventanas de todas las aplicaciones. Invalida los [`WindowId`] del listado anterior.
    fn windows(&mut self) -> Result<Vec<Window>, GatherError>;
    fn move_to(&mut self, id: WindowId, x: i32, y: i32) -> Result<(), GatherError>;
    fn resize(&mut self, id: WindowId, width: i32, height: i32) -> Result<(), GatherError>;
}

/// Un movimiento planeado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub window: Window,
    pub to: Bounds,
}

/// Lo que hay que hacer: adónde y qué.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Pantalla destino; `None` si no hay nada que mover.
    pub target: Option<DisplayId>,
    pub moves: Vec<Move>,
    /// Minimizadas que estaban en la integrada y se dejan donde están.
    pub skipped: Vec<Window>,
}

/// Resultado de [`gather`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GatherReport {
    pub target: Option<DisplayId>,
    pub moved: Vec<Move>,
    pub skipped: Vec<Window>,
    pub failed: Vec<(Window, GatherError)>,
}

impl GatherReport {
    /// ¿Se movieron todas las que había que mover?
    pub fn complete(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Decide qué ventanas mover y adónde, sin tocar nada.
pub fn plan(displays: &[Display], windows: &[Window]) -> Result<Plan, GatherError> {
    let builtin = displays
        .iter()
        .find(|d| d.builtin)
        .ok_or(GatherError::NoBuiltin)?;
    // Apagada o en espejo: lo que tenga encima ya no se queda escondido.
    if !builtin.enabled || builtin.mirror_of.is_some() {
        return Ok(Plan::default());
    }
    // Las que reflejan a otra comparten sus coordenadas: no cuentan como lugar propio.
    let screens: Vec<&Display> = displays
        .iter()
        .filter(|d| d.enabled && d.mirror_of.is_none())
        .collect();
    let on_builtin: Vec<&Window> = windows
        .iter()
        .filter(|w| screen_of(&screens, w.frame) == Some(builtin.id))
        .collect();
    if on_builtin.is_empty() {
        return Ok(Plan::default());
    }
    let externals = || screens.iter().filter(|d| !d.builtin);
    let target = externals()
        .find(|d| d.main)
        .or_else(|| externals().next())
        .ok_or(GatherError::NoTarget)?;

    let mut plan = Plan {
        target: Some(target.id),
        ..Plan::default()
    };
    for w in on_builtin {
        if w.minimized {
            plan.skipped.push(w.clone());
        } else {
            plan.moves.push(Move {
                window: w.clone(),
                to: relocate(w.frame, builtin.bounds, target.bounds),
            });
        }
    }
    Ok(plan)
}

/// Lista las ventanas y mueve las de la pantalla integrada al monitor principal.
pub fn gather<A: WindowApi>(
    api: &mut A,
    displays: &[Display],
) -> Result<GatherReport, GatherError> {
    let windows = api.windows()?;
    let plan = plan(displays, &windows)?;
    let mut report = GatherReport {
        target: plan.target,
        skipped: plan.skipped,
        ..GatherReport::default()
    };
    for m in plan.moves {
        match apply(api, &m) {
            Ok(()) => report.moved.push(m),
            // Sin permiso no va a funcionar con ninguna otra.
            Err(GatherError::PermissionDenied) => return Err(GatherError::PermissionDenied),
            Err(e) => report.failed.push((m.window, e)),
        }
    }
    Ok(report)
}

fn apply<A: WindowApi>(api: &mut A, m: &Move) -> Result<(), GatherError> {
    let from = m.window.frame;
    // Primero se mueve: encoger en la integrada (más chica) podría chocar con su borde.
    api.move_to(m.window.id, m.to.x, m.to.y)?;
    if (m.to.width, m.to.height) != (from.width, from.height) {
        api.resize(m.window.id, m.to.width, m.to.height)?;
    }
    Ok(())
}

/// Pantalla con la que más se solapa el rectángulo, si se solapa con alguna.
fn screen_of(screens: &[&Display], frame: Bounds) -> Option<DisplayId> {
    screens
        .iter()
        .map(|d| (overlap(d.bounds, frame), d.id))
        .filter(|(area, _)| *area > 0)
        // A igual área gana la de id menor, para que el resultado no dependa del orden.
        .max_by_key(|(area, id)| (*area, std::cmp::Reverse(*id)))
        .map(|(_, id)| id)
}

fn overlap(a: Bounds, b: Bounds) -> i64 {
    let w = axis_overlap(a.x, a.width, b.x, b.width);
    let h = axis_overlap(a.y, a.height, b.y, b.height);
    w * h
}

fn axis_overlap(a: i32, a_len: i32, b: i32, b_len: i32) -> i64 {
    let (a, a_len, b, b_len) = (
        i64::from(a),
        i64::from(a_len.max(0)),
        i64::from(b),
        i64::from(b_len.max(0)),
    );
    ((a + a_len).min(b + b_len) - a.max(b)).max(0)
}

/// Lleva `frame` de `from` a `to`: misma posición relativa, nunca más grande que `to`.
fn relocate(frame: Bounds, from: Bounds, to: Bounds) -> Bounds {
    let width = frame.width.clamp(1, to.width.max(1));
    let height = frame.height.clamp(1, to.height.max(1));
    Bounds {
        x: place(
            frame.x,
            frame.width,
            from.x,
            from.width,
            to.x,
            to.width,
            width,
        ),
        y: place(
            frame.y,
            frame.height,
            from.y,
            from.height,
            to.y,
            to.height,
            height,
        ),
        width,
        height,
    }
}

/// Un eje de [`relocate`]: la fracción del espacio libre que quedaba a la izquierda (o
/// arriba) se conserva en el destino.
fn place(pos: i32, len: i32, from: i32, from_len: i32, to: i32, to_len: i32, new_len: i32) -> i32 {
    let free_from = i64::from(from_len) - i64::from(len);
    let free_to = i64::from(to_len) - i64::from(new_len);
    let offset = i64::from(pos) - i64::from(from);
    let fraction = if free_from > 0 {
        (offset as f64 / free_from as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let x = i64::from(to) + (fraction * free_to.max(0) as f64).round() as i64;
    i32::try_from(x).unwrap_or(to)
}

#[cfg(test)]
mod tests;
