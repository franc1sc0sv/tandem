//! Cambios que el agente escribe en `tandem.toml` (T-080: los códigos DDC de un monitor
//! recién calibrado; T-082: el cableado elegido en la UI; T-084: el puesto que llegó de la
//! otra máquina), sin tocar el resto del archivo: los comentarios y el orden del usuario
//! se conservan.
//!
//! El archivo nuevo se valida como configuración completa antes de escribirlo, y se
//! escribe aparte y se renombra: un error o un corte a medias dejan el anterior intacto.

use std::collections::BTreeMap;
use std::path::Path;

use tandem_core::{DdcProfile, Input, Machine, MonitorConfig, MonitorId};
use tandem_proto::DeskSync;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, TableLike, Value};

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
    let ddc_value = ddc_value(ddc)?;
    edit_monitor(text, id, |t| {
        t.insert("ddc", Item::Value(ddc_value));
        if let Some(model) = edid_model
            && !t.contains_key("edid_model")
        {
            t.insert("edid_model", toml_edit::value(model));
        }
    })
}

/// Lo mismo sobre el archivo.
pub fn set_monitor_ddc(
    path: &Path,
    id: &MonitorId,
    ddc: &DdcProfile,
    edid_model: Option<&str>,
) -> Result<(), ConfigEditError> {
    rewrite(path, |text| with_monitor_ddc(text, id, ddc, edid_model))
}

/// `wiring = { pc = "displayport", mac = "hdmi" }` en el `[[monitors]]` de `id`.
pub fn with_monitor_wiring(
    text: &str,
    id: &MonitorId,
    wiring: &BTreeMap<Machine, Input>,
) -> Result<String, ConfigEditError> {
    let value = wiring_value(wiring)?;
    edit_monitor(text, id, |t| {
        t.insert("wiring", Item::Value(value));
    })
}

/// Lo mismo sobre el archivo.
pub fn set_monitor_wiring(
    path: &Path,
    id: &MonitorId,
    wiring: &BTreeMap<Machine, Input>,
) -> Result<(), ConfigEditError> {
    rewrite(path, |text| with_monitor_wiring(text, id, wiring))
}

/// `keyboard = "pc"` en `[input]`: dónde está el teclado y el mouse, dicho por el usuario
/// en el asistente (T-087).
pub fn with_keyboard(text: &str, machine: Machine) -> Result<String, ConfigEditError> {
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| ConfigEditError::Parse(e.message().to_owned()))?;
    let input = doc
        .entry("input")
        .or_insert_with(|| Item::Table(Table::new()));
    let Some(input) = input.as_table_like_mut() else {
        return Err(ConfigEditError::Invalid("[input] no es una tabla".into()));
    };
    // El nombre de antes (`server`) valdría lo mismo; se deja uno solo.
    input.remove("server");
    input.insert("keyboard", toml_edit::value(machine.to_string()));
    let out = doc.to_string();
    AgentConfig::from_toml(&out).map_err(|e| ConfigEditError::Invalid(e.to_string()))?;
    Ok(out)
}

/// Lo mismo sobre el archivo.
pub fn set_keyboard(path: &Path, machine: Machine) -> Result<(), ConfigEditError> {
    rewrite(path, |text| with_keyboard(text, machine))
}

