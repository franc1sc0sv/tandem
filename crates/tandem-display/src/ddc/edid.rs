//! Lectura del bloque base del EDID (128 bytes) para identificar un monitor por modelo.
//!
//! El nombre que da Windows a un monitor físico suele ser "Monitor PnP genérico", así que
//! la identidad real (fabricante, producto, nombre, serie) sale del EDID.

use std::fmt;

const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
const BASE_LEN: usize = 128;
const DESCRIPTORS: [usize; 4] = [54, 72, 90, 108];
const TAG_SERIAL: u8 = 0xFF;
const TAG_NAME: u8 = 0xFC;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EdidError {
    #[error("EDID demasiado corto ({0} bytes)")]
    TooShort(usize),
    #[error("encabezado de EDID inválido")]
    BadHeader,
    #[error("checksum de EDID inválido")]
    BadChecksum,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edid {
    /// Código PnP de tres letras (`SAM` para Samsung).
    pub manufacturer: String,
    pub product_code: u16,
    pub serial_number: u32,
    /// Descriptor de nombre (0xFC), por ejemplo `LC34G55T`.
    pub name: Option<String>,
    /// Descriptor de número de serie en texto (0xFF).
    pub serial_text: Option<String>,
    /// Tamaño de la imagen en cm (ancho, alto), bytes 21-22. `None` si el monitor no lo da
    /// (proyectores, o cuando el EDID guarda una relación de aspecto en vez del tamaño).
    pub size_cm: Option<(u8, u8)>,
}

impl Edid {
    /// Identificador PnP como lo muestra Windows: `SAM7140`.
    pub fn pnp_id(&self) -> String {
        format!("{}{:04X}", self.manufacturer, self.product_code)
    }

    /// La diagonal en pulgadas enteras, como se venden los monitores (`34`).
    pub fn diagonal_inches(&self) -> Option<u32> {
        let (w, h) = self.size_cm?;
        let cm = (f64::from(w).powi(2) + f64::from(h).powi(2)).sqrt();
        Some((cm / 2.54).round() as u32)
    }
}

impl fmt::Display for Edid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.pnp_id())?;
        if let Some(name) = &self.name {
            write!(f, " {name}")?;
        }
        Ok(())
    }
}

pub fn parse(bytes: &[u8]) -> Result<Edid, EdidError> {
    if bytes.len() < BASE_LEN {
        return Err(EdidError::TooShort(bytes.len()));
    }
    let base = &bytes[..BASE_LEN];
    if base[..8] != HEADER {
        return Err(EdidError::BadHeader);
    }
    if base.iter().fold(0u8, |acc, b| acc.wrapping_add(*b)) != 0 {
        return Err(EdidError::BadChecksum);
    }
    let mut edid = Edid {
        manufacturer: manufacturer(u16::from_be_bytes([base[8], base[9]])),
        product_code: u16::from_le_bytes([base[10], base[11]]),
        serial_number: u32::from_le_bytes([base[12], base[13], base[14], base[15]]),
        name: None,
        serial_text: None,
        // Con uno de los dos en 0, los bytes son una relación de aspecto, no un tamaño.
        size_cm: (base[21] != 0 && base[22] != 0).then_some((base[21], base[22])),
    };
    for offset in DESCRIPTORS {
        let d = &base[offset..offset + 18];
        // Los descriptores de pantalla empiezan con 00 00 (si no, es un timing).
        if d[0] != 0 || d[1] != 0 {
            continue;
        }
        let slot = match d[3] {
            TAG_NAME => &mut edid.name,
            TAG_SERIAL => &mut edid.serial_text,
            _ => continue,
        };
        if slot.is_none() {
            *slot = descriptor_text(&d[5..18]);
        }
    }
    Ok(edid)
}

/// Código PnP a partir de la palabra del EDID (bytes 8-9, big endian): tres letras de
/// 5 bits cada una, `A` = 1. macOS da la misma palabra en `CGDisplayVendorNumber`.
pub fn manufacturer(word: u16) -> String {
    [10u16, 5, 0]
        .iter()
        .map(|shift| {
            let v = ((word >> shift) & 0x1F) as u8;
            if (1..=26).contains(&v) {
                char::from(b'A' + v - 1)
            } else {
                '?'
            }
        })
        .collect()
}

