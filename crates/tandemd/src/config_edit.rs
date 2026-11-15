//! Cambios que el agente escribe en `tandem.toml` (T-084: el puesto que llegó de la otra
//! máquina; T-087: dónde está el teclado), sin tocar el resto del archivo: los comentarios
//! y el orden del usuario se conservan.
//!
//! El archivo nuevo se valida como configuración completa antes de escribirlo, y se
//! escribe aparte y se renombra: un error o un corte a medias dejan el anterior intacto.

use std::path::Path;

use tandem_core::{Machine, ScreenSpec};
use tandem_proto::DeskSync;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value};

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
    #[error("el cambio dejaría una configuración inválida: {0}")]
    Invalid(String),
    #[error("no se pudo escribir {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

fn parse(text: &str) -> Result<DocumentMut, ConfigEditError> {
    text.parse()
        .map_err(|e: toml_edit::TomlError| ConfigEditError::Parse(e.message().to_owned()))
}

fn validated(doc: &DocumentMut) -> Result<String, ConfigEditError> {
    let out = doc.to_string();
    AgentConfig::from_toml(&out).map_err(|e| ConfigEditError::Invalid(e.to_string()))?;
    Ok(out)
}

fn input_table(doc: &mut DocumentMut) -> Result<&mut dyn toml_edit::TableLike, ConfigEditError> {
    doc.entry("input")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or_else(|| ConfigEditError::Invalid("[input] no es una tabla".into()))
}

/// `keyboard = "pc"` en `[input]`: dónde está el teclado y el mouse, dicho por el usuario
/// en el asistente (T-087).
pub fn with_keyboard(text: &str, machine: Machine) -> Result<String, ConfigEditError> {
    let mut doc = parse(text)?;
    let input = input_table(&mut doc)?;
    // El nombre de antes (`server`) valdría lo mismo; se deja uno solo.
    input.remove("server");
    input.insert("keyboard", toml_edit::value(machine.to_string()));
    validated(&doc)
}

/// Lo mismo sobre el archivo.
pub fn set_keyboard(path: &Path, machine: Machine) -> Result<(), ConfigEditError> {
    rewrite(path, |text| with_keyboard(text, machine))
}

