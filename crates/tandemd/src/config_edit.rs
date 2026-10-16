//! Cambios que el agente escribe en `tandem.toml` (T-080: los códigos DDC de un monitor
//! recién calibrado), sin tocar el resto del archivo: los comentarios y el orden del
//! usuario se conservan.
//!
//! El archivo nuevo se valida como configuración completa antes de escribirlo, y se
//! escribe aparte y se renombra: un error o un corte a medias dejan el anterior intacto.

use std::path::Path;

use tandem_core::{DdcProfile, MonitorId};
use toml_edit::{DocumentMut, Item, Value};

use crate::config::AgentConfig;

#[derive(Debug, thiserror::Error)]
pub enum ConfigEditError {
    #[error("no se pudo leer {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("tandem.toml no es TOML válido: {0}")]
    Parse(String),
    #[error("el monitor `{0}` no está en tandem.toml")]
    NoMonitor(MonitorId),
    #[error("el cambio dejaría una configuración inválida: {0}")]
    Invalid(String),
    #[error("no se pudo escribir {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// `ddc = { hdmi = 0x05, display_port = 0x0F }` (y `edid_model`, si se da y el monitor no
/// tenía) en el `[[monitors]]` de `id`, sobre el texto de `tandem.toml`.
pub fn with_monitor_ddc(
    text: &str,
    id: &MonitorId,
    ddc: &DdcProfile,
    edid_model: Option<&str>,
) -> Result<String, ConfigEditError> {
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| ConfigEditError::Parse(e.message().to_owned()))?;
    let ddc_value = ddc_value(ddc)?;
    let found = match doc.get_mut("monitors") {
        Some(Item::ArrayOfTables(monitors)) => monitors
            .iter_mut()
            .find(|t| t.get("id").and_then(Item::as_str) == Some(id.0.as_str()))
            .map(|t| {
                t.insert("ddc", Item::Value(ddc_value.clone()));
                if let Some(model) = edid_model
                    && !t.contains_key("edid_model")
                {
                    t.insert("edid_model", toml_edit::value(model));
                }
            }),
        Some(Item::Value(Value::Array(monitors))) => monitors
            .iter_mut()
            .filter_map(Value::as_inline_table_mut)
            .find(|t| t.get("id").and_then(Value::as_str) == Some(id.0.as_str()))
            .map(|t| {
                t.insert("ddc", ddc_value.clone());
                if let Some(model) = edid_model
                    && !t.contains_key("edid_model")
                {
                    t.insert("edid_model", model.into());
                }
            }),
        _ => None,
    };
    found.ok_or_else(|| ConfigEditError::NoMonitor(id.clone()))?;
    let out = doc.to_string();
    AgentConfig::from_toml(&out).map_err(|e| ConfigEditError::Invalid(e.to_string()))?;
    Ok(out)
}

/// Lo mismo sobre el archivo.
pub fn set_monitor_ddc(
    path: &Path,
    id: &MonitorId,
    ddc: &DdcProfile,
    edid_model: Option<&str>,
) -> Result<(), ConfigEditError> {
    let shown = path.display().to_string();
    let text = std::fs::read_to_string(path).map_err(|source| ConfigEditError::Read {
        path: shown.clone(),
        source,
    })?;
    let out = with_monitor_ddc(&text, id, ddc, edid_model)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, out)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|source| ConfigEditError::Write {
            path: shown,
            source,
        })
}

