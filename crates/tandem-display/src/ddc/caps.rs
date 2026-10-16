//! Parser del capabilities string de DDC/CI (MCCS 2.x).
//!
//! El formato es una lista de `clave(valor)` dentro de un par de paréntesis. Los monitores
//! reales se salen del estándar: el LC34G55T mete `SAMSUNG` suelto antes de `cmds(`, deja
//! espacios dentro de las listas y repite paréntesis. Por eso el parser es tolerante con la
//! forma y estricto solo con lo que importa (los códigos VCP y sus valores).

use std::collections::BTreeMap;

/// Claves conocidas de MCCS. Sirven para separar texto suelto pegado a una clave
/// (`SAMSUNGcmds(` → texto `SAMSUNG` + clave `cmds`).
const KNOWN_KEYS: &[&str] = &[
    "prot",
    "type",
    "model",
    "cmds",
    "vcp",
    "vcpname",
    "mccs_ver",
    "mswhql",
    "asset_eep",
    "mpu",
    "window",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapsError {
    #[error("capabilities vacío")]
    Empty,
    #[error("paréntesis sin cerrar (abierto en la posición {0})")]
    Unclosed(usize),
    #[error("paréntesis de cierre sobrante en la posición {0}")]
    UnexpectedClose(usize),
    #[error("valor hexadecimal inválido `{token}` en `{key}`")]
    BadHex { key: String, token: String },
    #[error("anidamiento inesperado en `vcp` (código {0:#04X})")]
    TooDeep(u8),
}

/// Capabilities ya interpretado.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub prot: Option<String>,
    /// `type(...)`: normalmente `lcd` o `crt`.
    pub display_type: Option<String>,
    pub model: Option<String>,
    pub mccs_ver: Option<String>,
    /// Comandos DDC/CI soportados (`cmds`).
    pub cmds: Vec<u8>,
    /// Código VCP → valores declarados. Vacío si el código no enumera valores
    /// (controles continuos como brillo).
    pub vcp: BTreeMap<u8, Vec<u8>>,
    /// Texto suelto fuera de cualquier `clave(...)`, por ejemplo `SAMSUNG`.
    pub bare: Vec<String>,
    /// Pares `clave(valor)` que no se interpretan, en el orden en que llegaron.
    pub other: Vec<(String, String)>,
}

impl Capabilities {
    pub fn supports(&self, vcp: u8) -> bool {
        self.vcp.contains_key(&vcp)
    }

    /// Valores declarados para un código. Ojo: declarar no es obedecer (ver
    /// `docs/research/ddc-34-LC34G55T.md`), así que esto es informativo.
    pub fn declared_values(&self, vcp: u8) -> Option<&[u8]> {
        self.vcp.get(&vcp).map(Vec::as_slice)
    }
}

/// Interpreta un capabilities string tal como lo devuelve el monitor.
pub fn parse(raw: &str) -> Result<Capabilities, CapsError> {
    let s = raw.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if s.is_empty() {
        return Err(CapsError::Empty);
    }
    let body = strip_outer(s)?;
    let mut caps = Capabilities::default();
    for item in split_items(body)? {
        match item {
            Item::Bare(text) => caps.bare.push(text),
            Item::Pair(key, value) => apply(&mut caps, key, value)?,
        }
    }
    Ok(caps)
}

enum Item<'a> {
    Bare(String),
    Pair(String, &'a str),
}

/// Quita el par de paréntesis exterior si envuelve todo el string.
fn strip_outer(s: &str) -> Result<&str, CapsError> {
    if !s.starts_with('(') {
        return Ok(s);
    }
    let close = matching_close(s, 0)?;
    if close == s.len() - 1 {
        Ok(&s[1..close])
    } else {
        // El primer paréntesis no envuelve todo: no había paréntesis exterior.
        Ok(s)
    }
}

/// Índice del `)` que cierra el `(` en `open`.
fn matching_close(s: &str, open: usize) -> Result<usize, CapsError> {
    let mut depth = 0usize;
    for (i, b) in s.bytes().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            _ => {}
        }
    }
    Err(CapsError::Unclosed(open))
}

fn split_items(body: &str) -> Result<Vec<Item<'_>>, CapsError> {
    let mut items = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let bytes = body.as_bytes();
    while i < bytes.len() {
        match bytes[i] {
            b'(' => {
                let close = matching_close(body, i)?;
                let (bare, key) = split_key(body[start..i].trim());
                if let Some(bare) = bare {
                    items.push(Item::Bare(bare));
                }
                items.push(Item::Pair(key, &body[i + 1..close]));
                i = close + 1;
                start = i;
            }
            b')' => return Err(CapsError::UnexpectedClose(i)),
            _ => i += 1,
        }
    }
    let rest = body[start..].trim();
    if !rest.is_empty() {
        items.push(Item::Bare(rest.to_owned()));
    }
    Ok(items)
}

