//! Geometría de pantallas y cruce de bordes entre las dos máquinas.
//!
//! Cada máquina reporta sus pantallas en sus coordenadas nativas del cursor
//! (puntos en macOS, píxeles físicos en Windows Per-Monitor-V2). [`Layout`] se
//! queda solo con las que el [`State`] dice que se están mirando de verdad
//! (DESIGN §7 y §8.1), así el cursor nunca entra en un monitor fantasma, y pone
//! las dos máquinas una al lado de la otra según el orden de los monitores en el
//! escritorio. Se recalcula entero con cada cambio de estado: es un valor, no hay
//! nada que invalidar.
//!
//! El cruce se prueba contra el borde de la pantalla donde está el cursor, nunca
//! contra la caja que encierra a todas (docs/research/input-engine.md), y la
//! posición se lleva como fracción a lo largo del borde compartido, para que
//! monitores de distinto tamaño se sientan continuos.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use tandem_core::{BuiltinDisplay, Config, Machine, MonitorId, State};

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

/// Qué es una pantalla para Tandem.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenId {
    /// Un monitor externo de la configuración.
    Monitor(MonitorId),
    /// La pantalla integrada de la MacBook.
    Builtin,
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

/// Lado del escritorio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn opposite(self) -> Self {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayoutOptions {
    /// Lado de la PC en que queda la Mac cuando solo tiene su pantalla integrada:
    /// la laptop no tiene lugar en el orden de los monitores.
    pub builtin_side: Side,
    /// Margen de las esquinas en puntos lógicos: ahí no se cruza, para no
    /// pelear con las esquinas activas de macOS ni con los botones de las ventanas.
    pub corner_margin: f64,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            builtin_side: Side::Right,
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

/// Tramo del borde que mira a la otra máquina y por el que se puede cruzar.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Segment {
    screen: usize,
    y0: f64,
    y1: f64,
}

impl Segment {
    fn len(&self) -> f64 {
        self.y1 - self.y0
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Facing {
    facing: Side,
    /// Ordenados de arriba abajo; la fracción del cruce se mide sobre su concatenación.
    edge: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq)]
struct MachineLayout {
    screens: Vec<Screen>,
    /// `None` si la otra máquina no es visible: no hay a dónde cruzar.
    side: Option<Facing>,
}

/// Resultado de mover el cursor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Motion {
    /// El cursor sigue en la misma máquina, en esta posición.
    Local(Point),
    /// El cursor sale por el borde y entra a la otra máquina en esta posición.
    Cross { to: Machine, pos: Point },
}

/// Pantallas visibles de las dos máquinas y cómo se tocan.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    opts: LayoutOptions,
    mac: MachineLayout,
    pc: MachineLayout,
}

