//! Rutas normalizadas entre Windows y Unix.
//!
//! En el cable una ruta es relativa a la raíz de la copia, con `/` como separador y sin
//! `.` ni `..`. Quien recibe la une a su carpeta de destino con el separador de su sistema
//! y rechaza lo que su sistema no puede guardar (en Windows, `:`, `\`, `?`, nombres
//! reservados como `CON`, puntos o espacios al final).

use std::path::{Component, Path, PathBuf};

use crate::error::{FilesError, Result};

const WINDOWS_FORBIDDEN: &[char] = &['<', '>', ':', '"', '|', '?', '*', '\\'];
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Un nombre de archivo o carpeta, sin separadores. `windows` dice qué reglas aplicar.
pub fn check_segment(seg: &str, windows: bool) -> std::result::Result<(), &'static str> {
    if seg.is_empty() {
        return Err("componente vacío");
    }
    if seg == "." || seg == ".." {
        return Err("`.` y `..` no se aceptan");
    }
    if seg.contains('/') || seg.contains('\0') {
        return Err("el nombre trae un separador o un byte nulo");
    }
    if windows {
        if seg
            .chars()
            .any(|c| c.is_control() || WINDOWS_FORBIDDEN.contains(&c))
        {
            return Err("el nombre trae caracteres que Windows no admite");
        }
        if seg.ends_with('.') || seg.ends_with(' ') {
            return Err("Windows no admite nombres que terminan en punto o espacio");
        }
        let stem = seg.split('.').next().unwrap_or(seg).to_ascii_uppercase();
        if WINDOWS_RESERVED.contains(&stem.as_str()) {
            return Err("nombre reservado de Windows");
        }
    }
    Ok(())
}

/// De una ruta relativa nativa a la del cable (`a/b/c`).
pub fn to_wire(rel: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => {
                let s = s
                    .to_str()
                    .ok_or_else(|| FilesError::path(&rel.to_string_lossy(), "no es UTF-8"))?;
                // Un nombre de Unix con `/` es imposible, pero con `\` o `:` es válido allá
                // y lo rechazará quien lo reciba en Windows.
                check_segment(s, false).map_err(|w| FilesError::path(s, w))?;
                parts.push(s);
            }
            Component::CurDir => {}
            _ => return Err(FilesError::path(&rel.to_string_lossy(), "no es relativa")),
        }
    }
    Ok(parts.join("/"))
}

/// De la ruta del cable a una nativa bajo `root`, con las reglas de este sistema.
pub fn from_wire(root: &Path, wire: &str) -> Result<PathBuf> {
    from_wire_for(root, wire, cfg!(windows))
}

pub fn from_wire_for(root: &Path, wire: &str, windows: bool) -> Result<PathBuf> {
    let mut out = root.to_path_buf();
    for seg in wire.split('/') {
        check_segment(seg, windows).map_err(|w| FilesError::path(wire, w))?;
        out.push(seg);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn la_ruta_del_cable_usa_barras_y_se_une_a_la_raiz() {
        let p = from_wire_for(Path::new("/dst"), "a b/ñandú/x.txt", false).unwrap();
        assert_eq!(p, Path::new("/dst/a b/ñandú/x.txt"));
        assert_eq!(
            to_wire(Path::new("a b/ñandú/x.txt")).unwrap(),
            "a b/ñandú/x.txt"
        );
    }

    #[test]
    fn rechaza_lo_que_escapa_de_la_raiz() {
        for bad in [
            "",
            "..",
            "a/../b",
            "/etc/passwd",
            "a//b",
            "./a",
            "a/",
            "a\0b",
        ] {
            assert!(
                from_wire_for(Path::new("/dst"), bad, false).is_err(),
                "{bad:?}"
            );
        }
        assert!(to_wire(Path::new("/abs")).is_err());
        assert!(to_wire(Path::new("../x")).is_err());
    }

    #[test]
    fn windows_rechaza_lo_que_no_puede_guardar() {
        for bad in ["a:b", "a\\b", "CON", "con.txt", "x?", "fin.", "fin ", "a|b"] {
            assert!(
                from_wire_for(Path::new("C:\\d"), bad, true).is_err(),
                "{bad:?}"
            );
        }
        // En Unix los mismos nombres son válidos, salvo los del cable.
        for ok in ["a:b", "a\\b", "CON", "fin."] {
            assert!(from_wire_for(Path::new("/d"), ok, false).is_ok(), "{ok:?}");
        }
    }

    proptest! {
        /// Cualquier ruta que sale de `to_wire` vuelve igual por `from_wire` y no escapa de
        /// la raíz.
        #[test]
        fn ida_y_vuelta_sin_escapar(segs in proptest::collection::vec("[a-zA-Z0-9 ñáéí_.-]{1,12}", 1..5)) {
            let segs: Vec<String> = segs.into_iter().filter(|s| s != "." && s != "..").collect();
            prop_assume!(!segs.is_empty());
            let rel: PathBuf = segs.iter().collect();
            let wire = to_wire(&rel).unwrap();
            let back = from_wire_for(Path::new("/root"), &wire, false).unwrap();
            prop_assert!(back.starts_with("/root"));
            prop_assert_eq!(back.strip_prefix("/root").unwrap(), rel.as_path());
        }
    }
}