/// Separa el texto que precede a `(` en (texto suelto, clave).
fn split_key(text: &str) -> (Option<String>, String) {
    let lower = text.to_ascii_lowercase();
    // La clave conocida más larga que cierra el texto; así `vcpname` gana a `name`.
    let known = KNOWN_KEYS
        .iter()
        .filter(|k| lower.ends_with(*k))
        .max_by_key(|k| k.len());
    let split_at = match known {
        Some(k) => text.len() - k.len(),
        // Clave desconocida: la última palabra es la clave, lo anterior es texto suelto.
        None => text.rfind(char::is_whitespace).map_or(0, |p| p + 1),
    };
    let bare = text[..split_at].trim();
    let key = text[split_at..].trim().to_ascii_lowercase();
    ((!bare.is_empty()).then(|| bare.to_owned()), key)
}

fn apply(caps: &mut Capabilities, key: String, value: &str) -> Result<(), CapsError> {
    let text = || Some(value.trim().to_owned());
    match key.as_str() {
        "prot" => caps.prot = text(),
        "type" => caps.display_type = text(),
        "model" => caps.model = text(),
        "mccs_ver" => caps.mccs_ver = text(),
        "cmds" => caps.cmds.extend(hex_list(&key, value)?),
        "vcp" => parse_vcp(&mut caps.vcp, value)?,
        _ => caps.other.push((key, value.trim().to_owned())),
    }
    Ok(())
}

/// `vcp(02 04 14(05 08) 60( 12 0F 10) ...)`.
fn parse_vcp(vcp: &mut BTreeMap<u8, Vec<u8>>, value: &str) -> Result<(), CapsError> {
    let mut last: Option<u8> = None;
    let mut rest = value;
    while !rest.is_empty() {
        let next = rest.find('(').unwrap_or(rest.len());
        for code in hex_list("vcp", &rest[..next])? {
            vcp.entry(code).or_default();
            last = Some(code);
        }
        if next == rest.len() {
            break;
        }
        let close = matching_close(rest, next)?;
        let inner = &rest[next + 1..close];
        // Un `(` siempre enumera los valores del código anterior.
        let code = last.ok_or_else(|| CapsError::BadHex {
            key: "vcp".into(),
            token: format!("({inner})"),
        })?;
        if inner.contains('(') {
            return Err(CapsError::TooDeep(code));
        }
        vcp.entry(code).or_default().extend(hex_list("vcp", inner)?);
        // Los valores son de `code`; un segundo `(` seguido no puede ser suyo.
        last = None;
        rest = &rest[close + 1..];
    }
    Ok(())
}