impl Layout {
    /// Arma el layout a partir de lo que reporta cada máquina y del estado.
    ///
    /// Se descartan las pantallas que no se están mirando: monitores que el estado
    /// asigna a la otra máquina (fantasmas que el sistema no pudo quitar), monitores
    /// que no están en la configuración, la pantalla integrada si no está extendida
    /// y todas las de una máquina que no es visible.
    pub fn compute(
        cfg: &Config,
        state: &State,
        mac: &[Screen],
        pc: &[Screen],
        opts: LayoutOptions,
    ) -> Result<Self, GeometryError> {
        if !opts.corner_margin.is_finite() || opts.corner_margin < 0.0 {
            return Err(GeometryError::InvalidMargin(opts.corner_margin));
        }
        for s in mac.iter().chain(pc) {
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

        let mac_screens = visible_screens(cfg, state, Machine::Mac, mac);
        let pc_screens = visible_screens(cfg, state, Machine::Pc, pc);

        let mac_facing = if mac_screens.is_empty() || pc_screens.is_empty() {
            None
        } else {
            Some(mac_facing(
                cfg,
                &mac_screens,
                &pc_screens,
                opts.builtin_side,
            ))
        };

        let build = |screens: Vec<Screen>, facing: Option<Side>| {
            let side = facing.map(|facing| Facing {
                facing,
                edge: exposed_edge(&screens, facing),
            });
            MachineLayout { screens, side }
        };

        Ok(Self {
            opts,
            mac: build(mac_screens, mac_facing),
            pc: build(pc_screens, mac_facing.map(Side::opposite)),
        })
    }

    pub fn options(&self) -> LayoutOptions {
        self.opts
    }

    /// Pantallas visibles de una máquina.
    pub fn screens(&self, machine: Machine) -> &[Screen] {
        &self.machine(machine).screens
    }

    /// Lado de la máquina por el que se pasa a la otra; `None` si no se puede cruzar.
    pub fn facing(&self, machine: Machine) -> Option<Side> {
        self.machine(machine).side.as_ref().map(|s| s.facing)
    }

    /// Pantalla que contiene el punto o, si cae fuera de todas, la más cercana.
    pub fn screen_at(&self, machine: Machine, pos: Point) -> Option<&Screen> {
        self.screen_index(machine, pos)
            .map(|i| &self.machine(machine).screens[i])
    }

    /// Mueve el cursor de `machine` desde `pos` con un delta nativo.
    ///
    /// Si el movimiento sale por el borde que mira a la otra máquina, fuera del
    /// margen de las esquinas, devuelve dónde aparece en la otra. Si no, el cursor
    /// pasa a otra pantalla local o se queda contra el borde de la actual, como hace
    /// el sistema. Sin pantallas visibles devuelve `pos` tal cual.
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
        let edge_x = match side.facing {
            Side::Right if dx > 0.0 && pos.x + dx >= rect.right() => rect.right(),
            Side::Left if dx < 0.0 && pos.x + dx < rect.left() => rect.left(),
            _ => return None,
        };
        // Altura en la que la trayectoria toca el borde.
        let t = ((edge_x - pos.x) / dx).clamp(0.0, 1.0);
        let y = pos.y + dy * t;

        let margin = self.opts.corner_margin * ml.screens[cur].scale;
        if y < rect.top() + margin || y >= rect.bottom() - margin {
            return None;
        }
        let total: f64 = side.edge.iter().map(Segment::len).sum();
        let mut before = 0.0;
        let mut offset = None;
        for seg in &side.edge {
            if seg.screen == cur && y >= seg.y0 && y < seg.y1 {
                offset = Some(before + (y - seg.y0));
                break;
            }
            before += seg.len();
        }
        let fraction = offset? / total;

        let to = machine.other();
        let pos = self.land(to, fraction)?;
        Some(Motion::Cross { to, pos })
    }

    /// Punto del borde de `machine` que está a `fraction` de su largo.
    fn land(&self, machine: Machine, fraction: f64) -> Option<Point> {
        let ml = self.machine(machine);
        let side = ml.side.as_ref()?;
        let total: f64 = side.edge.iter().map(Segment::len).sum();
        let mut left = fraction * total;
        let last = side.edge.last()?;
        let mut hit = (last, last.len());
        for seg in &side.edge {
            if left < seg.len() {
                hit = (seg, left);
                break;
            }
            left -= seg.len();
        }
        let (seg, offset) = hit;
        let rect = ml.screens[seg.screen].rect;
        let x = match side.facing {
            Side::Left => rect.left(),
            Side::Right => rect.right() - 1.0,
        };
        let y = (seg.y0 + offset).min(seg.y1 - 1.0);
        Some(rect.clamp(Point::new(x, y)))
    }

