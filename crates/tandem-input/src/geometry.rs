//! Geometría de pantallas y cruce de bordes entre las dos máquinas.
//!
//! Cada máquina tiene sus pantallas en sus coordenadas nativas del cursor (puntos en
//! macOS, píxeles físicos en Windows Per-Monitor-V2). [`Layout`] las toma del acomodo
//! ([`Arrangement`], DESIGN §7): las declaradas en el puesto compartido o, sin declarar,
//! las que lee cada máquina; y pone las dos máquinas una al lado de la otra según de qué
//! lado de B está A. Se recalcula entero con cada cambio: es un valor, no hay
//! nada que invalidar.
//!
//! El cruce se prueba contra el borde de la pantalla donde está el cursor, nunca
//! contra la caja que encierra a todas (docs/research/input-engine.md), y la
//! posición se lleva como fracción a lo largo del borde compartido, para que
//! monitores de distinto tamaño se sientan continuos.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use tandem_core::Machine;
pub use tandem_core::{Align, Side};

/// Posición del cursor en las coordenadas nativas de una máquina.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// Rectángulo semiabierto `[x, x + width) × [y, y + height)`, con `y` hacia abajo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn left(&self) -> f64 {
        f64::from(self.x)
    }

    pub fn right(&self) -> f64 {
        f64::from(self.x) + f64::from(self.width)
    }

    pub fn top(&self) -> f64 {
        f64::from(self.y)
    }

    pub fn bottom(&self) -> f64 {
        f64::from(self.y) + f64::from(self.height)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.left() && p.x < self.right() && p.y >= self.top() && p.y < self.bottom()
    }

    /// Lleva el punto al último píxel dentro del rectángulo.
    pub fn clamp(&self, p: Point) -> Point {
        Point::new(
            p.x.clamp(self.left(), self.right() - 1.0),
            p.y.clamp(self.top(), self.bottom() - 1.0),
        )
    }

    /// Distancia del punto al rectángulo (0 si está dentro).
    pub fn distance(&self, p: Point) -> f64 {
        let dx = (self.left() - p.x).max(p.x - (self.right() - 1.0)).max(0.0);
        let dy = (self.top() - p.y).max(p.y - (self.bottom() - 1.0)).max(0.0);
        dx.hypot(dy)
    }
}

/// El id de una pantalla en el acomodo (`a-1`, o el de un `[[screens]]`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScreenId(pub String);

impl From<&str> for ScreenId {
    fn from(s: &str) -> Self {
        ScreenId(s.to_owned())
    }
}

impl std::fmt::Display for ScreenId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Una pantalla tal como la reporta su máquina.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    pub id: ScreenId,
    /// En coordenadas nativas del cursor de su máquina.
    pub rect: Rect,
    /// Unidades nativas por punto lógico: `dpi / 96` en Windows Per-Monitor-V2;
    /// `1.0` en macOS, donde el cursor ya se mueve en puntos.
    pub scale: f64,
}

impl Screen {
    pub fn new(id: ScreenId, rect: Rect, scale: f64) -> Self {
        Self { id, rect, scale }
    }

    /// Delta nativo de esta pantalla → puntos lógicos (lo que viaja por el enlace).
    pub fn to_logical(&self, dx: f64, dy: f64) -> (f64, f64) {
        (dx / self.scale, dy / self.scale)
    }

    /// Puntos lógicos → delta nativo de esta pantalla. Con [`Screen::to_logical`]
    /// en el origen, la velocidad se siente igual en monitores de distinto DPI.
    pub fn to_native(&self, dx: f64, dy: f64) -> (f64, f64) {
        (dx * self.scale, dy * self.scale)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayoutOptions {
    /// Margen de las esquinas en puntos lógicos: ahí no se cruza, para no
    /// pelear con las esquinas activas de macOS ni con los botones de las ventanas.
    pub corner_margin: f64,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            corner_margin: 16.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum GeometryError {
    #[error("la pantalla {0:?} no tiene área")]
    EmptyScreen(ScreenId),
    #[error("la pantalla {id:?} tiene una escala inválida ({scale})")]
    InvalidScale { id: ScreenId, scale: f64 },
    #[error("el margen de las esquinas es inválido ({0})")]
    InvalidMargin(f64),
}

/// El acomodo de las dos máquinas: las pantallas de cada una, de qué lado de B está A y,
/// si se declaró, cómo se alinean (D18). Una máquina sin pantallas (apagada, o que todavía
/// no las mandó) no tiene a dónde cruzar.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Arrangement {
    pub a: Vec<Screen>,
    pub b: Vec<Screen>,
    pub a_side: Side,
    /// `Some`: los dos escritorios se tocan solo en el tramo que comparten con esa
    /// alineación, medido en puntos lógicos, y el cursor sale a la misma altura. `None` (sin
    /// acomodo declarado): todo el borde de cada uno cruza y la posición va como fracción
    /// del largo.
    pub align: Option<Align>,
}

/// El tramo compartido sobre un borde de largo `mine` frente a uno de largo `theirs` (los
/// dos en puntos lógicos), con `align`: desde dónde hasta dónde, en el borde propio, está
/// el otro escritorio. Es el borde de cruce. Vacío (`start >= end`) si no se tocan.
pub fn shared_span(mine: f64, theirs: f64, align: Align) -> (f64, f64) {
    let off = (mine - theirs) * align.factor();
    (off.max(0.0), (off + theirs).min(mine))
}

/// Tramo del borde que mira a la otra máquina y por el que se puede cruzar, sobre el eje
/// del borde: `y` en un borde izquierdo o derecho, `x` en uno de arriba o abajo.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Segment {
    screen: usize,
    t0: f64,
    t1: f64,
}

impl Segment {
    fn len(&self) -> f64 {
        self.t1 - self.t0
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Facing {
    facing: Side,
    /// Ordenados a lo largo del eje; la posición del cruce se mide sobre su concatenación.
    edge: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq)]
struct MachineLayout {
    screens: Vec<Screen>,
    /// `None` si alguna de las dos no tiene pantallas: no hay a dónde cruzar.
    side: Option<Facing>,
}

impl MachineLayout {
    /// El largo de un tramo medido en unidades nativas (`logical = false`) o en puntos
    /// lógicos, con la escala de su pantalla.
    fn measure(&self, seg: &Segment, logical: bool) -> f64 {
        if logical {
            seg.len() / self.screens[seg.screen].scale
        } else {
            seg.len()
        }
    }

