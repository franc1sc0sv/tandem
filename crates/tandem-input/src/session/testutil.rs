//! Puestos de prueba para las máquinas de estados.

use tandem_core::{Config, Machine, State};

use crate::geometry::{Layout, LayoutOptions, Rect, Screen, ScreenId};

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

pub fn mon(id: &str, x: i32, w: u32, h: u32, scale: f64) -> Screen {
    Screen::new(ScreenId::Monitor(id.into()), Rect::new(x, 0, w, h), scale)
}

fn state(m34: Machine, m32: Machine) -> State {
    let mut s = State::initial(&desk());
    s.owner.insert("m34".into(), m34);
    s.owner.insert("m32".into(), m32);
    s.pc_on = true;
    s
}

fn no_margin() -> LayoutOptions {
    LayoutOptions {
        corner_margin: 0.0,
        ..LayoutOptions::default()
    }
}

/// 34" (3440×1440) en la PC a la izquierda; 32" (1920×1080) en la Mac a la derecha.
pub fn layout_pc_mac() -> Layout {
    let mac = [mon("m32", 3440, 1920, 1080, 1.0)];
    let pc = [mon("m34", 0, 3440, 1440, 1.0)];
    Layout::compute(
        &desk(),
        &state(Machine::Pc, Machine::Mac),
        &mac,
        &pc,
        no_margin(),
    )
    .unwrap()
}

/// Como [`layout_pc_mac`], con el monitor de la Mac de solo 600 de alto.
pub fn layout_pc_mac_small() -> Layout {
    let mac = [mon("m32", 3440, 1920, 600, 1.0)];
    let pc = [mon("m34", 0, 3440, 1440, 1.0)];
    Layout::compute(
        &desk(),
        &state(Machine::Pc, Machine::Mac),
        &mac,
        &pc,
        no_margin(),
    )
    .unwrap()
}

/// Lo mismo, pero la Mac no reporta pantallas: no hay a dónde cruzar.
pub fn layout_pc_only() -> Layout {
    let pc = [mon("m34", 0, 3440, 1440, 1.0)];
    Layout::compute(
        &desk(),
        &state(Machine::Pc, Machine::Mac),
        &[],
        &pc,
        no_margin(),
    )
    .unwrap()
}

/// 34" en la Mac a la izquierda; 32" 4K al 150 % en la PC a la derecha.
pub fn layout_mac_pc_hidpi() -> Layout {
    let mac = [mon("m34", 0, 3440, 1440, 1.0)];
    let pc = [mon("m32", 3440, 3840, 2160, 1.5)];
    Layout::compute(
        &desk(),
        &state(Machine::Mac, Machine::Pc),
        &mac,
        &pc,
        no_margin(),
    )
    .unwrap()
}

/// Última columna del 34" de la PC en [`layout_pc_mac`].
pub fn edge_pc() -> f64 {
    3439.0
}