    fn machine(&self, machine: Machine) -> &MachineLayout {
        match machine {
            Machine::Mac => &self.mac,
            Machine::Pc => &self.pc,
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

fn visible_screens(
    cfg: &Config,
    state: &State,
    machine: Machine,
    reported: &[Screen],
) -> Vec<Screen> {
    if !state.is_visible(cfg, machine) {
        return Vec::new();
    }
    reported
        .iter()
        .filter(|s| match &s.id {
            ScreenId::Monitor(id) => {
                cfg.monitor(id).is_some() && state.owner.get(id) == Some(&machine)
            }
            // En espejo la integrada no agrega espacio al escritorio.
            ScreenId::Builtin => {
                machine == Machine::Mac && state.builtin_effective(cfg) == BuiltinDisplay::Extend
            }
        })
        .cloned()
        .collect()
}

/// Lado por el que la Mac mira a la PC: el orden de los monitores en el escritorio
/// dice cuál máquina está a la izquierda.
fn mac_facing(cfg: &Config, mac: &[Screen], pc: &[Screen], builtin_side: Side) -> Side {
    let first_desk_pos = |screens: &[Screen]| {
        screens
            .iter()
            .filter_map(|s| match &s.id {
                ScreenId::Monitor(id) => cfg.monitors.iter().position(|m| &m.id == id),
                ScreenId::Builtin => None,
            })
            .min()
    };
    let mac_side = match (first_desk_pos(mac), first_desk_pos(pc)) {
        (Some(m), Some(p)) if m < p => Side::Left,
        (Some(_), Some(_)) => Side::Right,
        // La Mac solo tiene la integrada.
        _ => builtin_side,
    };
    mac_side.opposite()
}

/// Tramos del lado `facing` de cada pantalla que no tocan otra pantalla local.
fn exposed_edge(screens: &[Screen], facing: Side) -> Vec<Segment> {
    let mut edge = Vec::new();
    for (i, s) in screens.iter().enumerate() {
        let mut free = vec![(s.rect.top(), s.rect.bottom())];
        for (j, o) in screens.iter().enumerate() {
            if i == j {
                continue;
            }
            // `o` tapa el borde si contiene los puntos justo del otro lado.
            let covers = match facing {
                Side::Right => o.rect.left() <= s.rect.right() && s.rect.right() < o.rect.right(),
                Side::Left => o.rect.left() < s.rect.left() && s.rect.left() <= o.rect.right(),
            };
            if covers {
                free = subtract(&free, (o.rect.top(), o.rect.bottom()));
            }
        }
        edge.extend(
            free.into_iter()
                .map(|(y0, y1)| Segment { screen: i, y0, y1 }),
        );
    }
    let outer = |seg: &Segment| match facing {
        Side::Right => -screens[seg.screen].rect.right(),
        Side::Left => screens[seg.screen].rect.left(),
    };
    edge.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(outer(a).total_cmp(&outer(b))));
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

    /// El puesto real: 34" a la izquierda, 32" a la derecha.
    fn desk() -> Config {
        Config::from_toml(
            r#"
            [[monitors]]
            id = "m34"
            label = '34"'
            wiring = { pc = "displayport", mac = "hdmi" }

            [[monitors]]
            id = "m32"
            label = '32"'
            wiring = { pc = "displayport", mac = "hdmi" }
            "#,
        )
        .unwrap()
    }

    fn mon(id: &str, x: i32, y: i32, w: u32, h: u32, scale: f64) -> Screen {
        Screen::new(ScreenId::Monitor(id.into()), Rect::new(x, y, w, h), scale)
    }

    fn builtin(x: i32, y: i32) -> Screen {
        Screen::new(ScreenId::Builtin, Rect::new(x, y, 1512, 982), 1.0)
    }

    fn state(m34: Machine, m32: Machine, pc_on: bool, pref: BuiltinDisplay) -> State {
        let cfg = desk();
        let mut s = State::initial(&cfg, Machine::Mac);
        s.owner.insert("m34".into(), m34);
        s.owner.insert("m32".into(), m32);
        s.on.pc = pc_on;
        s.builtin_pref = pref;
        s
    }

    fn no_margin() -> LayoutOptions {
        LayoutOptions {
            corner_margin: 0.0,
            ..LayoutOptions::default()
        }
    }

    /// Lo que reporta cada máquina si tuviera los dos monitores puestos; el estado
    /// decide cuáles cuentan. La PC tiene el 34" a 1440p y el 32" a 4K con 150 %.
    fn reported() -> (Vec<Screen>, Vec<Screen>) {
        let mac = vec![
            mon("m34", 0, 0, 3440, 1440, 1.0),
            mon("m32", 3440, 0, 1920, 1080, 1.0),
            builtin(0, 1440),
        ];
        let pc = vec![
            mon("m34", 0, 0, 3440, 1440, 1.0),
            mon("m32", 3440, 0, 3840, 2160, 1.5),
        ];
        (mac, pc)
    }

    /// Compartido: 34" en la PC (izquierda), 32" en la Mac (derecha).
    fn shared(opts: LayoutOptions) -> Layout {
        let (mac, pc) = reported();
        let st = state(Machine::Pc, Machine::Mac, true, BuiltinDisplay::Off);
        Layout::compute(&desk(), &st, &mac, &pc, opts).unwrap()
    }

    #[test]
    fn compartido_solo_cuenta_las_pantallas_que_se_miran() {
        let l = shared(no_margin());
        let ids = |m| {
            l.screens(m)
                .iter()
                .map(|s| s.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(Machine::Pc), vec![ScreenId::Monitor("m34".into())]);
        assert_eq!(ids(Machine::Mac), vec![ScreenId::Monitor("m32".into())]);
        assert_eq!(l.facing(Machine::Pc), Some(Side::Right));
        assert_eq!(l.facing(Machine::Mac), Some(Side::Left));
    }

    #[test]
    fn cruce_entre_monitores_de_distinto_tamano_es_proporcional() {
        let l = shared(no_margin());
        // Mitad del 34" (1440 de alto) → mitad del 32" de la Mac (1080 de alto).
        let m = l.step(Machine::Pc, Point::new(3439.0, 720.0), 5.0, 0.0);
        assert_eq!(
            m,
            Motion::Cross {
                to: Machine::Mac,
                pos: Point::new(3440.0, 540.0)
            }
        );
        // Un cuarto desde arriba.
        let m = l.step(Machine::Pc, Point::new(3439.0, 360.0), 1.0, 0.0);
        assert_eq!(
            m,
            Motion::Cross {
                to: Machine::Mac,
                pos: Point::new(3440.0, 270.0)
            }
        );
        // Y de vuelta: el borde izquierdo de la Mac lleva al derecho de la PC.
        let m = l.step(Machine::Mac, Point::new(3440.0, 270.0), -1.0, 0.0);
        assert_eq!(
            m,
            Motion::Cross {
                to: Machine::Pc,
                pos: Point::new(3439.0, 360.0)
            }
        );
    }

    #[test]
    fn el_borde_que_no_mira_a_la_otra_maquina_no_cruza() {
        let l = shared(no_margin());
        // Izquierda del 34" de la PC: no hay nada ahí.
        assert_eq!(
            l.step(Machine::Pc, Point::new(0.0, 500.0), -10.0, 0.0),
            Motion::Local(Point::new(0.0, 500.0))
        );
        // Derecha del 32" de la Mac.
        assert_eq!(
            l.step(Machine::Mac, Point::new(5359.0, 500.0), 10.0, 3.0),
            Motion::Local(Point::new(5359.0, 503.0))
        );
        // Un movimiento normal dentro de la pantalla.
        assert_eq!(
            l.step(Machine::Pc, Point::new(100.0, 100.0), 10.0, -5.0),
            Motion::Local(Point::new(110.0, 95.0))
        );
    }

    #[test]
    fn esquinas_muertas() {
        let l = shared(LayoutOptions {
            corner_margin: 20.0,
            ..LayoutOptions::default()
        });
        let at = |y| l.step(Machine::Pc, Point::new(3439.0, y), 4.0, 0.0);
        // Dentro del margen de arriba y de abajo el cursor se queda contra el borde.
        assert_eq!(at(0.0), Motion::Local(Point::new(3439.0, 0.0)));
        assert_eq!(at(19.0), Motion::Local(Point::new(3439.0, 19.0)));
        assert_eq!(at(1420.5), Motion::Local(Point::new(3439.0, 1420.5)));
        assert_eq!(at(1439.0), Motion::Local(Point::new(3439.0, 1439.0)));
        // Justo fuera del margen cruza.
        assert!(matches!(
            at(20.0),
            Motion::Cross {
                to: Machine::Mac,
                ..
            }
        ));
        assert!(matches!(
            at(1419.0),
            Motion::Cross {
                to: Machine::Mac,
                ..
            }
        ));
    }

    #[test]
    fn el_margen_de_las_esquinas_se_escala_por_dpi() {
        // Solo Windows salvo el 32", que la PC muestra a 150 %: la Mac a la derecha
        // con su integrada, la PC con los dos monitores.
        let (mac, pc) = reported();
        let st = state(Machine::Pc, Machine::Pc, true, BuiltinDisplay::Off);
        let l = Layout::compute(
            &desk(),
            &st,
            &mac,
            &pc,
            LayoutOptions {
                corner_margin: 20.0,
                builtin_side: Side::Right,
            },
        )
        .unwrap();
        let at = |y| l.step(Machine::Pc, Point::new(7279.0, y), 4.0, 0.0);
        // 20 puntos a 150 % son 30 píxeles.
        assert!(matches!(at(29.0), Motion::Local(_)));
        assert!(matches!(
            at(30.0),
            Motion::Cross {
                to: Machine::Mac,
                ..
            }
        ));
        assert!(matches!(at(2130.0), Motion::Local(_)));
        assert!(matches!(
            at(2129.0),
            Motion::Cross {
                to: Machine::Mac,
                ..
            }
        ));
    }

    #[test]
    fn la_velocidad_se_escala_por_dpi() {
        let (_, pc) = reported();
        let (m34, m32) = (&pc[0], &pc[1]);
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
        let m = l.step(Machine::Pc, Point::new(3430.0, 700.0), 20.0, 20.0);
        let Motion::Cross { to, pos } = m else {
            panic!("no cruzó: {m:?}");
        };
        assert_eq!(to, Machine::Mac);
        assert!((pos.y - 710.0 * 1080.0 / 1440.0).abs() < 1e-9);
    }

    #[test]
    fn layout_recalculado_cuando_cambia_el_estado() {
        let cfg = desk();
        let (mac, pc) = reported();
        let opts = no_margin();

        // Solo Mac con la PC apagada: no hay a dónde cruzar.
        let st = state(Machine::Mac, Machine::Mac, false, BuiltinDisplay::Off);
        let l = Layout::compute(&cfg, &st, &mac, &pc, opts).unwrap();
        assert!(l.screens(Machine::Pc).is_empty());
        assert_eq!(l.facing(Machine::Mac), None);
        assert_eq!(
            l.screens(Machine::Mac).len(),
            2,
            "la integrada apagada no cuenta"
        );
        assert_eq!(
            l.step(Machine::Mac, Point::new(5359.0, 10.0), 50.0, 0.0),
            Motion::Local(Point::new(5359.0, 10.0))
        );

        // Solo Mac con la PC prendida: la PC no tiene qué mostrar, sigue sin cruce.
        let st = state(Machine::Mac, Machine::Mac, true, BuiltinDisplay::Off);
        let l = Layout::compute(&cfg, &st, &mac, &pc, opts).unwrap();
        assert_eq!(l.facing(Machine::Mac), None);

        // El 34" pasa a la PC: la PC queda a la izquierda.
        let st = state(Machine::Pc, Machine::Mac, true, BuiltinDisplay::Off);
        let l = Layout::compute(&cfg, &st, &mac, &pc, opts).unwrap();
        assert_eq!(l.facing(Machine::Pc), Some(Side::Right));

        // Al revés: el 32" en la PC y el 34" en la Mac, la PC queda a la derecha.
        let st = state(Machine::Mac, Machine::Pc, true, BuiltinDisplay::Off);
        let l = Layout::compute(&cfg, &st, &mac, &pc, opts).unwrap();
        assert_eq!(l.facing(Machine::Pc), Some(Side::Left));
        assert_eq!(l.facing(Machine::Mac), Some(Side::Right));
        assert!(matches!(
            l.step(Machine::Mac, Point::new(3439.0, 700.0), 3.0, 0.0),
            Motion::Cross {
                to: Machine::Pc,
                ..
            }
        ));

        // Solo Windows: la Mac se queda sin monitores externos y usa su integrada,
        // aunque la preferencia sea apagada.
        let st = state(Machine::Pc, Machine::Pc, true, BuiltinDisplay::Off);
        let l = Layout::compute(&cfg, &st, &mac, &pc, opts).unwrap();
        assert_eq!(
            l.screens(Machine::Mac)
                .iter()
                .map(|s| &s.id)
                .collect::<Vec<_>>(),
            vec![&ScreenId::Builtin]
        );
        assert_eq!(l.facing(Machine::Mac), Some(Side::Left));
        // Mitad del 32" de la PC (2160 de alto) → mitad de la integrada (982).
        assert_eq!(
            l.step(Machine::Pc, Point::new(7279.0, 1080.0), 2.0, 0.0),
            Motion::Cross {
                to: Machine::Mac,
                pos: Point::new(0.0, 1440.0 + 491.0)
            }
        );
    }

    #[test]
    fn la_integrada_extendida_suma_su_borde() {
        let cfg = desk();
        // La Mac tiene el 34" (izquierda) y la integrada debajo, alineada a la derecha.
        let mac = vec![
            mon("m34", 0, 0, 3440, 1440, 1.0),
            Screen::new(ScreenId::Builtin, Rect::new(1928, 1440, 1512, 982), 1.0),
        ];
        let pc = vec![mon("m32", 0, 0, 2560, 1440, 1.0)];
        let st = state(Machine::Mac, Machine::Pc, true, BuiltinDisplay::Extend);
        let l = Layout::compute(&cfg, &st, &mac, &pc, no_margin()).unwrap();
        assert_eq!(l.screens(Machine::Mac).len(), 2);
        // El borde derecho de la Mac mide 1440 + 982; la integrada ocupa la parte de abajo.
        let m = l.step(Machine::Mac, Point::new(3439.0, 1440.0 + 491.0), 1.0, 0.0);
        let Motion::Cross { pos, .. } = m else {
            panic!("no cruzó: {m:?}");
        };
        let expected = (1440.0 + 491.0) / (1440.0 + 982.0) * 1440.0;
        assert!((pos.y - expected).abs() < 1e-9);
        assert_eq!(pos.x, 0.0);

        // En espejo la integrada no agrega espacio.
        let st = state(Machine::Mac, Machine::Pc, true, BuiltinDisplay::Mirror);
        let l = Layout::compute(&cfg, &st, &mac, &pc, no_margin()).unwrap();
        assert_eq!(l.screens(Machine::Mac).len(), 1);
    }

    #[test]
    fn un_borde_tapado_por_otra_pantalla_local_no_cruza() {
        let cfg = desk();
        // La Mac reporta los dos monitores, pero el 34" lo está mostrando la PC:
        // es un fantasma que macOS no pudo quitar.
        let mac = vec![
            mon("m34", 0, 0, 3440, 1440, 1.0),
            mon("m32", 3440, 0, 1920, 1080, 1.0),
        ];
        let pc = vec![mon("m34", 0, 0, 3440, 1440, 1.0)];
        let st = state(Machine::Pc, Machine::Mac, true, BuiltinDisplay::Off);
        let l = Layout::compute(&cfg, &st, &mac, &pc, no_margin()).unwrap();
        // El 34" que la Mac no pudo quitar no cuenta: el cursor nunca entra ahí.
        assert_eq!(l.screens(Machine::Mac).len(), 1);
        let m = l.step(Machine::Mac, Point::new(3440.0, 500.0), -1.0, 0.0);
        assert!(matches!(
            m,
            Motion::Cross {
                to: Machine::Pc,
                ..
            }
        ));

        // Con las dos pantallas en la Mac, la unión entre ellas no es un borde.
        let edge = exposed_edge(&mac, Side::Right);
        assert_eq!(
            edge,
            vec![
                Segment {
                    screen: 1,
                    y0: 0.0,
                    y1: 1080.0
                },
                Segment {
                    screen: 0,
                    y0: 1080.0,
                    y1: 1440.0
                },
            ]
        );
    }

    #[test]
    fn pasa_a_otra_pantalla_local_sin_cruzar() {
        let cfg = desk();
        let (mac, pc) = reported();
        let st = state(Machine::Pc, Machine::Pc, true, BuiltinDisplay::Off);
        let l = Layout::compute(&cfg, &st, &mac, &pc, no_margin()).unwrap();
        assert_eq!(
            l.step(Machine::Pc, Point::new(3439.0, 500.0), 2.0, 0.0),
            Motion::Local(Point::new(3441.0, 500.0))
        );
    }

    #[test]
    fn rechaza_pantallas_invalidas() {
        let cfg = desk();
        let st = State::initial(&cfg, Machine::Mac);
        let bad = [mon("m34", 0, 0, 0, 1440, 1.0)];
        assert!(matches!(
            Layout::compute(&cfg, &st, &bad, &[], LayoutOptions::default()),
            Err(GeometryError::EmptyScreen(_))
        ));
        let bad = [mon("m34", 0, 0, 10, 10, 0.0)];
        assert!(matches!(
            Layout::compute(&cfg, &st, &[], &bad, LayoutOptions::default()),
            Err(GeometryError::InvalidScale { .. })
        ));
        let opts = LayoutOptions {
            corner_margin: f64::NAN,
            ..LayoutOptions::default()
        };
        assert!(matches!(
            Layout::compute(&cfg, &st, &[], &[], opts),
            Err(GeometryError::InvalidMargin(_))
        ));
    }

    #[test]
    fn sin_pantallas_el_cursor_no_se_mueve() {
        let cfg = desk();
        let st = state(Machine::Mac, Machine::Mac, false, BuiltinDisplay::Off);
        let l = Layout::compute(&cfg, &st, &[], &[], LayoutOptions::default()).unwrap();
        let p = Point::new(10.0, 10.0);
        assert_eq!(l.step(Machine::Pc, p, 5.0, 5.0), Motion::Local(p));
        assert!(l.screen_at(Machine::Pc, p).is_none());
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

    fn arb_layout() -> impl Strategy<Value = Layout> {
        (
            arb_screen("m34"),
            arb_screen("m32"),
            any::<bool>(),
            0.0f64..40.0,
        )
            .prop_map(|(a, b, pc_left, margin)| {
                let (m34, m32) = if pc_left {
                    (Machine::Pc, Machine::Mac)
                } else {
                    (Machine::Mac, Machine::Pc)
                };
                let st = state(m34, m32, true, BuiltinDisplay::Off);
                let opts = LayoutOptions {
                    corner_margin: margin,
                    ..LayoutOptions::default()
                };
                // Cada máquina reporta solo su monitor; da igual dónde lo ponga.
                let (mac, pc) = if pc_left {
                    (vec![b], vec![a])
                } else {
                    (vec![a], vec![b])
                };
                Layout::compute(&desk(), &st, &mac, &pc, opts).unwrap()
            })
    }

    fn inside(l: &Layout, m: Machine, p: Point) -> bool {
        l.screens(m).iter().any(|s| s.rect.contains(p))
    }

    /// Varias pantallas por máquina, en cualquier posición (pueden solaparse o dejar
    /// huecos), con la integrada extendida y cualquier dueño de cada monitor.
    fn arb_multi_layout() -> impl Strategy<Value = Layout> {
        let machine = prop_oneof![Just(Machine::Mac), Just(Machine::Pc)];
        let pref = prop_oneof![
            Just(BuiltinDisplay::Off),
            Just(BuiltinDisplay::Mirror),
            Just(BuiltinDisplay::Extend),
        ];
        (
            (arb_screen("m34"), arb_screen("m32")),
            (arb_screen("m34"), arb_screen("m32")),
            (-4000i32..4000, -2000i32..3000),
            (machine.clone(), machine, pref, any::<bool>()),
            0.0f64..40.0,
        )
            .prop_map(
                |((mac34, mac32), (pc34, pc32), (bx, by), (o34, o32, pref, side), margin)| {
                    let st = state(o34, o32, true, pref);
                    let opts = LayoutOptions {
                        corner_margin: margin,
                        builtin_side: if side { Side::Left } else { Side::Right },
                    };
                    // Cada máquina reporta todo lo que ve, fantasmas incluidos.
                    let mac = vec![mac34, mac32, builtin(bx, by)];
                    let pc = vec![pc34, pc32];
                    Layout::compute(&desk(), &st, &mac, &pc, opts).unwrap()
                },
            )
    }

    // QA ronda 1: las propiedades de arriba solo usan una pantalla por máquina.
    proptest! {
        /// Con varias pantallas, solapadas o con huecos, el cursor siempre termina
        /// dentro de una pantalla visible, y nunca en la máquina que no es visible.
        #[test]
        fn varias_pantallas_el_cursor_siempre_queda_en_una_visible(
            l in arb_multi_layout(),
            mac in any::<bool>(),
            pick in any::<prop::sample::Index>(),
            fx in 0.0f64..1.0,
            fy in 0.0f64..1.0,
            dx in -3000.0f64..3000.0,
            dy in -3000.0f64..3000.0,
        ) {
            // La Mac siempre tiene algo visible; la PC no siempre.
            let m = if mac || l.screens(Machine::Pc).is_empty() {
                Machine::Mac
            } else {
                Machine::Pc
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
            match (l.facing(Machine::Mac), l.facing(Machine::Pc)) {
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
            // La Mac siempre tiene algo visible; la PC no siempre.
            let m = if mac || l.screens(Machine::Pc).is_empty() {
                Machine::Mac
            } else {
                Machine::Pc
            };
            let screens = l.screens(m);
            let r = pick.get(screens).rect;
            // En el borde derecho, donde más fácil sería cruzar por error.
            let p = Point::new(r.right() - 1.0, (r.top() + fy * f64::from(r.height)).floor());
            prop_assert_eq!(l.step(m, p, 0.0, 0.0), Motion::Local(p));
        }
    }

    proptest! {
        /// Pase lo que pase, el cursor termina dentro de una pantalla visible.
        #[test]
        fn el_cursor_siempre_queda_en_una_pantalla_visible(
            l in arb_layout(),
            mac in any::<bool>(),
            fx in 0.0f64..1.0,
            fy in 0.0f64..1.0,
            dx in -500.0f64..500.0,
            dy in -500.0f64..500.0,
        ) {
            let m = if mac { Machine::Mac } else { Machine::Pc };
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

        /// Cruzar y volver enseguida deja el cursor a menos de un píxel de la altura original.
        #[test]
        fn ida_y_vuelta_conserva_la_altura(l in arb_layout(), fy in 0.0f64..1.0) {
            let side = l.facing(Machine::Pc).unwrap();
            let r = l.screens(Machine::Pc)[0].rect;
            let (x, dx) = match side {
                Side::Right => (r.right() - 1.0, 1.0),
                Side::Left => (r.left(), -1.0),
            };
            let p = Point::new(x, (r.top() + fy * f64::from(r.height)).min(r.bottom() - 1.0));
            if let Motion::Cross { pos, .. } = l.step(Machine::Pc, p, dx, 0.0) {
                match l.step(Machine::Mac, pos, -dx, 0.0) {
                    Motion::Cross { to, pos: back } => {
                        prop_assert_eq!(to, Machine::Pc);
                        prop_assert_eq!(back.x, p.x);
                        // La vuelta puede caer en la esquina muerta de la Mac; solo
                        // se mide cuando vuelve.
                        let tol = 1.0 + f64::from(r.height) / f64::from(l.screens(Machine::Mac)[0].rect.height);
                        prop_assert!((back.y - p.y).abs() <= tol, "{} vs {}", back.y, p.y);
                    }
                    Motion::Local(_) => {}
                }
            }
        }
    }
}