    /// El largo de todo el borde.
    fn edge_len(&self, logical: bool) -> Option<f64> {
        let side = self.side.as_ref()?;
        Some(side.edge.iter().map(|s| self.measure(s, logical)).sum())
    }
}

/// Resultado de mover el cursor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Motion {
    /// El cursor sigue en la misma máquina, en esta posición.
    Local(Point),
    /// El cursor sale por el borde y entra a la otra máquina en esta posición.
    Cross { to: Machine, pos: Point },
}

/// Pantallas de las dos máquinas y cómo se tocan.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    opts: LayoutOptions,
    align: Option<Align>,
    a: MachineLayout,
    b: MachineLayout,
}

/// Las coordenadas de un rectángulo sobre el eje del borde de `side`.
fn span(r: &Rect, side: Side) -> (f64, f64) {
    if side.is_vertical_edge() {
        (r.top(), r.bottom())
    } else {
        (r.left(), r.right())
    }
}

impl Layout {
    /// Arma el layout a partir del acomodo. Todas las pantallas cuentan: el acomodo ya es
    /// lo que se mira.
    pub fn compute(arr: &Arrangement, opts: LayoutOptions) -> Result<Self, GeometryError> {
        if !opts.corner_margin.is_finite() || opts.corner_margin < 0.0 {
            return Err(GeometryError::InvalidMargin(opts.corner_margin));
        }
        for s in arr.a.iter().chain(&arr.b) {
            if s.rect.width == 0 || s.rect.height == 0 {
                return Err(GeometryError::EmptyScreen(s.id.clone()));
            }
            if !s.scale.is_finite() || s.scale <= 0.0 {
                return Err(GeometryError::InvalidScale {
                    id: s.id.clone(),
                    scale: s.scale,
                });
            }
        }

        // A, de un lado de B, mira hacia el otro.
        let a_facing = (!arr.a.is_empty() && !arr.b.is_empty()).then(|| arr.a_side.opposite());

        let build = |screens: Vec<Screen>, facing: Option<Side>| {
            let side = facing.map(|facing| Facing {
                facing,
                edge: exposed_edge(&screens, facing),
            });
            MachineLayout { screens, side }
        };

        Ok(Self {
            opts,
            align: arr.align,
            a: build(arr.a.clone(), a_facing),
            b: build(arr.b.clone(), a_facing.map(Side::opposite)),
        })
    }

    pub fn options(&self) -> LayoutOptions {
        self.opts
    }

    /// Pantallas de una máquina.
    pub fn screens(&self, machine: Machine) -> &[Screen] {
        &self.machine(machine).screens
    }

    /// Lado de la máquina por el que se pasa a la otra; `None` si no se puede cruzar.
    pub fn facing(&self, machine: Machine) -> Option<Side> {
        self.machine(machine).side.as_ref().map(|s| s.facing)
    }

    /// El borde de cruce de `machine`: desde dónde hasta dónde de su borde (en puntos
    /// lógicos desde el inicio, arriba o a la izquierda) se puede pasar a la otra. Sin
    /// alineación, todo el borde. `None` si no se puede cruzar.
    pub fn crossing_span(&self, machine: Machine) -> Option<(f64, f64)> {
        let mine = self.machine(machine).edge_len(true)?;
        match self.align {
            None => Some((0.0, mine)),
            Some(align) => {
                let theirs = self.machine(machine.other()).edge_len(true)?;
                let (start, end) = shared_span(mine, theirs, align);
                (start < end).then_some((start, end))
            }
        }
    }

    /// Pantalla que contiene el punto o, si cae fuera de todas, la más cercana.
    pub fn screen_at(&self, machine: Machine, pos: Point) -> Option<&Screen> {
        self.screen_index(machine, pos)
            .map(|i| &self.machine(machine).screens[i])
    }

    /// Mueve el cursor de `machine` desde `pos` con un delta nativo.
    ///
    /// Si el movimiento sale por el borde que mira a la otra máquina, dentro del borde de
    /// cruce y fuera del margen de las esquinas, devuelve dónde aparece en la otra. Si no,
    /// el cursor pasa a otra pantalla local o se queda contra el borde de la actual, como
    /// hace el sistema. Sin pantallas devuelve `pos` tal cual.
    pub fn step(&self, machine: Machine, pos: Point, dx: f64, dy: f64) -> Motion {
        self.step_inner(machine, pos, dx, dy, true)
    }

    /// Como [`Layout::step`], sin cruzar nunca a la otra máquina: el cursor bloqueado
    /// (el atajo del servidor) se queda contra el borde.
    pub fn step_within(&self, machine: Machine, pos: Point, dx: f64, dy: f64) -> Point {
        match self.step_inner(machine, pos, dx, dy, false) {
            Motion::Local(p) => p,
            // `step_inner` sin cruce no devuelve otra cosa.
            Motion::Cross { .. } => pos,
        }
    }

