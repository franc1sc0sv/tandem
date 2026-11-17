//! Puestos de prueba para las máquinas de estados.

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
        },
        no_margin(),
    )
    .unwrap()
}

/// 34" (3440×1440) en la PC a la izquierda; 32" (1920×1080) en la Mac a la derecha.
pub fn layout_pc_mac() -> Layout {
    layout(
        vec![mon("m32", 3440, 1920, 1080, 1.0)],
        vec![mon("m34", 0, 3440, 1440, 1.0)],
        Side::Right,
    )
}

/// Como [`layout_pc_mac`], con el monitor de la Mac de solo 600 de alto.
pub fn layout_pc_mac_small() -> Layout {
    layout(
        vec![mon("m32", 3440, 1920, 600, 1.0)],
        vec![mon("m34", 0, 3440, 1440, 1.0)],
        Side::Right,
    )
}

/// Lo mismo, pero la Mac no tiene pantallas: no hay a dónde cruzar.
pub fn layout_pc_only() -> Layout {
    layout(vec![], vec![mon("m34", 0, 3440, 1440, 1.0)], Side::Right)
}

/// 34" en la Mac a la izquierda; 32" 4K al 150 % en la PC a la derecha.
pub fn layout_mac_pc_hidpi() -> Layout {
    layout(
        vec![mon("m34", 0, 3440, 1440, 1.0)],
        vec![mon("m32", 3440, 3840, 2160, 1.5)],
        Side::Left,
    )
}

/// Última columna del 34" de la PC en [`layout_pc_mac`].
pub fn edge_pc() -> f64 {
    3439.0
}
