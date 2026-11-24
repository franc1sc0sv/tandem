//! Test desks for the state machines.

use crate::geometry::{Arrangement, Layout, LayoutOptions, Rect, Screen, Side};

pub fn mon(id: &str, x: i32, w: u32, h: u32, scale: f64) -> Screen {
    Screen::new(id.into(), Rect::new(x, 0, w, h), scale)
}

fn no_margin() -> LayoutOptions {
    LayoutOptions { corner_margin: 0.0 }
}

fn layout(mac: Vec<Screen>, pc: Vec<Screen>, a_side: Side) -> Layout {
    Layout::compute(
        &Arrangement {
            a: mac,
            b: pc,
            a_side,
            align: None,
        },
        no_margin(),
    )
    .unwrap()
}

/// 34" (3440×1440) on the PC at the left; 32" (1920×1080) on the Mac at the right.
pub fn layout_pc_mac() -> Layout {
    layout(
        vec![mon("m32", 3440, 1920, 1080, 1.0)],
        vec![mon("m34", 0, 3440, 1440, 1.0)],
        Side::Right,
    )
}

/// Like [`layout_pc_mac`], with the Mac's monitor only 600 tall.
pub fn layout_pc_mac_small() -> Layout {
    layout(
        vec![mon("m32", 3440, 1920, 600, 1.0)],
        vec![mon("m34", 0, 3440, 1440, 1.0)],
        Side::Right,
    )
}

/// Like [`layout_pc_mac`], with a second PC screen (1920×1080) left of the 34".
pub fn layout_pc_two_mac() -> Layout {
    layout(
        vec![mon("m32", 3440, 1920, 1080, 1.0)],
        vec![
            mon("m24", -1920, 1920, 1080, 1.0),
            mon("m34", 0, 3440, 1440, 1.0),
        ],
        Side::Right,
    )
}

/// The same, but the Mac has no screens: there is nowhere to cross to.
pub fn layout_pc_only() -> Layout {
    layout(vec![], vec![mon("m34", 0, 3440, 1440, 1.0)], Side::Right)
}

/// 34" on the Mac at the left; 32" 4K at 150 % on the PC at the right.
pub fn layout_mac_pc_hidpi() -> Layout {
    layout(
        vec![mon("m34", 0, 3440, 1440, 1.0)],
        vec![mon("m32", 3440, 3840, 2160, 1.5)],
        Side::Left,
    )
}

/// Last column of the PC's 34" in [`layout_pc_mac`].
pub fn edge_pc() -> f64 {
    3439.0
}