    fn step_inner(&self, machine: Machine, pos: Point, dx: f64, dy: f64, cross: bool) -> Motion {
        let ml = self.machine(machine);
        let Some(cur) = self.screen_index(machine, pos) else {
            return Motion::Local(pos);
        };
        let screen = &ml.screens[cur];
        // Desde fuera de toda pantalla (geometría recién cambiada) se parte del
        // punto más cercano de la pantalla actual.
        let pos = screen.rect.clamp(pos);
        let target = Point::new(pos.x + dx, pos.y + dy);
        if ml.screens.iter().any(|s| s.rect.contains(target)) {
            return Motion::Local(target);
        }
        if cross && let Some(cross) = self.crossing(machine, cur, pos, dx, dy) {
            return cross;
        }
        Motion::Local(screen.rect.clamp(target))
    }

    fn crossing(
        &self,
        machine: Machine,
        cur: usize,
        pos: Point,
        dx: f64,
        dy: f64,
    ) -> Option<Motion> {
        let ml = self.machine(machine);
        let side = ml.side.as_ref()?;
        let rect = ml.screens[cur].rect;
        // Si el movimiento pasa el borde que mira a la otra: qué parte del movimiento hace
        // falta para tocarlo.
        let t = match side.facing {
            Side::Right if dx > 0.0 && pos.x + dx >= rect.right() => (rect.right() - pos.x) / dx,
            Side::Left if dx < 0.0 && pos.x + dx < rect.left() => (rect.left() - pos.x) / dx,
            Side::Down if dy > 0.0 && pos.y + dy >= rect.bottom() => (rect.bottom() - pos.y) / dy,
            Side::Up if dy < 0.0 && pos.y + dy < rect.top() => (rect.top() - pos.y) / dy,
            _ => return None,
        };
        let t = t.clamp(0.0, 1.0);
        // Dónde, a lo largo del borde, lo toca la trayectoria.
        let hit = if side.facing.is_vertical_edge() {
            pos.y + dy * t
        } else {
            pos.x + dx * t
        };

        let margin = self.opts.corner_margin * ml.screens[cur].scale;
        let (lo, hi) = span(&rect, side.facing);
        if hit < lo + margin || hit >= hi - margin {
            return None;
        }
        // Sin alineación se mide en unidades nativas (fracción del largo); con alineación,
        // en puntos lógicos (la misma distancia en las dos).
        let logical = self.align.is_some();
        let mut before = 0.0;
        let mut offset = None;
        for seg in &side.edge {
            if seg.screen == cur && hit >= seg.t0 && hit < seg.t1 {
                let inside = Segment { t1: hit, ..*seg };
                offset = Some(before + ml.measure(&inside, logical));
                break;
            }
            before += ml.measure(seg, logical);
        }
        let offset = offset?;
        let mine = ml.edge_len(logical)?;

        let to = machine.other();
        let theirs = self.machine(to).edge_len(logical)?;
        let at = match self.align {
            None => offset / mine * theirs,
            Some(align) => {
                // Dónde empieza el otro escritorio sobre este borde.
                let at = offset - (mine - theirs) * align.factor();
                if at < 0.0 || at >= theirs {
                    return None;
                }
                at
            }
        };
        let pos = self.land(to, at, logical)?;
        Some(Motion::Cross { to, pos })
    }

    /// Punto del borde de `machine` que está a `at` de su inicio, medido como en
    /// [`MachineLayout::measure`] (el último del borde si `at` lo pasa).
    fn land(&self, machine: Machine, at: f64, logical: bool) -> Option<Point> {
        let ml = self.machine(machine);
        let side = ml.side.as_ref()?;
        let last = side.edge.last()?;
        let mut left = at;
        let mut hit = (last, last.len());
        for seg in &side.edge {
            let len = ml.measure(seg, logical);
            if left < len {
                // De la medida del tramo a unidades nativas.
                hit = (seg, left / len * seg.len());
                break;
            }
            left -= len;
        }
        let (seg, offset) = hit;
        let rect = ml.screens[seg.screen].rect;
        let t = (seg.t0 + offset).min(seg.t1 - 1.0);
        let p = match side.facing {
            Side::Left => Point::new(rect.left(), t),
            Side::Right => Point::new(rect.right() - 1.0, t),
            Side::Up => Point::new(t, rect.top()),
            Side::Down => Point::new(t, rect.bottom() - 1.0),
        };
        Some(rect.clamp(p))
    }

    fn machine(&self, machine: Machine) -> &MachineLayout {
        match machine {
            Machine::A => &self.a,
            Machine::B => &self.b,
        }
    }

    fn screen_index(&self, machine: Machine, pos: Point) -> Option<usize> {
        let screens = &self.machine(machine).screens;
        screens
            .iter()
            .position(|s| s.rect.contains(pos))
            .or_else(|| {
                screens
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| a.rect.distance(pos).total_cmp(&b.rect.distance(pos)))
                    .map(|(i, _)| i)
            })
    }
}

/// Tramos del lado `facing` de cada pantalla que no tocan otra pantalla local.
fn exposed_edge(screens: &[Screen], facing: Side) -> Vec<Segment> {
    let mut edge = Vec::new();
    for (i, s) in screens.iter().enumerate() {
        let mut free = vec![span(&s.rect, facing)];
        for (j, o) in screens.iter().enumerate() {
            if i == j {
                continue;
            }
            let (s, o) = (&s.rect, &o.rect);
            // `o` tapa el borde si contiene los puntos justo del otro lado.
            let covers = match facing {
                Side::Right => o.left() <= s.right() && s.right() < o.right(),
                Side::Left => o.left() < s.left() && s.left() <= o.right(),
                Side::Down => o.top() <= s.bottom() && s.bottom() < o.bottom(),
                Side::Up => o.top() < s.top() && s.top() <= o.bottom(),
            };
            if covers {
                free = subtract(&free, span(o, facing));
            }
        }
        edge.extend(
            free.into_iter()
                .map(|(t0, t1)| Segment { screen: i, t0, t1 }),
        );
    }
    let outer = |seg: &Segment| {
        let r = &screens[seg.screen].rect;
        match facing {
            Side::Right => -r.right(),
            Side::Left => r.left(),
            Side::Down => -r.bottom(),
            Side::Up => r.top(),
        }
    };
    edge.sort_by(|a, b| a.t0.total_cmp(&b.t0).then(outer(a).total_cmp(&outer(b))));
    edge
}