/// Texto de un descriptor: termina en 0x0A y se rellena con espacios.
fn descriptor_text(raw: &[u8]) -> Option<String> {
    let end = raw.iter().position(|b| *b == 0x0A).unwrap_or(raw.len());
    let text: String = raw[..end]
        .iter()
        .map(|b| {
            if b.is_ascii_graphic() || *b == b' ' {
                char::from(*b)
            } else {
                '?'
            }
        })
        .collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Subclave del registro (bajo `HKLM`) con los parámetros del monitor, a partir de la
/// ruta de interfaz que devuelve `EnumDisplayDevicesW(.., EDD_GET_DEVICE_INTERFACE_NAME)`:
///
/// `\\?\DISPLAY#SAM7140#5&2a1b3c&0&UID4352#{e6f07b5f-...}` →
/// `SYSTEM\CurrentControlSet\Enum\DISPLAY\SAM7140\5&2a1b3c&0&UID4352\Device Parameters`.
pub fn registry_key_from_interface(path: &str) -> Option<String> {
    let rest = path
        .strip_prefix(r"\\?\")
        .or_else(|| path.strip_prefix(r"\\.\"))?;
    let mut parts = rest.split('#');
    let class = parts.next().filter(|p| !p.is_empty())?;
    let hardware = parts.next().filter(|p| !p.is_empty())?;
    let instance = parts.next().filter(|p| !p.is_empty())?;
    // El cuarto trozo es el GUID de la interfaz; si falta, la ruta no es de interfaz.
    parts.next().filter(|p| p.starts_with('{'))?;
    Some(format!(
        r"SYSTEM\CurrentControlSet\Enum\{class}\{hardware}\{instance}\Device Parameters"
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// EDID sintético con nombre y serie, checksum correcto.
    pub(crate) fn sample(name: &str, serial: &str) -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&HEADER);
        // "SAM": S=19, A=1, M=13.
        let word: u16 = (19 << 10) | (1 << 5) | 13;
        e[8..10].copy_from_slice(&word.to_be_bytes());
        e[10..12].copy_from_slice(&0x7140u16.to_le_bytes());
        e[12..16].copy_from_slice(&0x0102_0304u32.to_le_bytes());
        // Primer descriptor: un timing (no empieza con 00 00).
        e[54] = 0x01;
        e[55] = 0x1D;
        put_text(&mut e[72..90], TAG_NAME, name);
        put_text(&mut e[90..108], TAG_SERIAL, serial);
        // Rango de frecuencias (0xFD): debe ignorarse.
        e[111] = 0xFD;
        fix_checksum(&mut e);
        e
    }

    fn put_text(d: &mut [u8], tag: u8, text: &str) {
        d[3] = tag;
        let field = &mut d[5..18];
        field.fill(b' ');
        let n = text.len().min(13);
        field[..n].copy_from_slice(&text.as_bytes()[..n]);
        if n < 13 {
            field[n] = 0x0A;
        }
    }

    fn fix_checksum(e: &mut [u8]) {
        let sum = e[..127].iter().fold(0u8, |acc, b| acc.wrapping_add(*b));
        e[127] = 0u8.wrapping_sub(sum);
    }

    #[test]
    fn parses_identity() {
        let edid = parse(&sample("LC34G55T", "H4ZR900123")).unwrap();
        assert_eq!(edid.manufacturer, "SAM");
        assert_eq!(edid.product_code, 0x7140);
        assert_eq!(edid.serial_number, 0x0102_0304);
        assert_eq!(edid.pnp_id(), "SAM7140");
        assert_eq!(edid.name.as_deref(), Some("LC34G55T"));
        assert_eq!(edid.serial_text.as_deref(), Some("H4ZR900123"));
        assert_eq!(edid.to_string(), "SAM7140 LC34G55T");
    }

    #[test]
    fn reads_the_screen_size() {
        let mut e = sample("LC34G55T", "S");
        assert_eq!(parse(&e).unwrap().size_cm, None);
        assert_eq!(parse(&e).unwrap().diagonal_inches(), None);
        // El 34" ultrawide: 80 x 34 cm.
        e[21] = 80;
        e[22] = 34;
        fix_checksum(&mut e);
        let edid = parse(&e).unwrap();
        assert_eq!(edid.size_cm, Some((80, 34)));
        assert_eq!(edid.diagonal_inches(), Some(34));
        // Relación de aspecto (alto en 0): no es un tamaño.
        e[22] = 0;
        fix_checksum(&mut e);
        assert_eq!(parse(&e).unwrap().size_cm, None);
    }

    #[test]
    fn name_filling_all_13_bytes_has_no_terminator() {
        let edid = parse(&sample("ODYSSEY G5 34", "X")).unwrap();
        assert_eq!(edid.name.as_deref(), Some("ODYSSEY G5 34"));
    }

    #[test]
    fn accepts_extension_blocks() {
        let mut e = sample("LC34G55T", "S");
        e.extend_from_slice(&[0x02; 128]);
        assert_eq!(parse(&e).unwrap().name.as_deref(), Some("LC34G55T"));
    }

    #[test]
    fn without_descriptors_there_is_no_name() {
        let mut e = sample("X", "Y");
        e[75] = 0x10;
        e[93] = 0x10;
        fix_checksum(&mut e);
        let edid = parse(&e).unwrap();
        assert_eq!(edid.name, None);
        assert_eq!(edid.serial_text, None);
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(parse(&[0; 10]), Err(EdidError::TooShort(10)));
        let mut e = sample("A", "B");
        e[0] = 0x01;
        assert_eq!(parse(&e), Err(EdidError::BadHeader));
        let mut e = sample("A", "B");
        e[20] ^= 0xFF;
        assert_eq!(parse(&e), Err(EdidError::BadChecksum));
    }

    #[test]
    fn invalid_manufacturer_letters_are_marked() {
        assert_eq!(manufacturer(0), "???");
    }

    #[test]
    fn registry_key_from_interface_path() {
        assert_eq!(
            registry_key_from_interface(
                r"\\?\DISPLAY#SAM7140#5&2a1b3c&0&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
            )
            .as_deref(),
            Some(
                r"SYSTEM\CurrentControlSet\Enum\DISPLAY\SAM7140\5&2a1b3c&0&UID4352\Device Parameters"
            )
        );
        assert_eq!(
            registry_key_from_interface(r"MONITOR\SAM7140\{4d36e96e}\0001"),
            None
        );
        assert_eq!(registry_key_from_interface(r"\\?\DISPLAY#SAM7140"), None);
        assert_eq!(
            registry_key_from_interface(r"\\?\DISPLAY#SAM7140#5&2a#nope"),
            None
        );
        assert_eq!(registry_key_from_interface(r"\\?\##x#{g}"), None);
    }
}
