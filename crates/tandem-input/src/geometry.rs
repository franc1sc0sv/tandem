//! Screen geometry and edge crossing between the two machines.
//!
//! Each machine has its screens in its native cursor coordinates (points on
//! macOS, physical pixels on Windows Per-Monitor-V2). [`Layout`] takes them from the arrangement
//! ([`Arrangement`], DESIGN §7): those declared on the shared desk or, if not declared,
//! those each machine reads; and places the two machines side by side according to which
//! side of B A is on. It is recomputed whole on every change: it is a value, there is
//! nothing to invalidate.
//!
//! The crossing is tested against the edge of the screen where the cursor is, never
//! against the box that encloses all of them (docs/research/input-engine.md), and the
//! position is carried as a fraction along the shared edge, so that
//! monitors of different sizes feel continuous.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use tandem_core::Machine;
pub use tandem_core::{Align, Side};

/// Cursor position in a machine's native coordinates.
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

/// Half-open rectangle `[x, x + width) × [y, y + height)`, with `y` pointing down.
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

    /// Takes the point to the last pixel inside the rectangle.
    pub fn clamp(&self, p: Point) -> Point {
        Point::new(
            p.x.clamp(self.left(), self.right() - 1.0),
            p.y.clamp(self.top(), self.bottom() - 1.0),
        )
    }

    /// Distance from the point to the rectangle (0 if inside).
    pub fn distance(&self, p: Point) -> f64 {
        let dx = (self.left() - p.x).max(p.x - (self.right() - 1.0)).max(0.0);
        let dy = (self.top() - p.y).max(p.y - (self.bottom() - 1.0)).max(0.0);
        dx.hypot(dy)
    }
}

/// The id of a screen in the arrangement (`a-1`, or that of a `[[screens]]`).
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

/// A screen as its machine reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    pub id: ScreenId,
    /// In the native cursor coordinates of its machine.
    pub rect: Rect,
    /// Native units per logical point: `dpi / 96` on Windows Per-Monitor-V2;
    /// `1.0` on macOS, where the cursor already moves in points.
    pub scale: f64,
}

impl Screen {
    pub fn new(id: ScreenId, rect: Rect, scale: f64) -> Self {
        Self { id, rect, scale }
    }

    /// Native delta of this screen → logical points (what travels over the link).
    pub fn to_logical(&self, dx: f64, dy: f64) -> (f64, f64) {
        (dx / self.scale, dy / self.scale)
    }

    /// Logical points → native delta of this screen. With [`Screen::to_logical`]
    /// at the origin, speed feels the same on monitors of different DPI.
    pub fn to_native(&self, dx: f64, dy: f64) -> (f64, f64) {
        (dx * self.scale, dy * self.scale)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayoutOptions {
    /// Margin of the corners in logical points: no crossing there, so as not to
    /// fight with macOS hot corners or with window buttons.
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
    #[error("screen {0:?} has no area")]
    EmptyScreen(ScreenId),
    #[error("screen {id:?} has an invalid scale ({scale})")]
    InvalidScale { id: ScreenId, scale: f64 },
    #[error("the corner margin is invalid ({0})")]
    InvalidMargin(f64),
}

/// The arrangement of the two machines: each one's screens, which side of B A is on and,
/// if declared, how they align (D18). A machine without screens (off, or that has not
/// yet sent them) has nowhere to cross to.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Arrangement {
    pub a: Vec<Screen>,
    pub b: Vec<Screen>,
    pub a_side: Side,
    /// `Some`: the two desktops touch only along the stretch they share with that
    /// alignment, measured in logical points, and the cursor exits at the same height. `None` (no
    /// declared arrangement): the whole edge of each one crosses and the position goes as a fraction
    /// of the length.
    pub align: Option<Align>,
}

/// The stretch shared over an edge of length `mine` facing one of length `theirs` (both
/// in logical points), with `align`: from where to where, on the own edge, the other
/// desktop is. It is the crossing edge. Empty (`start >= end`) if they do not touch.
pub fn shared_span(mine: f64, theirs: f64, align: Align) -> (f64, f64) {
    let off = (mine - theirs) * align.factor();
    (off.max(0.0), (off + theirs).min(mine))
}

/// Stretch of the edge that faces the other machine and through which one can cross, along the edge
/// axis: `y` on a left or right edge, `x` on a top or bottom one.
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
    /// Sorted along the axis; the crossing position is measured over their concatenation.
    edge: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq)]