fn subtract(intervals: &[(f64, f64)], (c0, c1): (f64, f64)) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for &(a, b) in intervals {
        if c1 <= a || c0 >= b {
            out.push((a, b));
            continue;
        }
        if a < c0 {
            out.push((a, c0));
        }
        if c1 < b {
            out.push((c1, b));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn mon(id: &str, x: i32, y: i32, w: u32, h: u32, scale: f64) -> Screen {
        Screen::new(id.into(), Rect::new(x, y, w, h), scale)
    }

    fn laptop(x: i32, y: i32) -> Screen {
        Screen::new("laptop".into(), Rect::new(x, y, 1512, 982), 1.0)
    }

    fn arr(mac: Vec<Screen>, pc: Vec<Screen>, a_side: Side) -> Arrangement {
        Arrangement {
            a: mac,
            b: pc,
            a_side,
            align: None,
        }
    }

    fn no_margin() -> LayoutOptions {
        LayoutOptions { corner_margin: 0.0 }
    }

    /// El 34" (3440×1440) en la PC a la izquierda; el 32" (1920×1080) en la Mac a la derecha.
    fn shared(opts: LayoutOptions) -> Layout {
        let mac = vec![mon("m32", 3440, 0, 1920, 1080, 1.0)];
        let pc = vec![mon("m34", 0, 0, 3440, 1440, 1.0)];
        Layout::compute(&arr(mac, pc, Side::Right), opts).unwrap()
    }

    #[test]
    fn cada_maquina_mira_hacia_la_otra_segun_el_acomodo() {
        let l = shared(no_margin());
        assert_eq!(l.screens(Machine::B)[0].id, "m34".into());
        assert_eq!(l.screens(Machine::A)[0].id, "m32".into());
        assert_eq!(l.facing(Machine::B), Some(Side::Right));
        assert_eq!(l.facing(Machine::A), Some(Side::Left));

        // Sin acomodo declarado la Mac va a la izquierda.
        let l = Layout::compute(
            &Arrangement {
                a: vec![laptop(0, 0)],
                b: vec![mon("m34", 0, 0, 3440, 1440, 1.0)],
                ..Arrangement::default()
            },
            no_margin(),
        )
        .unwrap();
        assert_eq!(l.facing(Machine::A), Some(Side::Right));
        assert_eq!(l.facing(Machine::B), Some(Side::Left));
    }

    #[test]
    fn cruce_entre_monitores_de_distinto_tamano_es_proporcional() {
        let l = shared(no_margin());
        // Mitad del 34" (1440 de alto) → mitad del 32" de la Mac (1080 de alto).
        let m = l.step(Machine::B, Point::new(3439.0, 720.0), 5.0, 0.0);
        assert_eq!(
            m,
            Motion::Cross {
                to: Machine::A,
                pos: Point::new(3440.0, 540.0)
            }
        );
        // Un cuarto desde arriba.
        let m = l.step(Machine::B, Point::new(3439.0, 360.0), 1.0, 0.0);
        assert_eq!(
            m,
            Motion::Cross {
                to: Machine::A,
                pos: Point::new(3440.0, 270.0)
            }
        );
        // Y de vuelta: el borde izquierdo de la Mac lleva al derecho de la PC.
        let m = l.step(Machine::A, Point::new(3440.0, 270.0), -1.0, 0.0);
        assert_eq!(
            m,
            Motion::Cross {
                to: Machine::B,
                pos: Point::new(3439.0, 360.0)
            }
        );
    }

    #[test]
    fn el_borde_que_no_mira_a_la_otra_maquina_no_cruza() {
        let l = shared(no_margin());
        // Izquierda del 34" de la PC: no hay nada ahí.
        assert_eq!(
            l.step(Machine::B, Point::new(0.0, 500.0), -10.0, 0.0),
            Motion::Local(Point::new(0.0, 500.0))
        );
        // Derecha del 32" de la Mac.
        assert_eq!(
            l.step(Machine::A, Point::new(5359.0, 500.0), 10.0, 3.0),
            Motion::Local(Point::new(5359.0, 503.0))
        );
        // Un movimiento normal dentro de la pantalla.
        assert_eq!(
            l.step(Machine::B, Point::new(100.0, 100.0), 10.0, -5.0),
            Motion::Local(Point::new(110.0, 95.0))
        );
    }

    #[test]
    fn esquinas_muertas() {
        let l = shared(LayoutOptions {
            corner_margin: 20.0,
        });
        let at = |y| l.step(Machine::B, Point::new(3439.0, y), 4.0, 0.0);
        // Dentro del margen de arriba y de abajo el cursor se queda contra el borde.
        assert_eq!(at(0.0), Motion::Local(Point::new(3439.0, 0.0)));
        assert_eq!(at(19.0), Motion::Local(Point::new(3439.0, 19.0)));
        assert_eq!(at(1420.5), Motion::Local(Point::new(3439.0, 1420.5)));
        assert_eq!(at(1439.0), Motion::Local(Point::new(3439.0, 1439.0)));
        // Justo fuera del margen cruza.
        assert!(matches!(at(20.0), Motion::Cross { to: Machine::A, .. }));
        assert!(matches!(at(1419.0), Motion::Cross { to: Machine::A, .. }));
    }

    #[test]
    fn el_margen_de_las_esquinas_se_escala_por_dpi() {
        // La PC con el 34" y el 32" a 150 %; la Mac a la derecha con su pantalla.
        let pc = vec![
            mon("m34", 0, 0, 3440, 1440, 1.0),
            mon("m32", 3440, 0, 3840, 2160, 1.5),
        ];
        let l = Layout::compute(
            &arr(vec![laptop(0, 0)], pc, Side::Right),
            LayoutOptions {
                corner_margin: 20.0,
            },
        )
        .unwrap();
        let at = |y| l.step(Machine::B, Point::new(7279.0, y), 4.0, 0.0);
        // 20 puntos a 150 % son 30 píxeles.
        assert!(matches!(at(29.0), Motion::Local(_)));
        assert!(matches!(at(30.0), Motion::Cross { to: Machine::A, .. }));
        assert!(matches!(at(2130.0), Motion::Local(_)));
        assert!(matches!(at(2129.0), Motion::Cross { to: Machine::A, .. }));
        // Mitad del 32" de la PC (2160 de alto) → mitad de la pantalla de la Mac (982).
        let l = Layout::compute(
            &arr(
                vec![laptop(0, 0)],
                vec![
                    mon("m34", 0, 0, 3440, 1440, 1.0),
                    mon("m32", 3440, 0, 3840, 2160, 1.5),
                ],
                Side::Right,
            ),
            no_margin(),
        )
        .unwrap();
        assert_eq!(
            l.step(Machine::B, Point::new(7279.0, 1080.0), 2.0, 0.0),
            Motion::Cross {
                to: Machine::A,
                pos: Point::new(0.0, 491.0)
            }
        );
    }

    #[test]
    fn la_velocidad_se_escala_por_dpi() {
        let m34 = mon("m34", 0, 0, 3440, 1440, 1.0);
        let m32 = mon("m32", 3440, 0, 3840, 2160, 1.5);
        // 30 píxeles en el 32" a 150 % son 20 puntos; en el 34" a 100 %, 20 píxeles.
        let (lx, ly) = m32.to_logical(30.0, -15.0);
        assert_eq!((lx, ly), (20.0, -10.0));
        assert_eq!(m34.to_native(lx, ly), (20.0, -10.0));
        assert_eq!(m32.to_native(lx, ly), (30.0, -15.0));
    }

    #[test]
    fn cruce_en_diagonal_usa_la_altura_donde_toca_el_borde() {
        let l = shared(no_margin());
        // Desde x=3430 con dx=20 toca el borde a mitad del camino: y = 700 + 10.
        let m = l.step(Machine::B, Point::new(3430.0, 700.0), 20.0, 20.0);
        let Motion::Cross { to, pos } = m else {
            panic!("no cruzó: {m:?}");
        };
        assert_eq!(to, Machine::A);
        assert!((pos.y - 710.0 * 1080.0 / 1440.0).abs() < 1e-9);
    }

    #[test]
    fn sin_pantallas_de_la_otra_no_hay_cruce_y_el_lado_sale_del_acomodo() {
        let mac = vec![
            mon("m34", 0, 0, 3440, 1440, 1.0),
            mon("m32", 3440, 0, 1920, 1080, 1.0),
        ];
        let pc = vec![mon("pc", 0, 0, 2560, 1440, 1.0)];
        // La PC apagada (o sin pantallas todavía): no hay a dónde cruzar.
        let l = Layout::compute(&arr(mac.clone(), vec![], Side::Left), no_margin()).unwrap();
        assert_eq!(l.facing(Machine::A), None);
        assert_eq!(
            l.step(Machine::A, Point::new(5359.0, 10.0), 50.0, 0.0),
            Motion::Local(Point::new(5359.0, 10.0))
        );
        // La Mac a la izquierda: su borde derecho lleva a la PC.
        let l = Layout::compute(&arr(mac.clone(), pc.clone(), Side::Left), no_margin()).unwrap();
        assert_eq!(l.facing(Machine::A), Some(Side::Right));
        assert!(matches!(
            l.step(Machine::A, Point::new(5359.0, 500.0), 3.0, 0.0),
            Motion::Cross { to: Machine::B, .. }
        ));
        // A la derecha: el izquierdo.
        let l = Layout::compute(&arr(mac, pc, Side::Right), no_margin()).unwrap();
        assert_eq!(l.facing(Machine::A), Some(Side::Left));
        assert!(matches!(
            l.step(Machine::A, Point::new(0.0, 500.0), -3.0, 0.0),
            Motion::Cross { to: Machine::B, .. }
        ));
    }

    #[test]
    fn dos_pantallas_apiladas_suman_su_borde() {
        // La Mac tiene el 34" y su pantalla debajo, alineada a la derecha.
        let mac = vec![
            mon("m34", 0, 0, 3440, 1440, 1.0),
            Screen::new("laptop".into(), Rect::new(1928, 1440, 1512, 982), 1.0),
        ];
        let pc = vec![mon("m32", 0, 0, 2560, 1440, 1.0)];
        let l = Layout::compute(&arr(mac, pc, Side::Left), no_margin()).unwrap();
        // El borde derecho de la Mac mide 1440 + 982; la de abajo ocupa la parte de abajo.
        let m = l.step(Machine::A, Point::new(3439.0, 1440.0 + 491.0), 1.0, 0.0);
        let Motion::Cross { pos, .. } = m else {
            panic!("no cruzó: {m:?}");
        };
        let expected = (1440.0 + 491.0) / (1440.0 + 982.0) * 1440.0;
        assert!((pos.y - expected).abs() < 1e-9);
        assert_eq!(pos.x, 0.0);
    }

    #[test]
    fn un_borde_tapado_por_otra_pantalla_local_no_cruza() {
        let mac = vec![
            mon("m34", 0, 0, 3440, 1440, 1.0),
            mon("m32", 3440, 0, 1920, 1080, 1.0),
        ];
        // Con las dos pantallas en la Mac, la unión entre ellas no es un borde.
        let edge = exposed_edge(&mac, Side::Right);
        assert_eq!(
            edge,
            vec![
                Segment {
                    screen: 1,
                    t0: 0.0,
                    t1: 1080.0
                },
                Segment {
                    screen: 0,
                    t0: 1080.0,
                    t1: 1440.0
                },
            ]
        );
    }

    #[test]
    fn pasa_a_otra_pantalla_local_sin_cruzar() {
        let pc = vec![
            mon("m34", 0, 0, 3440, 1440, 1.0),
            mon("m32", 3440, 0, 3840, 2160, 1.5),
        ];
        let l = Layout::compute(&arr(vec![laptop(0, 0)], pc, Side::Right), no_margin()).unwrap();
        assert_eq!(
            l.step(Machine::B, Point::new(3439.0, 500.0), 2.0, 0.0),
            Motion::Local(Point::new(3441.0, 500.0))
        );
    }

    #[test]
    fn rechaza_pantallas_invalidas() {
        let bad = arr(vec![mon("m34", 0, 0, 0, 1440, 1.0)], vec![], Side::Left);
        assert!(matches!(
            Layout::compute(&bad, LayoutOptions::default()),
            Err(GeometryError::EmptyScreen(_))
        ));
        let bad = arr(vec![], vec![mon("m34", 0, 0, 10, 10, 0.0)], Side::Left);
        assert!(matches!(
            Layout::compute(&bad, LayoutOptions::default()),
            Err(GeometryError::InvalidScale { .. })
        ));
        let opts = LayoutOptions {
            corner_margin: f64::NAN,
        };
        assert!(matches!(
            Layout::compute(&Arrangement::default(), opts),
            Err(GeometryError::InvalidMargin(_))
        ));
    }

    /// Un punto del borde `side` de `r` a `f` de su largo, y el paso que lo saca por ahí.
    fn on_edge(r: Rect, side: Side, f: f64) -> (Point, f64, f64) {
        let along = |lo: f64, len: u32| {
            (lo + f * f64::from(len))
                .floor()
                .min(lo + f64::from(len) - 1.0)
        };
        match side {
            Side::Left => (Point::new(r.left(), along(r.top(), r.height)), -1.0, 0.0),
            Side::Right => (
                Point::new(r.right() - 1.0, along(r.top(), r.height)),
                1.0,
                0.0,
            ),
            Side::Up => (Point::new(along(r.left(), r.width), r.top()), 0.0, -1.0),
            Side::Down => (
                Point::new(along(r.left(), r.width), r.bottom() - 1.0),
                0.0,
                1.0,
            ),
        }
    }

    /// DESIGN §7 / D18: la otra máquina en cada uno de los 4 lados con cada una de las 3
    /// alineaciones. B tiene el 34" (3440×1440) y A una pantalla más chica (1920×1080):
    /// el borde de cruce es el tramo que comparten, y sale a la misma distancia.
    #[test]
    fn cuatro_lados_por_tres_alineaciones_dan_el_borde_compartido() {
        let a_rect = Rect::new(0, 0, 1920, 1080);
        let b_rect = Rect::new(0, 0, 3440, 1440);
        for a_side in Side::ALL {
            for align in Align::ALL {
                let l = Layout::compute(
                    &Arrangement {
                        a: vec![Screen::new("a".into(), a_rect, 1.0)],
                        b: vec![Screen::new("b".into(), b_rect, 1.0)],
                        a_side,
                        align: Some(align),
                    },
                    no_margin(),
                )
                .unwrap();
                let case = format!("{a_side:?} {align:?}");
                assert_eq!(l.facing(Machine::B), Some(a_side), "{case}");
                assert_eq!(l.facing(Machine::A), Some(a_side.opposite()), "{case}");
                // Largo de cada borde y dónde empieza A sobre el de B.
                let (len_a, len_b) = if a_side.is_vertical_edge() {
                    (1080.0, 1440.0)
                } else {
                    (1920.0, 3440.0)
                };
                let start = (len_b - len_a) * align.factor();
                assert_eq!(
                    l.crossing_span(Machine::B),
                    Some((start, start + len_a)),
                    "{case}"
                );
                assert_eq!(l.crossing_span(Machine::A), Some((0.0, len_a)), "{case}");

                let at = |u: f64| {
                    let f = u / len_b;
                    let (p, dx, dy) = on_edge(b_rect, a_side, f);
                    l.step(Machine::B, p, dx, dy)
                };
                let land = |along: f64| {
                    let (p, _, _) = on_edge(a_rect, a_side.opposite(), along / len_a);
                    p
                };
                // Dentro del tramo cruza a la misma distancia de su inicio.
                for d in [0.0, 1.0, len_a / 2.0, len_a - 1.0] {
                    assert_eq!(
                        at(start + d),
                        Motion::Cross {
                            to: Machine::A,
                            pos: land(d)
                        },
                        "{case} a {d}"
                    );
                }
                // Fuera del tramo, el cursor se queda contra el borde.
                if start >= 1.0 {
                    assert!(matches!(at(start - 1.0), Motion::Local(_)), "{case}");
                }
                if start + len_a < len_b {
                    assert!(matches!(at(start + len_a), Motion::Local(_)), "{case}");
                }
                // Y de vuelta: de cualquier punto de A se llega al tramo de B.
                let (p, dx, dy) = on_edge(a_rect, a_side.opposite(), 0.5);
                let Motion::Cross { to, pos } = l.step(Machine::A, p, dx, dy) else {
                    panic!("{case}: A no cruzó");
                };
                assert_eq!(to, Machine::B);
                let u = if a_side.is_vertical_edge() {
                    pos.y
                } else {
                    pos.x
                };
                assert_eq!(u, start + len_a / 2.0, "{case}");
            }
        }
    }

    #[test]
    fn con_alineacion_el_tramo_se_mide_en_puntos_logicos() {
        // B con un 4K a 150 % (2560×1440 puntos) a la derecha de A (1512×982 a 100 %),
        // alineados al centro: A empieza a (1440 − 982) / 2 = 229 puntos del borde de B.
        let l = Layout::compute(
            &Arrangement {
                a: vec![laptop(0, 0)],
                b: vec![mon("4k", 0, 0, 3840, 2160, 1.5)],
                a_side: Side::Right,
                align: Some(Align::Center),
            },
            no_margin(),
        )
        .unwrap();
        assert_eq!(l.crossing_span(Machine::B), Some((229.0, 1211.0)));
        // 229 puntos son 343.5 píxeles: justo antes no cruza; justo después sí, arriba.
        assert!(matches!(
            l.step(Machine::B, Point::new(3839.0, 343.0), 1.0, 0.0),
            Motion::Local(_)
        ));
        let Motion::Cross { to, pos } = l.step(Machine::B, Point::new(3839.0, 345.0), 1.0, 0.0)
        else {
            panic!("no cruzó dentro del tramo");
        };
        assert_eq!((to, pos.x), (Machine::A, 0.0));
        assert!((pos.y - 1.0).abs() < 1e-9, "{pos:?}");
        assert_eq!(shared_span(100.0, 300.0, Align::End), (0.0, 100.0));
        assert_eq!(shared_span(300.0, 100.0, Align::End), (200.0, 300.0));
        assert_eq!(shared_span(300.0, 100.0, Align::Start), (0.0, 100.0));
    }

    #[test]
    fn sin_pantallas_el_cursor_no_se_mueve() {
        let l = Layout::compute(&Arrangement::default(), LayoutOptions::default()).unwrap();
        let p = Point::new(10.0, 10.0);
        assert_eq!(l.step(Machine::B, p, 5.0, 5.0), Motion::Local(p));
        assert!(l.screen_at(Machine::B, p).is_none());
    }

    fn arb_screen(id: &'static str) -> impl Strategy<Value = Screen> {
        (
            -4000i32..4000,
            -2000i32..2000,
            200u32..5000,
            200u32..3000,
            prop::sample::select(vec![1.0, 1.25, 1.5, 2.0]),
        )
            .prop_map(move |(x, y, w, h, scale)| mon(id, x, y, w, h, scale))
    }

    fn side() -> impl Strategy<Value = Side> {
        prop::sample::select(Side::ALL.to_vec())
    }

    fn align() -> impl Strategy<Value = Option<Align>> {
        prop::option::of(prop::sample::select(Align::ALL.to_vec()))
    }

    /// Una pantalla por máquina, en cualquier lugar, la Mac de cualquier lado, con o sin
    /// alineación.
    fn arb_layout() -> impl Strategy<Value = Layout> {
        (
            arb_screen("a"),
            arb_screen("b"),
            side(),
            align(),
            0.0f64..40.0,
        )
            .prop_map(|(a, b, a_side, align, margin)| {
                let opts = LayoutOptions {
                    corner_margin: margin,
                };
                let arr = Arrangement {
                    align,
                    ..arr(vec![a], vec![b], a_side)
                };
                Layout::compute(&arr, opts).unwrap()
            })
    }

    fn inside(l: &Layout, m: Machine, p: Point) -> bool {
        l.screens(m).iter().any(|s| s.rect.contains(p))
    }

    /// Varias pantallas por máquina, en cualquier posición (pueden solaparse o dejar
    /// huecos); a veces la PC sin ninguna.
    fn arb_multi_layout() -> impl Strategy<Value = Layout> {
        (
            proptest::collection::vec(arb_screen("mac"), 1..4),
            proptest::collection::vec(arb_screen("pc"), 0..4),
            side(),
            align(),
            0.0f64..40.0,
        )
            .prop_map(|(mac, pc, a_side, align, margin)| {
                let opts = LayoutOptions {
                    corner_margin: margin,
                };
                let arr = Arrangement {
                    align,
                    ..arr(mac, pc, a_side)
                };
                Layout::compute(&arr, opts).unwrap()
            })
    }

    // QA ronda 1: las propiedades de abajo solo usan una pantalla por máquina.
    proptest! {
        /// Con varias pantallas, solapadas o con huecos, el cursor siempre termina
        /// dentro de una pantalla, y nunca en la máquina que no tiene ninguna.
        #[test]
        fn varias_pantallas_el_cursor_siempre_queda_en_una_pantalla(
            l in arb_multi_layout(),
            mac in any::<bool>(),
            pick in any::<prop::sample::Index>(),
            fx in 0.0f64..1.0,
            fy in 0.0f64..1.0,
            dx in -3000.0f64..3000.0,
            dy in -3000.0f64..3000.0,
        ) {
            // La Mac siempre tiene algo; la PC no siempre.
            let m = if mac || l.screens(Machine::B).is_empty() {
                Machine::A
            } else {
                Machine::B
            };
            let screens = l.screens(m);
            let r = pick.get(screens).rect;
            let p = r.clamp(Point::new(
                r.left() + fx * f64::from(r.width),
                r.top() + fy * f64::from(r.height),
            ));
            match l.step(m, p, dx, dy) {
                Motion::Local(q) => prop_assert!(inside(&l, m, q), "{q:?}"),
                Motion::Cross { to, pos } => {
                    prop_assert_eq!(to, m.other());
                    prop_assert!(l.facing(m).is_some());
                    prop_assert!(inside(&l, to, pos), "{pos:?}");
                    // Se entra por el lado que mira a la máquina de origen, en el
                    // borde de alguna pantalla que contiene el punto.
                    let facing = l.facing(to).unwrap();
                    let on_edge = l.screens(to).iter().any(|s| {
                        s.rect.contains(pos)
                            && match facing {
                                Side::Left => pos.x == s.rect.left(),
                                Side::Right => pos.x == s.rect.right() - 1.0,
                                Side::Up => pos.y == s.rect.top(),
                                Side::Down => pos.y == s.rect.bottom() - 1.0,
                            }
                    });
                    prop_assert!(on_edge, "{pos:?} no está en el borde {facing:?}");
                }
            }
        }

        /// Los dos lados siempre se miran: si una máquina puede cruzar, la otra
        /// también, por el lado opuesto.
        #[test]
        fn los_lados_siempre_son_opuestos(l in arb_multi_layout()) {
            match (l.facing(Machine::A), l.facing(Machine::B)) {
                (Some(a), Some(b)) => prop_assert_eq!(a, b.opposite()),
                (None, None) => {}
                other => prop_assert!(false, "lados inconsistentes: {other:?}"),
            }
        }

        /// Un movimiento nulo nunca cruza ni mueve el cursor que está dentro.
        #[test]
        fn sin_movimiento_no_hay_cruce(
            l in arb_multi_layout(),
            mac in any::<bool>(),
            pick in any::<prop::sample::Index>(),
            fy in 0.0f64..1.0,
        ) {
            let m = if mac || l.screens(Machine::B).is_empty() {
                Machine::A
            } else {
                Machine::B
            };
            let screens = l.screens(m);
            let r = pick.get(screens).rect;
            // En el borde derecho, donde más fácil sería cruzar por error.
            let p = Point::new(r.right() - 1.0, (r.top() + fy * f64::from(r.height)).floor());
            prop_assert_eq!(l.step(m, p, 0.0, 0.0), Motion::Local(p));
        }
    }

    proptest! {
        /// Pase lo que pase, el cursor termina dentro de una pantalla.
        #[test]
        fn el_cursor_siempre_queda_en_una_pantalla(
            l in arb_layout(),
            mac in any::<bool>(),
            fx in 0.0f64..1.0,
            fy in 0.0f64..1.0,
            dx in -500.0f64..500.0,
            dy in -500.0f64..500.0,
        ) {
            let m = if mac { Machine::A } else { Machine::B };
            let r = l.screens(m)[0].rect;
            let p = r.clamp(Point::new(
                r.left() + fx * f64::from(r.width),
                r.top() + fy * f64::from(r.height),
            ));
            match l.step(m, p, dx, dy) {
                Motion::Local(q) => prop_assert!(inside(&l, m, q), "{q:?}"),
                Motion::Cross { to, pos } => {
                    prop_assert_eq!(to, m.other());
                    prop_assert!(inside(&l, to, pos), "{pos:?}");
                }
            }
        }

        /// Cruzar y volver enseguida deja el cursor a menos de un píxel de la posición
        /// original a lo largo del borde, por cualquiera de los cuatro lados.
        #[test]
        fn ida_y_vuelta_conserva_la_posicion(l in arb_layout(), f in 0.0f64..1.0) {
            let side = l.facing(Machine::B).unwrap();
            let b = &l.screens(Machine::B)[0];
            let a = &l.screens(Machine::A)[0];
            let (p, dx, dy) = on_edge(b.rect, side, f);
            if let Motion::Cross { pos, .. } = l.step(Machine::B, p, dx, dy) {
                match l.step(Machine::A, pos, -dx, -dy) {
                    Motion::Cross { to, pos: back } => {
                        prop_assert_eq!(to, Machine::B);
                        let (len_b, len_a) = if side.is_vertical_edge() {
                            (f64::from(b.rect.height), f64::from(a.rect.height))
                        } else {
                            (f64::from(b.rect.width), f64::from(a.rect.width))
                        };
                        // La vuelta puede caer en la esquina muerta de la Mac; solo
                        // se mide cuando vuelve.
                        let tol = 1.0 + (len_b / len_a).max(b.scale / a.scale);
                        if side.is_vertical_edge() {
                            prop_assert_eq!(back.x, p.x);
                            prop_assert!((back.y - p.y).abs() <= tol, "{} vs {}", back.y, p.y);
                        } else {
                            prop_assert_eq!(back.y, p.y);
                            prop_assert!((back.x - p.x).abs() <= tol, "{} vs {}", back.x, p.x);
                        }
                    }
                    Motion::Local(_) => {}
                }
            }
        }

        /// Con alineación, solo se cruza dentro del borde compartido, y se sale a la misma
        /// distancia (en puntos lógicos) de donde empieza el otro escritorio.
        #[test]
        fn con_alineacion_solo_cruza_el_tramo_compartido(
            l in arb_layout(),
            f in 0.0f64..1.0,
        ) {
            let side = l.facing(Machine::B).unwrap();
            let b = &l.screens(Machine::B)[0];
            let (p, dx, dy) = on_edge(b.rect, side, f);
            let u = if side.is_vertical_edge() { p.y - b.rect.top() } else { p.x - b.rect.left() };
            let u = u / b.scale;
            let span = l.crossing_span(Machine::B);
            match l.step(Machine::B, p, dx, dy) {
                Motion::Cross { .. } => {
                    let (s, e) = span.unwrap();
                    prop_assert!(u >= s - 1e-9 && u < e + 1e-9, "{u} fuera de {s}..{e}");
                }
                Motion::Local(_) => {}
            }
        }
    }
}
