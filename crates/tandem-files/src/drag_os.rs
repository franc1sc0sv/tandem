//! Lo portable de los arrastres del sistema operativo (T-121): decisiones puras que las
//! implementaciones de Windows y macOS comparten, para probarlas en Linux.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use crate::drag::{DragDetector, DragEnd};

/// Qué hace `IDropSource::QueryContinueDrag` en cada vuelta del arrastre de Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropAction {
    Continue,
    Drop,
    Cancel,
}

/// Esc cancela; soltar el botón izquierdo suelta; si no, el arrastre sigue.
pub fn drop_action(escape: bool, left_down: bool) -> DropAction {
    if escape {
        DropAction::Cancel
    } else if !left_down {
        DropAction::Drop
    } else {
        DropAction::Continue
    }
}

/// Cómo terminó el arrastre: soltar donde nadie acepta la copia (efecto o operación vacíos)
/// es lo mismo que cancelar.
pub fn end_of(dropped: bool, accepted: bool) -> DragEnd {
    if dropped && accepted {
        DragEnd::Dropped
    } else {
        DragEnd::Cancelled
    }
}

/// La ruta de un `file://` del portapapeles de arrastre de macOS, con los `%xx` resueltos.
/// `None` si no es un archivo local o el texto no es UTF-8 válido.
pub fn parse_file_url(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // `file://localhost/ruta` y `file:///ruta` son lo mismo; otro host no es local.
    let path = match rest.strip_prefix("localhost") {
        Some(p) => p,
        None => rest,
    };
    if !path.starts_with('/') {
        return None;
    }
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = path.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let path = String::from_utf8(out).ok()?;
    // Las carpetas llegan con «/» al final.
    let trimmed = path.trim_end_matches('/');
    Some(PathBuf::from(if trimmed.is_empty() {
        "/"
    } else {
        trimmed
    }))
}

/// Lo que el destino de arrastre invisible del borde sabe del arrastre en curso. Lo escribe
/// el hilo de la ventana (Windows) y lo lee el agente por [`DragDetector`]; el doble de
/// prueba escribe lo mismo sin ventana.
#[derive(Debug, Clone, Default)]
pub struct DragProbe(Arc<Mutex<Option<Vec<PathBuf>>>>);

impl DragProbe {
    /// Entró un arrastre con `paths` (CF_HDROP). Sin rutas no es un arrastre de archivos.
    pub fn enter(&self, paths: Vec<PathBuf>) {
        self.set((!paths.is_empty()).then_some(paths));
    }

    /// El arrastre salió del borde, se soltó o se canceló.
    pub fn leave(&self) {
        self.set(None);
    }

    fn set(&self, v: Option<Vec<PathBuf>>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = v;
    }
}