/// Lista de bytes en hexadecimal separados por espacios. Algunos monitores los
/// mandan pegados (`0204`), así que un token largo se parte de dos en dos.
fn hex_list(key: &str, s: &str) -> Result<Vec<u8>, CapsError> {
    let mut out = Vec::new();
    for token in s.split_whitespace() {
        let bad = || CapsError::BadHex {
            key: key.to_owned(),
            token: token.to_owned(),
        };
        if token.len() % 2 != 0 && token.len() != 1 {
            return Err(bad());
        }
        if token.len() == 1 {
            out.push(u8::from_str_radix(token, 16).map_err(|_| bad())?);
            continue;
        }
        for pair in token.as_bytes().chunks(2) {
            let pair = std::str::from_utf8(pair).map_err(|_| bad())?;
            out.push(u8::from_str_radix(pair, 16).map_err(|_| bad())?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tal cual lo devuelve el LC34G55T (docs/research/ddc-34-LC34G55T.md), en una línea.
    const LC34G55T: &str = "(prot(monitor)type(lcd)SAMSUNGcmds(01 02 03 07 0C E3 F3)vcp(02 04 05 08 10 12 14(05 08 0B 0C) 16 18 1A 52 60( 12 0F 10) AA(01 02 03 FF) AC AE B2 B6 C6 C8 C9 D6(01 04 05) DC(00 02 03 05 ) DF FD)mccs_ver(2.1)mswhql(1))";

    #[test]
    fn parses_real_lc34g55t_string() {
        let caps = parse(LC34G55T).unwrap();
        assert_eq!(caps.prot.as_deref(), Some("monitor"));
        assert_eq!(caps.display_type.as_deref(), Some("lcd"));
        assert_eq!(caps.mccs_ver.as_deref(), Some("2.1"));
        assert_eq!(caps.bare, vec!["SAMSUNG".to_owned()]);
        assert_eq!(caps.cmds, vec![0x01, 0x02, 0x03, 0x07, 0x0C, 0xE3, 0xF3]);
        assert_eq!(caps.other, vec![("mswhql".to_owned(), "1".to_owned())]);

        let codes: Vec<u8> = caps.vcp.keys().copied().collect();
        assert_eq!(
            codes,
            vec![
                0x02, 0x04, 0x05, 0x08, 0x10, 0x12, 0x14, 0x16, 0x18, 0x1A, 0x52, 0x60, 0xAA, 0xAC,
                0xAE, 0xB2, 0xB6, 0xC6, 0xC8, 0xC9, 0xD6, 0xDC, 0xDF, 0xFD
            ]
        );
        assert_eq!(caps.declared_values(0x60), Some(&[0x12, 0x0F, 0x10][..]));
        assert_eq!(
            caps.declared_values(0x14),
            Some(&[0x05, 0x08, 0x0B, 0x0C][..])
        );
        assert_eq!(
            caps.declared_values(0xAA),
            Some(&[0x01, 0x02, 0x03, 0xFF][..])
        );
        assert_eq!(caps.declared_values(0xD6), Some(&[0x01, 0x04, 0x05][..]));
        assert_eq!(
            caps.declared_values(0xDC),
            Some(&[0x00, 0x02, 0x03, 0x05][..])
        );
        assert_eq!(caps.declared_values(0x10), Some(&[][..]));
        assert!(caps.supports(0xFD));
        assert!(!caps.supports(0x62));
    }

    #[test]
    fn parses_real_string_with_line_breaks_and_trailing_nul() {
        // Como aparece en la nota de investigación, partido en varias líneas.
        let multiline = "(prot(monitor)type(lcd)SAMSUNGcmds(01 02 03 07 0C E3 F3)\n vcp(02 04 05 08 10 12 14(05 08 0B 0C) 16 18 1A 52 60( 12 0F 10) AA(01 02 03 FF)\n     AC AE B2 B6 C6 C8 C9 D6(01 04 05) DC(00 02 03 05 ) DF FD)\n mccs_ver(2.1)mswhql(1))\0";
        assert_eq!(parse(multiline).unwrap(), parse(LC34G55T).unwrap());
    }

    #[test]
    fn accepts_missing_outer_parens_and_model() {
        let caps = parse("prot(monitor) model(U2720Q) vcp(10 12 60(0F 11))").unwrap();
        assert_eq!(caps.model.as_deref(), Some("U2720Q"));
        assert_eq!(caps.declared_values(0x60), Some(&[0x0F, 0x11][..]));
        assert!(caps.bare.is_empty());
    }

    #[test]
    fn splits_concatenated_hex() {
        let caps = parse("(vcp(021012 60(0F11)))").unwrap();
        let codes: Vec<u8> = caps.vcp.keys().copied().collect();
        assert_eq!(codes, vec![0x02, 0x10, 0x12, 0x60]);
        assert_eq!(caps.declared_values(0x60), Some(&[0x0F, 0x11][..]));
    }

    #[test]
    fn vcpname_is_not_confused_with_other_keys() {
        let caps = parse("(vcp(10)vcpname(10(Brillo)))").unwrap();
        assert!(caps.supports(0x10));
        assert_eq!(
            caps.other,
            vec![("vcpname".to_owned(), "10(Brillo)".to_owned())]
        );
    }

    #[test]
    fn unknown_key_after_bare_word() {
        let caps = parse("(ACME foo(bar))").unwrap();
        assert_eq!(caps.bare, vec!["ACME".to_owned()]);
        assert_eq!(caps.other, vec![("foo".to_owned(), "bar".to_owned())]);
    }

    #[test]
    fn rejects_malformed_strings() {
        assert_eq!(parse("  \0"), Err(CapsError::Empty));
        assert!(matches!(parse("(vcp(10 12)"), Err(CapsError::Unclosed(_))));
        assert!(matches!(
            parse("vcp(10))"),
            Err(CapsError::UnexpectedClose(_))
        ));
        assert!(matches!(parse("(vcp(1G))"), Err(CapsError::BadHex { .. })));
        assert!(matches!(parse("(vcp(123))"), Err(CapsError::BadHex { .. })));
        assert!(matches!(
            parse("(vcp((01)))"),
            Err(CapsError::BadHex { .. })
        ));
        assert_eq!(parse("(vcp(60(0F(1))))"), Err(CapsError::TooDeep(0x60)));
        assert!(matches!(parse("(cmds(ZZ))"), Err(CapsError::BadHex { .. })));
    }

    #[test]
    fn values_never_attach_to_a_previous_group() {
        // `14(05)(06)`: el segundo grupo no tiene código propio.
        assert!(parse("(vcp(14(05)(06)))").is_err());
    }
}