struct MachineLayout {
    screens: Vec<Screen>,
    /// `None` if either has no screens: there is nowhere to cross to.
    side: Option<Facing>,
}

impl MachineLayout {
    /// The length of a stretch measured in native units (`logical = false`) or in logical
    /// points, with its screen's scale.
    fn measure(&self, seg: &Segment, logical: bool) -> f64 {
        if logical {
            seg.len() / self.screens[seg.screen].scale
        } else {
            seg.len()
        }
    }

    /// The length of the whole edge.
    fn edge_len(&self, logical: bool) -> Option<f64> {
        let side = self.side.as_ref()?;
        Some(side.edge.iter().map(|s| self.measure(s, logical)).sum())
    }
}

/// Result of moving the cursor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Motion {
    /// The cursor stays on the same machine, at this position.
    Local(Point),
    /// The cursor leaves through the edge and enters the other machine at this position.
    Cross { to: Machine, pos: Point },
}

/// Screens of the two machines and how they touch.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    opts: LayoutOptions,
    align: Option<Align>,
    a: MachineLayout,
    b: MachineLayout,
}

/// The coordinates of a rectangle on the edge axis of `side`.
fn span(r: &Rect, side: Side) -> (f64, f64) {
    if side.is_vertical_edge() {
        (r.top(), r.bottom())
    } else {
        (r.left(), r.right())
    }
}

impl Layout {
    /// Builds the layout from the arrangement. All screens count: the arrangement already is
    /// what is looked at.
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

        // A, on one side of B, faces the other.
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

    /// Screens of a machine.
    pub fn screens(&self, machine: Machine) -> &[Screen] {
        &self.machine(machine).screens
    }

    /// Side of the machine through which one passes to the other; `None` if it cannot be crossed.
    pub fn facing(&self, machine: Machine) -> Option<Side> {
        self.machine(machine).side.as_ref().map(|s| s.facing)
    }

    /// The crossing edge of `machine`: from where to where of its edge (in logical points
    /// from the start, top or left) one can pass to the other. Without
    /// alignment, the whole edge. `None` if it cannot be crossed.
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

    /// Screen that contains the point or, if it falls outside all of them, the nearest one.
    pub fn screen_at(&self, machine: Machine, pos: Point) -> Option<&Screen> {
        self.screen_index(machine, pos)
            .map(|i| &self.machine(machine).screens[i])
    }

    /// Moves `machine`'s cursor from `pos` with a native delta.
    ///
    /// If the movement leaves through the edge that faces the other machine, within the crossing
    /// edge and outside the corner margin, it returns where it appears on the other. Otherwise,
    /// the cursor moves to another local screen or stays against the edge of the current one, as
    /// the system does. Without screens it returns `pos` as is.
    pub fn step(&self, machine: Machine, pos: Point, dx: f64, dy: f64) -> Motion {
        self.step_inner(machine, pos, dx, dy, true)
    }

    /// Like [`Layout::step`], without ever crossing to the other machine: the locked cursor
    /// (the server's shortcut) stays against the edge.
    pub fn step_within(&self, machine: Machine, pos: Point, dx: f64, dy: f64) -> Point {
        match self.step_inner(machine, pos, dx, dy, false) {
            Motion::Local(p) => p,
            // `step_inner` without a crossing returns nothing else.
            Motion::Cross { .. } => pos,
        }
    }