/// El puesto compartido de `sync` sobre el texto de `tandem.toml` (T-084): los
/// `[[monitors]]` (se actualizan en su lugar los que ya estaban, se agregan los nuevos al
/// final y se quitan los que ya no están), `[input] lock_shortcut`, `desk_rev` y
/// `desk_origin`. Lo solo-local (máquina, red, energía, el resto de `[input]`) no se toca.
pub fn with_desk(text: &str, sync: &DeskSync) -> Result<String, ConfigEditError> {
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| ConfigEditError::Parse(e.message().to_owned()))?;
    let mut old: Vec<Table> = match doc.remove("monitors") {
        Some(Item::ArrayOfTables(monitors)) => monitors.into_iter().collect(),
        Some(Item::Value(Value::Array(monitors))) => monitors
            .into_iter()
            .filter_map(|v| match v {
                Value::InlineTable(t) => Some(t.into_table()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut monitors = ArrayOfTables::new();
    for m in &sync.desk.monitors {
        let at = old
            .iter()
            .position(|t| t.get("id").and_then(Item::as_str) == Some(m.id.0.as_str()));
        let mut table = at.map(|i| old.remove(i)).unwrap_or_default();
        fill_monitor(&mut table, m)?;
        monitors.push(table);
    }
    if monitors.is_empty() {
        doc.insert(
            "monitors",
            Item::Value(Value::Array(toml_edit::Array::new())),
        );
    } else {
        doc.insert("monitors", Item::ArrayOfTables(monitors));
    }
    let input = doc
        .entry("input")
        .or_insert_with(|| Item::Table(Table::new()));
    let Some(input) = input.as_table_like_mut() else {
        return Err(ConfigEditError::Invalid("[input] no es una tabla".into()));
    };
    let keys: toml_edit::Array = sync.lock_shortcut.iter().map(String::as_str).collect();
    input.insert("lock_shortcut", Item::Value(Value::Array(keys)));
    doc.insert(
        "desk_rev",
        toml_edit::value(i64::try_from(sync.rev).unwrap_or(i64::MAX)),
    );
    doc.insert("desk_origin", toml_edit::value(sync.origin.to_string()));
    let out = doc.to_string();
    AgentConfig::from_toml(&out).map_err(|e| ConfigEditError::Invalid(e.to_string()))?;
    Ok(out)
}

/// Lo mismo sobre el archivo.
pub fn set_desk(path: &Path, sync: &DeskSync) -> Result<(), ConfigEditError> {
    rewrite(path, |text| with_desk(text, sync))
}

fn fill_monitor(t: &mut Table, m: &MonitorConfig) -> Result<(), ConfigEditError> {
    t.insert("id", toml_edit::value(m.id.0.as_str()));
    t.insert("label", toml_edit::value(m.label.as_str()));
    if m.edid_aliases.is_empty() {
        t.remove("edid_aliases");
    } else {
        let aliases: toml_edit::Array = m.edid_aliases.iter().map(String::as_str).collect();
        t.insert("edid_aliases", Item::Value(Value::Array(aliases)));
    }
    match &m.edid_model {
        Some(model) => {
            t.insert("edid_model", toml_edit::value(model.as_str()));
        }
        None => {
            t.remove("edid_model");
        }
    }
    t.insert("wiring", Item::Value(wiring_value(&m.wiring)?));
    match &m.ddc {
        Some(ddc) => {
            t.insert("ddc", Item::Value(ddc_value(ddc)?));
        }
        None => {
            t.remove("ddc");
        }
    }
    Ok(())
}

/// Aplica `edit` al monitor `id` (en `[[monitors]]` o en `monitors = [{ .. }]`) y valida
/// el resultado como configuración completa.
fn edit_monitor(
    text: &str,
    id: &MonitorId,
    edit: impl FnOnce(&mut dyn TableLike),
) -> Result<String, ConfigEditError> {
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| ConfigEditError::Parse(e.message().to_owned()))?;
    let table: Option<&mut dyn TableLike> = match doc.get_mut("monitors") {
        Some(Item::ArrayOfTables(monitors)) => monitors
            .iter_mut()
            .find(|t| t.get("id").and_then(Item::as_str) == Some(id.0.as_str()))
            .map(|t| t as &mut dyn TableLike),
        Some(Item::Value(Value::Array(monitors))) => monitors
            .iter_mut()
            .filter_map(Value::as_inline_table_mut)
            .find(|t| t.get("id").and_then(Value::as_str) == Some(id.0.as_str()))
            .map(|t| t as &mut dyn TableLike),
        _ => None,
    };
    edit(table.ok_or_else(|| ConfigEditError::NoMonitor(id.clone()))?);
    let out = doc.to_string();
    AgentConfig::from_toml(&out).map_err(|e| ConfigEditError::Invalid(e.to_string()))?;
    Ok(out)
}

/// Lee, cambia y escribe aparte + renombra: un error deja el archivo como estaba.
fn rewrite(
    path: &Path,
    change: impl FnOnce(&str) -> Result<String, ConfigEditError>,
) -> Result<(), ConfigEditError> {
    let shown = path.display().to_string();
    let text = std::fs::read_to_string(path).map_err(|source| ConfigEditError::Read {
        path: shown.clone(),
        source,
    })?;
    let out = change(&text)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, out)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|source| ConfigEditError::Write {
            path: shown,
            source,
        })
}

