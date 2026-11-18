//! Lo portable de los arrastres del sistema operativo (T-121): decisiones puras que las
//! implementaciones de Windows y macOS comparten, para probarlas en Linux.

use std::path::PathBuf;

use crate::drag::DragEnd;

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
