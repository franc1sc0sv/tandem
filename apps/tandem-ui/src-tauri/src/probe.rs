//! La ventana de la sonda de calibración (T-078): «¿ves esto?» a pantalla completa en el
//! monitor que se calibra.
//!
//! El agente dice dónde está el monitor en el escritorio de esta máquina (buscado por su
//! EDID) o solo su nombre. Aquí se decide en cuál de los monitores que ve Tauri abrir la
//! ventana; la ventana en sí (`probe.html`) la abre y la cierra la app ([`crate::app`]).
//!
//! Las coordenadas del agente son las del sistema: píxeles en Windows y puntos en macOS
//! ([`Units`]), y Tauri da los monitores siempre en píxeles físicos. No se puede adivinar
//! cuál es: en una Mac con pantalla Retina, un lugar en puntos cae también dentro de la
//! integrada en píxeles.

use crate::session::{ProbeView, ScreenView};

/// Prefijo de la etiqueta de cada ventana de sonda (`probe-<id>`).
pub const LABEL_PREFIX: &str = "probe-";

/// Un monitor como lo da Tauri: posición y tamaño en píxeles físicos.
#[derive(Debug, Clone, PartialEq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub name: Option<String>,
}

impl Area {
    fn contains(&self, px: f64, py: f64, scale: f64) -> bool {
        let (x, y) = (f64::from(self.x) / scale, f64::from(self.y) / scale);
        let (w, h) = (
            f64::from(self.width) / scale,
            f64::from(self.height) / scale,
        );
        px >= x && px < x + w && py >= y && py < y + h
    }
}

/// En qué unidades da el sistema el lugar de las pantallas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Units {
    /// Windows: el escritorio virtual en píxeles.
    Pixels,
    /// macOS: el espacio global de Quartz en puntos.
    Points,
}

impl Units {
    pub const NATIVE: Units = if cfg!(target_os = "macos") {
        Units::Points
    } else {
        Units::Pixels
    };
}

pub fn window_label(id: u64) -> String {
    format!("{LABEL_PREFIX}{id}")
}

/// El número de sonda de una ventana, si es de sonda.
pub fn probe_id(label: &str) -> Option<u64> {
    label.strip_prefix(LABEL_PREFIX)?.parse().ok()
}

/// En cuál monitor abrir la sonda. `None` si no hay forma de saberlo: la app usa el
/// principal (y el texto de la sonda dice qué monitor es).
pub fn pick(
    screen: Option<ScreenView>,
    units: Units,
    name: &str,
    monitors: &[Area],
) -> Option<usize> {
    if let Some(s) = screen {
        // El centro: un borde compartido entre dos monitores no decide nada.
        let cx = f64::from(s.x) + f64::from(s.width) / 2.0;
        let cy = f64::from(s.y) + f64::from(s.height) / 2.0;
        let found = monitors.iter().position(|m| match units {
            Units::Pixels => m.contains(cx, cy, 1.0),
            Units::Points => m.scale > 0.0 && m.contains(cx, cy, m.scale),
        });
        if let Some(i) = found {
            return Some(i);
        }
    }
    let wanted = name.trim().to_lowercase();
    if wanted.is_empty() {
        return None;
    }
    // En macOS el nombre que da el sistema suele ser el del EDID.
    let hits: Vec<usize> = monitors
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            m.name.as_deref().is_some_and(|n| {
                let n = n.trim().to_lowercase();
                !n.is_empty() && (n.contains(&wanted) || wanted.contains(&n))
            })
        })
        .map(|(i, _)| i)
        .collect();
    match hits.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

/// La página de la ventana de la sonda.
pub const PAGE: &str = "probe.html";
/// Dónde deja [`init_script`] la vista para `probe.html` (`src/probe-window.ts`).
pub const VIEW_GLOBAL: &str = "__TANDEM_PROBE__";