/// Como se escribe a mano: `{ pc = "displayport", mac = "hdmi" }`, la PC primero.
fn wiring_value(wiring: &BTreeMap<Machine, Input>) -> Result<Value, ConfigEditError> {
    let input = |i: Input| match i {
        Input::Hdmi => "hdmi",
        Input::DisplayPort => "displayport",
    };
    let parts: Vec<String> = [Machine::Pc, Machine::Mac]
        .into_iter()
        .filter_map(|m| wiring.get(&m).map(|i| format!("{m} = \"{}\"", input(*i))))
        .collect();
    parse_value("wiring", &format!("wiring = {{ {} }}", parts.join(", ")))
}

fn parse_value(key: &str, snippet: &str) -> Result<Value, ConfigEditError> {
    let doc: DocumentMut = snippet
        .parse()
        .map_err(|e: toml_edit::TomlError| ConfigEditError::Parse(e.message().to_owned()))?;
    doc.get(key)
        .and_then(Item::as_value)
        .cloned()
        .ok_or_else(|| ConfigEditError::Parse(snippet.to_owned()))
}

/// En hexadecimal, como se escriben a mano: se arma como texto y se lee con toml_edit
/// para que conserve esa forma.
fn ddc_value(ddc: &DdcProfile) -> Result<Value, ConfigEditError> {
    parse_value(
        "ddc",
        &format!(
            "ddc = {{ hdmi = {:#04X}, display_port = {:#04X} }}",
            ddc.hdmi, ddc.display_port
        ),
    )
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

    fn sync_of(text: &str, rev: u64) -> DeskSync {
        let cfg = AgentConfig::from_toml(text).unwrap();
        DeskSync {
            rev,
            origin: Machine::Mac,
            desk: cfg.desk,
            lock_shortcut: cfg.input.lock_shortcut,
        }
    }

    #[test]
    fn the_desk_from_the_other_machine_keeps_what_is_local() {
        let mut sync = sync_of(PC, 3);
        // El 32" cambió de cableado en la otra máquina, el 34" se fue y llegó uno nuevo.
        sync.desk.monitors.remove(0);
        sync.desk.monitors[0].wiring = [
            (Machine::Pc, Input::Hdmi),
            (Machine::Mac, Input::DisplayPort),
        ]
        .into();
        sync.desk.monitors.push(MonitorConfig {
            id: "sam7489".into(),
            label: "27\"".into(),
            edid_model: Some("SAM7489".into()),
            edid_aliases: Vec::new(),
            wiring: [(Machine::Pc, Input::DisplayPort)].into(),
            ddc: Some(CODES),
        });
        sync.lock_shortcut = vec!["ControlLeft".into(), "KeyL".into()];
        let out = with_desk(PC, &sync).unwrap();
        assert!(out.starts_with("# La PC.\n"), "{out}");
        assert!(out.contains("machine = \"pc\""), "{out}");
        // El comentario del 32" sigue: se actualizó en su lugar.
        assert!(out.contains("# El 32\": todavía sin códigos."), "{out}");
        let cfg = AgentConfig::from_toml(&out).unwrap();
        assert_eq!(cfg.machine, Machine::Pc);
        assert_eq!(cfg.desk, sync.desk);
        assert_eq!(cfg.input.lock_shortcut, sync.lock_shortcut);
        assert_eq!(cfg.desk_rev, 3);
        assert_eq!(cfg.desk_origin, Some(Machine::Mac));
        // Y otra vez no cambia nada.
        assert_eq!(with_desk(&out, &sync).unwrap(), out);
    }

    #[test]
    fn aliases_are_written_and_removed_with_the_monitor() {
        let mut sync = sync_of(PC, 2);
        sync.desk.monitors[0].edid_aliases = vec!["SAM7119".into()];
        let out = with_desk(PC, &sync).unwrap();
        assert!(out.contains("edid_aliases = [\"SAM7119\"]"), "{out}");
        assert_eq!(AgentConfig::from_toml(&out).unwrap().desk, sync.desk);
        // Al quitarlo, el vínculo se va con él (T-087).
        sync.desk.monitors.remove(0);
        sync.rev = 3;
        let out = with_desk(&out, &sync).unwrap();
        assert!(
            !out.contains("SAM7119") && !out.contains("LC34G55T"),
            "{out}"
        );
    }

    #[test]
    fn the_keyboard_is_saved_without_touching_the_rest() {
        let text = format!("{PC}\n[input]\nserver = \"mac\"\nclipboard = false\n");
        let out = with_keyboard(&text, Machine::Pc).unwrap();
        let cfg = AgentConfig::from_toml(&out).unwrap();
        assert_eq!(cfg.input.keyboard, Some(Machine::Pc));
        assert!(!cfg.input.clipboard);
        assert!(!out.contains("server"), "{out}");
        assert!(out.starts_with("# La PC.\n"));
        // Sin sección [input], la crea.
        let out = with_keyboard(PC, Machine::Mac).unwrap();
        assert_eq!(
            AgentConfig::from_toml(&out).unwrap().input.keyboard,
            Some(Machine::Mac)
        );
    }

    #[test]
    fn a_desk_without_monitors_is_still_valid() {
        let mut sync = sync_of(PC, 1);
        sync.desk.monitors.clear();
        let out = with_desk(PC, &sync).unwrap();
        assert!(
            AgentConfig::from_toml(&out)
                .unwrap()
                .desk
                .monitors
                .is_empty()
        );
        let back = with_desk(&out, &sync_of(PC, 2)).unwrap();
        assert_eq!(
            AgentConfig::from_toml(&back).unwrap().desk.monitors.len(),
            2
        );
    }

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
    fn rewires_a_monitor_and_keeps_everything_else() {
        let swapped = [
            (Machine::Pc, Input::Hdmi),
            (Machine::Mac, Input::DisplayPort),
        ]
        .into();
        let out = with_monitor_wiring(PC, &"m32".into(), &swapped).unwrap();
        assert!(out.contains("# El 32\": todavía sin códigos."), "{out}");
        assert!(
            out.contains("wiring = { pc = \"hdmi\", mac = \"displayport\" }"),
            "{out}"
        );
        let cfg = AgentConfig::from_toml(&out).unwrap();
        let m32 = cfg.desk.monitor(&"m32".into()).unwrap();
        assert_eq!(m32.wiring, swapped);
        // El 34" no cambia.
        assert_eq!(
            cfg.desk
                .monitor(&"m34".into())
                .unwrap()
                .input_for(Machine::Pc),
            Some(Input::DisplayPort)
        );
        // Solo a una máquina también vale; a ninguna, no.
        let pc_only = [(Machine::Pc, Input::DisplayPort)].into();
        let out = with_monitor_wiring(PC, &"m34".into(), &pc_only).unwrap();
        assert!(out.contains("wiring = { pc = \"displayport\" }"), "{out}");
        assert!(matches!(
            with_monitor_wiring(PC, &"m34".into(), &BTreeMap::new()),
            Err(ConfigEditError::Invalid(_))
        ));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tandem.toml");
        std::fs::write(&path, PC).unwrap();
        set_monitor_wiring(&path, &"m32".into(), &swapped).unwrap();
        let cfg = AgentConfig::load(&path).unwrap();
        assert_eq!(cfg.desk.monitor(&"m32".into()).unwrap().wiring, swapped);
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