    fn step_inner(&self, machine: Machine, pos: Point, dx: f64, dy: f64, cross: bool) -> Motion {
        let ml = self.machine(machine);
        let Some(cur) = self.screen_index(machine, pos) else {
            return Motion::Local(pos);
        };
        let screen = &ml.screens[cur];
        // From outside any screen (geometry just changed) it starts from the
        // nearest point of the current screen.
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
        // If the movement passes the edge that faces the other: what part of the movement
        // is needed to touch it.
        let t = match side.facing {
            Side::Right if dx > 0.0 && pos.x + dx >= rect.right() => (rect.right() - pos.x) / dx,
            Side::Left if dx < 0.0 && pos.x + dx < rect.left() => (rect.left() - pos.x) / dx,
            Side::Down if dy > 0.0 && pos.y + dy >= rect.bottom() => (rect.bottom() - pos.y) / dy,
            Side::Up if dy < 0.0 && pos.y + dy < rect.top() => (rect.top() - pos.y) / dy,
            _ => return None,
        };
        let t = t.clamp(0.0, 1.0);
        // Where, along the edge, the trajectory touches it.
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
        // Without alignment it is measured in native units (fraction of the length); with alignment,
        // in logical points (the same distance on both).
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
                // Where the other desktop starts over this edge.
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

    /// Point of `machine`'s edge that is `at` from its start, measured as in
    /// [`MachineLayout::measure`] (the last one of the edge if `at` passes it).
    fn land(&self, machine: Machine, at: f64, logical: bool) -> Option<Point> {
        let ml = self.machine(machine);
        let side = ml.side.as_ref()?;
        let last = side.edge.last()?;
        let mut left = at;
        let mut hit = (last, last.len());
        for seg in &side.edge {
            let len = ml.measure(seg, logical);
            if left < len {
                // From the measure of the stretch to native units.
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

/// Stretches of the `facing` side of each screen that do not touch another local screen.
fn exposed_edge(screens: &[Screen], facing: Side) -> Vec<Segment> {
    let mut edge = Vec::new();
    for (i, s) in screens.iter().enumerate() {
        let mut free = vec![span(&s.rect, facing)];
        for (j, o) in screens.iter().enumerate() {
            if i == j {
                continue;
            }
            let (s, o) = (&s.rect, &o.rect);
            // `o` covers the edge if it contains the points just on the other side.
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

    /// The 34" (3440×1440) on the PC at the left; the 32" (1920×1080) on the Mac at the right.
    fn shared(opts: LayoutOptions) -> Layout {
        let mac = vec![mon("m32", 3440, 0, 1920, 1080, 1.0)];
        let pc = vec![mon("m34", 0, 0, 3440, 1440, 1.0)];
        Layout::compute(&arr(mac, pc, Side::Right), opts).unwrap()
    }

    #[test]
    fn each_machine_faces_the_other_according_to_the_arrangement() {
        let l = shared(no_margin());
        assert_eq!(l.screens(Machine::B)[0].id, "m34".into());
        assert_eq!(l.screens(Machine::A)[0].id, "m32".into());
        assert_eq!(l.facing(Machine::B), Some(Side::Right));
        assert_eq!(l.facing(Machine::A), Some(Side::Left));

        // Without a declared arrangement the Mac goes on the left.
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
    fn crossing_between_monitors_of_different_size_is_proportional() {
        let l = shared(no_margin());
        // Half of the 34" (1440 tall) → half of the 32" of the Mac (1080 tall).
        let m = l.step(Machine::B, Point::new(3439.0, 720.0), 5.0, 0.0);
        assert_eq!(
            m,
            Motion::Cross {
                to: Machine::A,
                pos: Point::new(3440.0, 540.0)
            }
        );
        // A quarter from the top.
        let m = l.step(Machine::B, Point::new(3439.0, 360.0), 1.0, 0.0);
        assert_eq!(
            m,
            Motion::Cross {
                to: Machine::A,
                pos: Point::new(3440.0, 270.0)
            }
        );
        // And back: the Mac's left edge leads to the PC's right.
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
    fn the_edge_that_does_not_face_the_other_machine_does_not_cross() {
        let l = shared(no_margin());
        // Left of the PC's 34": there is nothing there.
        assert_eq!(
            l.step(Machine::B, Point::new(0.0, 500.0), -10.0, 0.0),
            Motion::Local(Point::new(0.0, 500.0))
        );
        // Right of the Mac's 32".
        assert_eq!(
            l.step(Machine::A, Point::new(5359.0, 500.0), 10.0, 3.0),
            Motion::Local(Point::new(5359.0, 503.0))
        );
        // A normal movement inside the screen.
        assert_eq!(
            l.step(Machine::B, Point::new(100.0, 100.0), 10.0, -5.0),
            Motion::Local(Point::new(110.0, 95.0))
        );
    }

    #[test]
    fn dead_corners() {
        let l = shared(LayoutOptions {
            corner_margin: 20.0,
        });
        let at = |y| l.step(Machine::B, Point::new(3439.0, y), 4.0, 0.0);
        // Within the top and bottom margin the cursor stays against the edge.
        assert_eq!(at(0.0), Motion::Local(Point::new(3439.0, 0.0)));
        assert_eq!(at(19.0), Motion::Local(Point::new(3439.0, 19.0)));
        assert_eq!(at(1420.5), Motion::Local(Point::new(3439.0, 1420.5)));
        assert_eq!(at(1439.0), Motion::Local(Point::new(3439.0, 1439.0)));
        // Just outside the margin it crosses.
        assert!(matches!(at(20.0), Motion::Cross { to: Machine::A, .. }));
        assert!(matches!(at(1419.0), Motion::Cross { to: Machine::A, .. }));
    }

    #[test]
    fn the_corner_margin_scales_by_dpi() {
        // The PC with the 34" and the 32" at 150 %; the Mac on the right with its screen.
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
        // 20 points at 150 % are 30 pixels.
        assert!(matches!(at(29.0), Motion::Local(_)));
        assert!(matches!(at(30.0), Motion::Cross { to: Machine::A, .. }));
        assert!(matches!(at(2130.0), Motion::Local(_)));
        assert!(matches!(at(2129.0), Motion::Cross { to: Machine::A, .. }));
        // Half of the PC's 32" (2160 tall) → half of the Mac's screen (982).
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
    fn speed_scales_by_dpi() {
        let m34 = mon("m34", 0, 0, 3440, 1440, 1.0);
        let m32 = mon("m32", 3440, 0, 3840, 2160, 1.5);
        // 30 pixels on the 32" at 150 % are 20 points; on the 34" at 100 %, 20 pixels.
        let (lx, ly) = m32.to_logical(30.0, -15.0);
        assert_eq!((lx, ly), (20.0, -10.0));
        assert_eq!(m34.to_native(lx, ly), (20.0, -10.0));
        assert_eq!(m32.to_native(lx, ly), (30.0, -15.0));
    }

    #[test]
    fn diagonal_crossing_uses_the_height_where_it_touches_the_edge() {
        let l = shared(no_margin());
        // From x=3430 with dx=20 it touches the edge halfway: y = 700 + 10.
        let m = l.step(Machine::B, Point::new(3430.0, 700.0), 20.0, 20.0);
        let Motion::Cross { to, pos } = m else {
            panic!("did not cross: {m:?}");
        };
        assert_eq!(to, Machine::A);
        assert!((pos.y - 710.0 * 1080.0 / 1440.0).abs() < 1e-9);
    }

    #[test]
    fn without_the_others_screens_there_is_no_crossing_and_the_side_comes_from_the_arrangement() {
        let mac = vec![
            mon("m34", 0, 0, 3440, 1440, 1.0),
            mon("m32", 3440, 0, 1920, 1080, 1.0),
        ];
        let pc = vec![mon("pc", 0, 0, 2560, 1440, 1.0)];
        // The PC off (or without screens yet): there is nowhere to cross to.
        let l = Layout::compute(&arr(mac.clone(), vec![], Side::Left), no_margin()).unwrap();
        assert_eq!(l.facing(Machine::A), None);
        assert_eq!(
            l.step(Machine::A, Point::new(5359.0, 10.0), 50.0, 0.0),
            Motion::Local(Point::new(5359.0, 10.0))
        );
        // The Mac on the left: its right edge leads to the PC.
        let l = Layout::compute(&arr(mac.clone(), pc.clone(), Side::Left), no_margin()).unwrap();
        assert_eq!(l.facing(Machine::A), Some(Side::Right));
        assert!(matches!(
            l.step(Machine::A, Point::new(5359.0, 500.0), 3.0, 0.0),
            Motion::Cross { to: Machine::B, .. }
        ));
        // On the right: the left one.
        let l = Layout::compute(&arr(mac, pc, Side::Right), no_margin()).unwrap();
        assert_eq!(l.facing(Machine::A), Some(Side::Left));
        assert!(matches!(
            l.step(Machine::A, Point::new(0.0, 500.0), -3.0, 0.0),
            Motion::Cross { to: Machine::B, .. }
        ));
    }

    #[test]
    fn two_stacked_screens_add_up_their_edge() {
        // The Mac has the 34" and its screen below, aligned to the right.
        let mac = vec![
            mon("m34", 0, 0, 3440, 1440, 1.0),
            Screen::new("laptop".into(), Rect::new(1928, 1440, 1512, 982), 1.0),
        ];
        let pc = vec![mon("m32", 0, 0, 2560, 1440, 1.0)];
        let l = Layout::compute(&arr(mac, pc, Side::Left), no_margin()).unwrap();
        // The Mac's right edge measures 1440 + 982; the bottom one takes the bottom part.
        let m = l.step(Machine::A, Point::new(3439.0, 1440.0 + 491.0), 1.0, 0.0);
        let Motion::Cross { pos, .. } = m else {
            panic!("did not cross: {m:?}");
        };
        let expected = (1440.0 + 491.0) / (1440.0 + 982.0) * 1440.0;
        assert!((pos.y - expected).abs() < 1e-9);
        assert_eq!(pos.x, 0.0);
    }

    #[test]
    fn an_edge_covered_by_another_local_screen_does_not_cross() {
        let mac = vec![
            mon("m34", 0, 0, 3440, 1440, 1.0),
            mon("m32", 3440, 0, 1920, 1080, 1.0),
        ];
        // With both screens on the Mac, the join between them is not an edge.
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
    fn moves_to_another_local_screen_without_crossing() {
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
    fn rejects_invalid_screens() {
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

    /// A point of edge `side` of `r` at `f` of its length, and the step that takes it out there.
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

    /// DESIGN §7 / D18: the other machine on each of the 4 sides with each of the 3
    /// alignments. B has the 34" (3440×1440) and A a smaller screen (1920×1080):
    /// the crossing edge is the stretch they share, and it exits at the same distance.
    #[test]
    fn four_sides_by_three_alignments_give_the_shared_edge() {
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
                // Length of each edge and where A starts on B's.
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
                // Within the stretch it crosses at the same distance from its start.
                for d in [0.0, 1.0, len_a / 2.0, len_a - 1.0] {
                    assert_eq!(
                        at(start + d),
                        Motion::Cross {
                            to: Machine::A,
                            pos: land(d)
                        },
                        "{case} at {d}"
                    );
                }
                // Outside the stretch, the cursor stays against the edge.
                if start >= 1.0 {
                    assert!(matches!(at(start - 1.0), Motion::Local(_)), "{case}");
                }
                if start + len_a < len_b {
                    assert!(matches!(at(start + len_a), Motion::Local(_)), "{case}");
                }
                // And back: from any point of A the stretch of B is reached.
                let (p, dx, dy) = on_edge(a_rect, a_side.opposite(), 0.5);
                let Motion::Cross { to, pos } = l.step(Machine::A, p, dx, dy) else {
                    panic!("{case}: A did not cross");
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
    fn with_alignment_the_stretch_is_measured_in_logical_points() {
        // B with a 4K at 150 % (2560×1440 points) to the right of A (1512×982 at 100 %),
        // aligned to the center: A starts at (1440 − 982) / 2 = 229 points from B's edge.
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
        // 229 points are 343.5 pixels: just before it does not cross; just after it does, at the top.
        assert!(matches!(
            l.step(Machine::B, Point::new(3839.0, 343.0), 1.0, 0.0),
            Motion::Local(_)
        ));
        let Motion::Cross { to, pos } = l.step(Machine::B, Point::new(3839.0, 345.0), 1.0, 0.0)
        else {
            panic!("did not cross within the stretch");
        };
        assert_eq!((to, pos.x), (Machine::A, 0.0));
        assert!((pos.y - 1.0).abs() < 1e-9, "{pos:?}");
        assert_eq!(shared_span(100.0, 300.0, Align::End), (0.0, 100.0));
        assert_eq!(shared_span(300.0, 100.0, Align::End), (200.0, 300.0));
        assert_eq!(shared_span(300.0, 100.0, Align::Start), (0.0, 100.0));
    }

    #[test]
    fn without_screens_the_cursor_does_not_move() {
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

    /// One screen per machine, anywhere, the Mac on either side, with or without
    /// alignment.
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

    /// Several screens per machine, in any position (they may overlap or leave
    /// gaps); sometimes the PC with none.
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

    // QA round 1: the properties below only use one screen per machine.
    proptest! {
        /// With several screens, overlapping or with gaps, the cursor always ends up
        /// inside a screen, and never on the machine that has none.
        #[test]
        fn several_screens_the_cursor_always_ends_up_on_a_screen(
            l in arb_multi_layout(),
            mac in any::<bool>(),
            pick in any::<prop::sample::Index>(),
            fx in 0.0f64..1.0,
            fy in 0.0f64..1.0,
            dx in -3000.0f64..3000.0,
            dy in -3000.0f64..3000.0,
        ) {
            // The Mac always has something; the PC not always.
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
                    // It enters through the side that faces the origin machine, on the
                    // edge of some screen that contains the point.
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
                    prop_assert!(on_edge, "{pos:?} is not on edge {facing:?}");
                }
            }
        }

        /// The two sides always face each other: if one machine can cross, the other
        /// can too, through the opposite side.
        #[test]
        fn the_sides_are_always_opposite(l in arb_multi_layout()) {
            match (l.facing(Machine::A), l.facing(Machine::B)) {
                (Some(a), Some(b)) => prop_assert_eq!(a, b.opposite()),
                (None, None) => {}
                other => prop_assert!(false, "inconsistent sides: {other:?}"),
            }
        }

        /// A null movement never crosses or moves a cursor that is inside.
        #[test]
        fn without_movement_there_is_no_crossing(
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
            // On the right edge, where it would be easiest to cross by mistake.
            let p = Point::new(r.right() - 1.0, (r.top() + fy * f64::from(r.height)).floor());
            prop_assert_eq!(l.step(m, p, 0.0, 0.0), Motion::Local(p));
        }
    }

    proptest! {
        /// Whatever happens, the cursor ends up inside a screen.
        #[test]
        fn the_cursor_always_ends_up_on_a_screen(
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

        /// Crossing and coming straight back leaves the cursor within one pixel of the original
        /// position along the edge, through any of the four sides.
        #[test]
        fn round_trip_keeps_the_position(l in arb_layout(), f in 0.0f64..1.0) {
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
                        // The return may land in the Mac's dead corner; it is only
                        // measured when it returns.
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

        /// With alignment, it only crosses within the shared edge, and exits at the same
        /// distance (in logical points) from where the other desktop starts.
        #[test]
        fn with_alignment_only_the_shared_stretch_crosses(
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
                    prop_assert!(u >= s - 1e-9 && u < e + 1e-9, "{u} outside {s}..{e}");
                }
                Motion::Local(_) => {}
            }
        }
    }

    /// Screens of a machine in a row, of different heights and aligned top, bottom or
    /// center (partial edges of the side that faces the other).
    fn arb_row(id: &'static str) -> impl Strategy<Value = Vec<Screen>> {
        proptest::collection::vec(
            (
                400u32..3000,
                300u32..2200,
                0u8..3,
                prop::sample::select(vec![1.0, 1.25, 1.5, 2.0]),
            ),
            1..4,
        )
        .prop_map(move |specs| {
            let tallest = specs.iter().map(|s| s.1).max().unwrap_or(0);
            let mut x = 0i32;
            specs
                .into_iter()
                .map(|(w, h, align, scale)| {
                    let y = match align {
                        0 => 0,
                        1 => (tallest - h) as i32,
                        _ => ((tallest - h) / 2) as i32,
                    };
                    let s = mon(id, x, y, w, h, scale);
                    x += w as i32;
                    s
                })
                .collect()
        })
    }

    /// Whether `p` is on the edge that faces the other machine of some screen, in a stretch
    /// that does not cover another local screen.
    fn on_exposed_edge(screens: &[Screen], facing: Side, p: Point) -> bool {
        exposed_edge(screens, facing).iter().any(|seg| {
            let r = screens[seg.screen].rect;
            let (on_line, along) = match facing {
                Side::Left => (p.x == r.left(), p.y),
                Side::Right => (p.x == r.right() - 1.0, p.y),
                Side::Up => (p.y == r.top(), p.x),
                Side::Down => (p.y == r.bottom() - 1.0, p.x),
            };
            on_line && along >= seg.t0 && along < seg.t1
        })
    }

    proptest! {
        /// T-093, respecting screens: in a long walk of random movements, going back and
        /// forth between the two machines through any of the 4 sides, with or without
        /// alignment, and with screens of different heights (partial edges and gaps in
        /// the corners), the cursor never ends up outside a screen of the machine it
        /// is on; every crossing leaves from an exposed stretch of the edge, outside the margin of the
        /// corners of its screen, and enters through an exposed stretch of the other.
        #[test]
        fn a_walk_never_leaves_the_screens_and_crosses_only_through_exposed_edges(
            a in arb_row("a"),
            b in arb_row("b"),
            a_side in side(),
            align in align(),
            margin in 0.0f64..40.0,
            moves in proptest::collection::vec((-900.0f64..900.0, -700.0f64..700.0), 1..80),
        ) {
            let opts = LayoutOptions { corner_margin: margin };
            let l = Layout::compute(&Arrangement { align, ..arr(a, b, a_side) }, opts).unwrap();
            let mut m = Machine::A;
            let r = l.screens(m)[0].rect;
            let mut p = Point::new(
                r.left() + f64::from(r.width) / 2.0,
                r.top() + f64::from(r.height) / 2.0,
            );
            for (dx, dy) in moves {
                match l.step(m, p, dx, dy) {
                    Motion::Local(q) => {
                        prop_assert!(inside(&l, m, q), "{q:?} outside {m}");
                        p = q;
                    }
                    Motion::Cross { to, pos } => {
                        let facing = l.facing(m).unwrap();
                        let s = l.screen_at(m, p).unwrap();
                        let rect = s.rect;
                        // Where the trajectory touches the edge of the origin screen, as
                        // `crossing` measures it.
                        let t = match facing {
                            Side::Right => (rect.right() - p.x) / dx,
                            Side::Left => (rect.left() - p.x) / dx,
                            Side::Down => (rect.bottom() - p.y) / dy,
                            Side::Up => (rect.top() - p.y) / dy,
                        }
                        .clamp(0.0, 1.0);
                        let (lo, hi) = span(&rect, facing);
                        let along = if facing.is_vertical_edge() { p.y + dy * t } else { p.x + dx * t };
                        let margin = margin * s.scale;
                        prop_assert!(
                            along >= lo + margin - 1e-6 && along < hi - margin + 1e-6,
                            "crossed at the corner: {along} in {rect:?}"
                        );
                        let along = along.clamp(lo, hi - 1.0);
                        let exit = match facing {
                            Side::Left => Point::new(rect.left(), along),
                            Side::Right => Point::new(rect.right() - 1.0, along),
                            Side::Up => Point::new(along, rect.top()),
                            Side::Down => Point::new(along, rect.bottom() - 1.0),
                        };
                        prop_assert!(
                            on_exposed_edge(l.screens(m), facing, exit),
                            "left through a covered stretch or one without an edge: {exit:?}"
                        );
                        prop_assert!(inside(&l, to, pos), "{pos:?} outside {to}");
                        prop_assert!(
                            on_exposed_edge(l.screens(to), l.facing(to).unwrap(), pos),
                            "entered outside an exposed stretch: {pos:?}"
                        );
                        m = to;
                        p = pos;
                    }
                }
            }
        }
    }
}