/// Lo que la ventana de la sonda tiene que mostrar, como script que corre antes que la
/// página: la ventana no tiene más estado que ese. El JSON es JavaScript válido.
pub fn init_script(view: &ProbeView) -> Result<String, serde_json::Error> {
    Ok(format!(
        "window.{VIEW_GLOBAL} = {};",
        serde_json::to_string(view)?
    ))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::session::{AskView, PlaceView};

    fn area(x: i32, width: u32, scale: f64, name: &str) -> Area {
        Area {
            x,
            y: 0,
            width,
            height: 1440,
            scale,
            name: Some(name.into()),
        }
    }

    fn screen(x: i32, width: u32, height: u32) -> Option<ScreenView> {
        Some(ScreenView {
            x,
            y: 0,
            width,
            height,
        })
    }

    #[test]
    fn windows_coordinates_are_pixels() {
        let ms = [
            area(0, 2560, 1.0, r"\\.\DISPLAY1"),
            area(2560, 3440, 1.0, r"\\.\DISPLAY2"),
        ];
        assert_eq!(
            pick(screen(2560, 3440, 1440), Units::Pixels, "LC34G55T", &ms),
            Some(1)
        );
        assert_eq!(pick(screen(0, 2560, 1440), Units::Pixels, "", &ms), Some(0));
    }

    #[test]
    fn mac_coordinates_are_points() {
        // La integrada Retina (escala 2, 1512 puntos) en (0,0) y el 32" (escala 1) a su
        // derecha: el agente da el 32" en puntos, Tauri en píxeles. En píxeles el centro
        // del 32" caería dentro de la integrada.
        let ms = [
            area(0, 3024, 2.0, "Pantalla integrada"),
            area(1512, 2560, 1.0, "Odyssey G5"),
        ];
        assert_eq!(
            pick(screen(1512, 2560, 1440), Units::Points, "", &ms),
            Some(1)
        );
        assert_eq!(pick(screen(0, 1512, 982), Units::Points, "", &ms), Some(0));
        assert_eq!(
            pick(screen(1512, 2560, 1440), Units::Pixels, "", &ms),
            Some(0)
        );
    }

    #[test]
    fn without_a_place_the_name_decides_if_it_is_unique() {
        let ms = [
            area(0, 3024, 2.0, "Pantalla integrada"),
            area(3024, 2560, 1.0, "Odyssey G5"),
            area(5584, 3440, 1.0, "LC34G55T"),
        ];
        let by_name = |name, ms: &[Area]| pick(None, Units::Points, name, ms);
        assert_eq!(by_name("odyssey g5", &ms), Some(1));
        assert_eq!(by_name("LC34G55T", &ms), Some(2));
        // Un lugar que no está en ningún monitor cae al nombre.
        assert_eq!(
            pick(screen(-9000, 10, 10), Units::Pixels, "LC34G55T", &ms),
            Some(2)
        );
        assert_eq!(by_name("Dell", &ms), None);
        assert_eq!(by_name("  ", &ms), None);
        // Dos con el mismo nombre: no se adivina.
        let twins = [
            area(0, 10, 1.0, "Odyssey G5"),
            area(10, 10, 1.0, "Odyssey G5"),
        ];
        assert_eq!(by_name("Odyssey G5", &twins), None);
    }

    #[test]
    fn labels_carry_the_probe_number() {
        assert_eq!(window_label(42), "probe-42");
        assert_eq!(probe_id("probe-42"), Some(42));
        assert_eq!(probe_id("main"), None);
        assert_eq!(probe_id("probe-x"), None);
    }

    #[test]
    fn the_window_gets_the_view_as_json() {
        let view = ProbeView::Show {
            id: 3,
            monitor: "Odyssey G5 «32»".into(),
            input: tandem_core::Input::Hdmi,
            ask: AskView::Candidate { code: 5 },
            timeout_ms: 8000,
            place: PlaceView::Monitor { screen: None },
        };
        let script = init_script(&view).unwrap();
        let json = script
            .strip_prefix("window.__TANDEM_PROBE__ = ")
            .and_then(|s| s.strip_suffix(';'))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(json).unwrap(),
            serde_json::to_value(&view).unwrap()
        );
    }

    proptest! {
        /// Si el lugar cae dentro de un monitor (en píxeles), se elige ese.
        #[test]
        fn a_place_inside_a_monitor_picks_it(
            widths in proptest::collection::vec(100u32..4000, 1..5),
            pick_at in any::<proptest::sample::Index>(),
        ) {
            let mut x = 0i32;
            let ms: Vec<Area> = widths
                .iter()
                .map(|w| {
                    let a = area(x, *w, 1.0, "");
                    x += *w as i32;
                    a
                })
                .collect();
            let i = pick_at.index(ms.len());
            let m = &ms[i];
            prop_assert_eq!(
                pick(screen(m.x, m.width, m.height), Units::Pixels, "", &ms),
                Some(i)
            );
        }
    }
}