impl DragDetector for DragProbe {
    fn file_drag(&self) -> Option<Vec<PathBuf>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Borde de la pantalla por el que se cruza.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripEdge {
    Left,
    Right,
    Top,
    Bottom,
}

/// Rectángulo en píxeles de pantalla `(x, y, ancho, alto)`.
pub type PxRect = (i32, i32, i32, i32);

/// La franja delgada, pegada por dentro al `edge` de `bounds`, donde vive la ventana
/// destino de arrastre. Por dentro: fuera del escritorio virtual nadie recibe un soltar.
pub fn strip_rect(bounds: PxRect, edge: StripEdge, thickness: i32) -> PxRect {
    let (x, y, w, h) = bounds;
    let t = thickness.clamp(1, w.min(h).max(1));
    match edge {
        StripEdge::Left => (x, y, t, h),
        StripEdge::Right => (x + w - t, y, t, h),
        StripEdge::Top => (x, y, w, t),
        StripEdge::Bottom => (x, y + h - t, w, t),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn esc_cancela_y_soltar_el_boton_suelta() {
        assert_eq!(drop_action(true, true), DropAction::Cancel);
        assert_eq!(drop_action(true, false), DropAction::Cancel);
        assert_eq!(drop_action(false, false), DropAction::Drop);
        assert_eq!(drop_action(false, true), DropAction::Continue);
    }

    #[test]
    fn soltar_sin_destino_que_acepte_es_cancelar() {
        assert_eq!(end_of(true, true), DragEnd::Dropped);
        assert_eq!(end_of(true, false), DragEnd::Cancelled);
        assert_eq!(end_of(false, true), DragEnd::Cancelled);
    }

    #[test]
    fn las_url_de_archivo_se_vuelven_rutas() {
        assert_eq!(
            parse_file_url("file:///Users/ana/Mi%20foto%C3%B1.png"),
            Some(PathBuf::from("/Users/ana/Mi fotoñ.png"))
        );
        assert_eq!(
            parse_file_url("file://localhost/tmp/x"),
            Some(PathBuf::from("/tmp/x"))
        );
        assert_eq!(
            parse_file_url("file:///Users/ana/carpeta/"),
            Some(PathBuf::from("/Users/ana/carpeta"))
        );
        assert_eq!(parse_file_url("file:///"), Some(PathBuf::from("/")));
    }

    #[test]
    fn lo_que_no_es_un_archivo_local_se_descarta() {
        for bad in [
            "https://ejemplo.com/a",
            "file://otro-host/a",
            "file:///a%zz",
            "file:///a%2",
            "file:///a%FF",
            "",
        ] {
            assert_eq!(parse_file_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn la_sonda_refleja_el_arrastre_de_archivos() {
        let p = DragProbe::default();
        assert_eq!(p.file_drag(), None);
        p.enter(vec![PathBuf::from("C:/a.txt")]);
        assert_eq!(p.file_drag(), Some(vec![PathBuf::from("C:/a.txt")]));
        let otra = p.clone();
        otra.leave();
        assert_eq!(p.file_drag(), None, "las copias comparten estado");
    }

    #[test]
    fn un_arrastre_sin_archivos_no_cuenta() {
        let p = DragProbe::default();
        p.enter(Vec::new());
        assert_eq!(p.file_drag(), None);
    }

    #[test]
    fn la_franja_queda_dentro_del_borde() {
        let b = (100, 50, 1920, 1080);
        assert_eq!(strip_rect(b, StripEdge::Left, 4), (100, 50, 4, 1080));
        assert_eq!(strip_rect(b, StripEdge::Right, 4), (2016, 50, 4, 1080));
        assert_eq!(strip_rect(b, StripEdge::Top, 4), (100, 50, 1920, 4));
        assert_eq!(strip_rect(b, StripEdge::Bottom, 4), (100, 1126, 1920, 4));
    }

    proptest! {
        #[test]
        fn la_franja_siempre_cabe_en_la_pantalla(
            x in -3000i32..3000, y in -3000i32..3000,
            w in 1i32..5000, h in 1i32..5000, t in -5i32..50, e in 0u8..4,
        ) {
            let edge = [StripEdge::Left, StripEdge::Right, StripEdge::Top, StripEdge::Bottom][e as usize];
            let (sx, sy, sw, sh) = strip_rect((x, y, w, h), edge, t);
            prop_assert!(sw >= 1 && sh >= 1);
            prop_assert!(sx >= x && sy >= y && sx + sw <= x + w && sy + sh <= y + h);
        }
    }

    /// Lo que codifica macOS: todo salvo los caracteres sin reservar queda en `%xx`.
    fn encode(path: &str) -> String {
        let mut out = String::from("file://");
        for b in path.bytes() {
            if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        out
    }

    proptest! {
        #[test]
        fn codificar_y_leer_devuelve_la_misma_ruta(
            parts in proptest::collection::vec("[^/\\x00]{1,12}", 1..5)
        ) {
            let path = format!("/{}", parts.join("/"));
            let expected = path.trim_end_matches('/').to_owned();
            prop_assert_eq!(parse_file_url(&encode(&path)), Some(PathBuf::from(expected)));
        }
    }
}