/// El puesto compartido de `sync` sobre el texto de `tandem.toml` (T-084): los
/// `[[screens]]` (se actualizan en su lugar los que ya estaban, se agregan los nuevos al
/// final y se quitan los que ya no están), `[arrangement]`, `[input] lock_shortcut`,
/// `desk_rev`, `desk_origin` y `desk_sum`. Lo solo-local (máquina, red, energía, el resto
/// de `[input]`) no se toca. Los `[[monitors]]` de v0.1 se borran.
pub fn with_desk(text: &str, sync: &DeskSync) -> Result<String, ConfigEditError> {
    let mut doc = parse(text)?;
    doc.remove("monitors");
    let mut old: Vec<Table> = match doc.remove("screens") {
        Some(Item::ArrayOfTables(screens)) => screens.into_iter().collect(),
        Some(Item::Value(Value::Array(screens))) => screens
            .into_iter()
            .filter_map(|v| match v {
                Value::InlineTable(t) => Some(t.into_table()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut screens = ArrayOfTables::new();
    for s in &sync.desk.screens {
        let at = old.iter().position(|t| {
            t.get("id").and_then(Item::as_str) == Some(s.id.as_str())
                && t.get("machine").and_then(Item::as_str) == Some(&s.machine.to_string())
        });
        let mut table = at.map(|i| old.remove(i)).unwrap_or_default();
        fill_screen(&mut table, s);
        screens.push(table);
    }
    if !screens.is_empty() {
        doc.insert("screens", Item::ArrayOfTables(screens));
    }
    let arrangement = doc
        .entry("arrangement")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or_else(|| ConfigEditError::Invalid("[arrangement] no es una tabla".into()))?;
    let a = sync.desk.arrangement;
    let side = match a.mac_side {
        tandem_core::Side::Left => "left",
        tandem_core::Side::Right => "right",
    };
    arrangement.insert("mac_side", toml_edit::value(side));
    arrangement.insert("offset", toml_edit::value(i64::from(a.offset)));
    let keys: toml_edit::Array = sync.lock_shortcut.iter().map(String::as_str).collect();
    input_table(&mut doc)?.insert("lock_shortcut", Item::Value(Value::Array(keys)));
    doc.insert(
        "desk_rev",
        toml_edit::value(i64::try_from(sync.rev).unwrap_or(i64::MAX)),
    );
    doc.insert("desk_origin", toml_edit::value(sync.origin.to_string()));
    doc.insert(
        "desk_sum",
        toml_edit::value(crate::desk_sync::fingerprint(
            &sync.desk,
            &sync.lock_shortcut,
        )),
    );
    validated(&doc)
}

/// Lo mismo sobre el archivo.
pub fn set_desk(path: &Path, sync: &DeskSync) -> Result<(), ConfigEditError> {
    rewrite(path, |text| with_desk(text, sync))
}

fn fill_screen(t: &mut Table, s: &ScreenSpec) {
    t.insert("machine", toml_edit::value(s.machine.to_string()));
    t.insert("id", toml_edit::value(s.id.as_str()));
    t.insert("x", toml_edit::value(i64::from(s.x)));
    t.insert("y", toml_edit::value(i64::from(s.y)));
    t.insert("width", toml_edit::value(i64::from(s.width)));
    t.insert("height", toml_edit::value(i64::from(s.height)));
    t.insert("scale", toml_edit::value(s.scale.as_f64()));
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

#[cfg(test)]
mod tests {
    use tandem_core::{Arrangement, Config, Scale, Side};

    use super::*;

    const PC: &str = r#"# La PC.
machine = "pc"

# El 34", medido a mano.
[[screens]]
machine = "pc"
id = "34"
x = 0
y = 0
width = 3440
height = 1440

[[screens]]
machine = "mac"
id = "laptop"
x = 0
y = 0
width = 1512
height = 982
scale = 2.0
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
        // La pantalla de la Mac se fue, el 34" cambió de lugar y llegó una nueva.
        sync.desk.screens.remove(1);
        sync.desk.screens[0].x = -3440;
        sync.desk.screens.push(ScreenSpec {
            machine: Machine::Pc,
            id: "32".into(),
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
            scale: Scale::from_milli(1500),
        });
        sync.desk.arrangement = Arrangement {
            mac_side: Side::Right,
            offset: 120,
        };
        sync.lock_shortcut = vec!["ControlLeft".into(), "KeyL".into()];
        let out = with_desk(PC, &sync).unwrap();
        assert!(out.starts_with("# La PC.\n"), "{out}");
        // El comentario del 34" sigue: se actualizó en su lugar.
        assert!(out.contains("# El 34\", medido a mano."), "{out}");
        assert!(!out.contains("laptop"), "{out}");
        let cfg = AgentConfig::from_toml(&out).unwrap();
        assert_eq!(cfg.machine, Machine::Pc);
        assert_eq!(cfg.desk, sync.desk);
        assert_eq!(cfg.input.lock_shortcut, sync.lock_shortcut);
        assert_eq!(cfg.desk_rev, 3);
        assert_eq!(cfg.desk_origin, Some(Machine::Mac));
        assert_eq!(
            cfg.desk_sum,
            Some(crate::desk_sync::fingerprint(
                &sync.desk,
                &sync.lock_shortcut
            ))
        );
        // Y otra vez no cambia nada.
        assert_eq!(with_desk(&out, &sync).unwrap(), out);
    }

    #[test]
    fn writing_the_desk_drops_the_old_monitors() {
        let old = "machine = \"pc\"\ndesk_rev = 2\n\n[[monitors]]\nid = \"m34\"\nlabel = \"34\"\nwiring = { pc = \"displayport\" }\n";
        let sync = DeskSync {
            rev: 3,
            origin: Machine::Pc,
            desk: Config::default(),
            lock_shortcut: vec![],
        };
        let out = with_desk(old, &sync).unwrap();
        assert!(!out.contains("monitors"), "{out}");
        let cfg = AgentConfig::from_toml(&out).unwrap();
        assert!(cfg.warnings().is_empty());
        assert_eq!(cfg.desk, Config::default());
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
    fn a_desk_without_screens_is_still_valid() {
        let mut sync = sync_of(PC, 1);
        sync.desk.screens.clear();
        let out = with_desk(PC, &sync).unwrap();
        assert!(
            AgentConfig::from_toml(&out)
                .unwrap()
                .desk
                .screens
                .is_empty()
        );
        let back = with_desk(&out, &sync_of(PC, 2)).unwrap();
        assert_eq!(AgentConfig::from_toml(&back).unwrap().desk.screens.len(), 2);
    }

    #[test]
    fn errors_leave_the_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tandem.toml");
        std::fs::write(&path, "machine = ").unwrap();
        assert!(matches!(
            set_keyboard(&path, Machine::Pc),
            Err(ConfigEditError::Parse(_))
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "machine = ");
        // Un puesto que no valida tampoco se escribe.
        std::fs::write(&path, PC).unwrap();
        let mut sync = sync_of(PC, 2);
        sync.desk.screens.push(sync.desk.screens[0].clone());
        assert!(matches!(
            set_desk(&path, &sync),
            Err(ConfigEditError::Invalid(_))
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), PC);
        assert!(matches!(
            set_desk(&dir.path().join("no-existe.toml"), &sync_of(PC, 2)),
            Err(ConfigEditError::Read { .. })
        ));
    }
}