/// En hexadecimal, como se escriben a mano: se arma como texto y se lee con toml_edit
/// para que conserve esa forma.
fn ddc_value(ddc: &DdcProfile) -> Result<Value, ConfigEditError> {
    let snippet = format!(
        "ddc = {{ hdmi = {:#04X}, display_port = {:#04X} }}",
        ddc.hdmi, ddc.display_port
    );
    let doc: DocumentMut = snippet
        .parse()
        .map_err(|e: toml_edit::TomlError| ConfigEditError::Parse(e.message().to_owned()))?;
    doc.get("ddc")
        .and_then(Item::as_value)
        .cloned()
        .ok_or(ConfigEditError::Parse(snippet))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PC: &str = r#"# La PC.
machine = "pc"

[[monitors]]
id = "m34"
label = '34"'
edid_model = "LC34G55T"
wiring = { pc = "displayport", mac = "hdmi" }
ddc = { hdmi = 0x06, display_port = 0x09 }

# El 32": todavía sin códigos.
[[monitors]]
id = "m32"
label = '32"'
wiring = { pc = "displayport", mac = "hdmi" }
"#;

    const CODES: DdcProfile = DdcProfile {
        hdmi: 0x05,
        display_port: 0x0F,
    };

    #[test]
    fn writes_the_codes_and_keeps_everything_else() {
        let out = with_monitor_ddc(PC, &"m32".into(), &CODES, Some("SAM7489")).unwrap();
        assert!(out.starts_with("# La PC.\n"), "{out}");
        assert!(out.contains("# El 32\": todavía sin códigos."), "{out}");
        assert!(
            out.contains("ddc = { hdmi = 0x05, display_port = 0x0F }"),
            "{out}"
        );
        assert!(out.contains("edid_model = \"SAM7489\""), "{out}");
        // El otro monitor no cambia.
        assert!(out.contains("ddc = { hdmi = 0x06, display_port = 0x09 }"));
        let cfg = AgentConfig::from_toml(&out).unwrap();
        let m32 = cfg.desk.monitor(&"m32".into()).unwrap();
        assert_eq!(m32.ddc, Some(CODES));
        assert_eq!(m32.edid_model.as_deref(), Some("SAM7489"));
    }

    #[test]
    fn replaces_old_codes_and_keeps_the_model_that_was_there() {
        let out = with_monitor_ddc(PC, &"m34".into(), &CODES, Some("SAM711A")).unwrap();
        let cfg = AgentConfig::from_toml(&out).unwrap();
        let m34 = cfg.desk.monitor(&"m34".into()).unwrap();
        assert_eq!(m34.ddc, Some(CODES));
        assert_eq!(m34.edid_model.as_deref(), Some("LC34G55T"));
        assert_eq!(out.matches("ddc =").count(), 1, "{out}");
    }

    #[test]
    fn inline_monitor_arrays_work_too() {
        let text = "machine = \"pc\"\nmonitors = [{ id = \"a\", label = \"a\", wiring = { pc = \"hdmi\" } }]\n";
        let out = with_monitor_ddc(text, &"a".into(), &CODES, None).unwrap();
        let cfg = AgentConfig::from_toml(&out).unwrap();
        assert_eq!(cfg.desk.monitor(&"a".into()).unwrap().ddc, Some(CODES));
    }

    #[test]
    fn errors_leave_the_file_alone() {
        assert!(matches!(
            with_monitor_ddc(PC, &"m99".into(), &CODES, None),
            Err(ConfigEditError::NoMonitor(_))
        ));
        assert!(matches!(
            with_monitor_ddc("machine = ", &"m32".into(), &CODES, None),
            Err(ConfigEditError::Parse(_))
        ));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tandem.toml");
        std::fs::write(&path, PC).unwrap();
        assert!(set_monitor_ddc(&path, &"m99".into(), &CODES, None).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), PC);
        set_monitor_ddc(&path, &"m32".into(), &CODES, None).unwrap();
        let cfg = AgentConfig::load(&path).unwrap();
        assert_eq!(cfg.desk.monitor(&"m32".into()).unwrap().ddc, Some(CODES));
        assert!(!dir.path().join("tandem.toml.tmp").exists());
        assert!(matches!(
            set_monitor_ddc(&dir.path().join("no.toml"), &"m32".into(), &CODES, None),
            Err(ConfigEditError::Read { .. })
        ));
    }
}
