//! Cambios que el agente escribe en `tandem.toml` (T-084: el puesto que llegó de la otra
//! máquina; T-087: dónde está el teclado), sin tocar el resto del archivo: los comentarios
//! y el orden del usuario se conservan.
//!
//! El archivo nuevo se valida como configuración completa antes de escribirlo, y se
//! escribe aparte y se renombra: un error o un corte a medias dejan el anterior intacto.

use std::path::Path;

use tandem_core::{Machine, MachineInfo, Machines, ScreenSpec};
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

/// `keyboard = "b"` en `[input]`: dónde está el teclado y el mouse, dicho por el usuario
/// en el asistente (T-087).
pub fn with_keyboard(text: &str, machine: Machine) -> Result<String, ConfigEditError> {
    let mut doc = parse(text)?;
    let input = input_table(&mut doc)?;
    // El nombre de antes (`server`) valdría lo mismo; se deja uno solo.
    input.remove("server");
    input.insert("keyboard", toml_edit::value(machine.as_str()));
    validated(&doc)
}

/// Los lados de `cfg` sobre el texto de `tandem.toml` (T-097): `machine`, el teclado y todo
/// el puesto. Es lo que se escribe al traducir un archivo de v0.2 o cuando la identidad dice
/// que esta máquina es el otro lado. Lo demás (red, energía, comentarios) no se toca.
pub fn with_sides(
    text: &str,
    machine: Machine,
    keyboard: Option<Machine>,
    sync: &DeskSync,
) -> Result<String, ConfigEditError> {
    let mut doc = parse(text)?;
    // Las pantallas que ya estaban cambian de nombre en su lugar: así `with_desk` las
    // reconoce y conserva sus comentarios.
    let old = doc.get("machine").and_then(Item::as_str).map(str::to_owned);
    let relabel = |label: &str| -> Option<Machine> {
        match (old.as_deref(), label) {
            // De v0.2: la de esta máquina es `machine`, la otra el otro lado.
            (Some(local), "mac" | "pc") if local == label => Some(machine),
            (_, "mac" | "pc") => Some(machine.other()),
            (Some(prev), _) => {
                let flip = Machine::parse(prev).is_some_and(|p| p != machine);
                Machine::parse(label).map(|m| if flip { m.other() } else { m })
            }
            (None, _) => Machine::parse(label),
        }
    };
    if let Some(Item::ArrayOfTables(screens)) = doc.get_mut("screens") {
        for t in screens.iter_mut() {
            if let Some(new) = t.get("machine").and_then(Item::as_str).and_then(relabel) {
                t["machine"] = toml_edit::value(new.as_str());
            }
        }
    }
    doc["machine"] = toml_edit::value(machine.as_str());
    if let Some(arr) = doc.get_mut("arrangement").and_then(Item::as_table_like_mut) {
        arr.remove("mac_side");
    }
    let input = input_table(&mut doc)?;
    input.remove("server");
    match keyboard {
        Some(k) => {
            input.insert("keyboard", toml_edit::value(k.as_str()));
        }
        None => {
            input.remove("keyboard");
        }
    }
    with_desk(&doc.to_string(), sync)
}

/// Lo mismo sobre el archivo.
pub fn set_sides(
    path: &Path,
    machine: Machine,
    keyboard: Option<Machine>,
    sync: &DeskSync,
) -> Result<(), ConfigEditError> {
    rewrite(path, |text| with_sides(text, machine, keyboard, sync))
}

/// Lo mismo sobre el archivo.
pub fn set_keyboard(path: &Path, machine: Machine) -> Result<(), ConfigEditError> {
    rewrite(path, |text| with_keyboard(text, machine))
}

/// El puesto compartido de `sync` sobre el texto de `tandem.toml` (T-084): los
/// `[[screens]]` (se actualizan en su lugar los que ya estaban, se agregan los nuevos al
/// final y se quitan los que ya no están), `[machines]`, `[arrangement]`,
/// `[input] lock_shortcut`, `desk_rev`, `desk_origin` y `desk_sum`. Lo solo-local (máquina, red, energía, el resto
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
                && t.get("machine").and_then(Item::as_str) == Some(s.machine.as_str())
        });
        let mut table = at.map(|i| old.remove(i)).unwrap_or_default();
        fill_screen(&mut table, s);
        screens.push(table);
    }
    if !screens.is_empty() {
        doc.insert("screens", Item::ArrayOfTables(screens));
    }
    fill_machines(&mut doc, &sync.desk.machines)?;
    let arrangement = doc
        .entry("arrangement")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or_else(|| ConfigEditError::Invalid("[arrangement] no es una tabla".into()))?;
    let a = sync.desk.arrangement;
    let side = match a.a_side {
        tandem_core::Side::Left => "left",
        tandem_core::Side::Right => "right",
    };
    arrangement.insert("a_side", toml_edit::value(side));
    arrangement.insert("offset", toml_edit::value(i64::from(a.offset)));
    let keys: toml_edit::Array = sync.lock_shortcut.iter().map(String::as_str).collect();
    input_table(&mut doc)?.insert("lock_shortcut", Item::Value(Value::Array(keys)));
    doc.insert(
        "desk_rev",
        toml_edit::value(i64::try_from(sync.rev).unwrap_or(i64::MAX)),
    );
    doc.insert("desk_origin", toml_edit::value(sync.origin.as_str()));
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

/// `[machines.a]` y `[machines.b]`; sin datos de ningún lado, sin la tabla.
fn fill_machines(doc: &mut DocumentMut, machines: &Machines) -> Result<(), ConfigEditError> {
    if machines.is_empty() {
        doc.remove("machines");
        return Ok(());
    }
    let table = doc
        .entry("machines")
        .or_insert_with(|| {
            let mut t = Table::new();
            // Solo `[machines.a]`/`[machines.b]`, sin una cabecera `[machines]` vacía.
            t.set_implicit(true);
            Item::Table(t)
        })
        .as_table_like_mut()
        .ok_or_else(|| ConfigEditError::Invalid("[machines] no es una tabla".into()))?;
    for side in Machine::ALL {
        match machines.get(side) {
            Some(info) => {
                let entry = table
                    .entry(side.as_str())
                    .or_insert_with(|| Item::Table(Table::new()));
                let t = entry.as_table_like_mut().ok_or_else(|| {
                    ConfigEditError::Invalid(format!(
                        "[machines.{}] no es una tabla",
                        side.as_str()
                    ))
                })?;
                fill_machine(t, info);
            }
            None => {
                table.remove(side.as_str());
            }
        }
    }
    Ok(())
}

fn fill_machine(t: &mut dyn toml_edit::TableLike, info: &MachineInfo) {
    let os = match info.os {
        tandem_core::Os::MacOs => "macos",
        tandem_core::Os::Windows => "windows",
    };
    t.insert("os", toml_edit::value(os));
    t.insert("name", toml_edit::value(info.name.as_str()));
    t.insert("color", toml_edit::value(info.color.as_str()));
}

fn fill_screen(t: &mut Table, s: &ScreenSpec) {
    t.insert("machine", toml_edit::value(s.machine.as_str()));
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
machine = "b"

# El 34", medido a mano.
[[screens]]
machine = "b"
id = "34"
x = 0
y = 0
width = 3440
height = 1440

[[screens]]
machine = "a"
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
            origin: Machine::A,
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
            machine: Machine::B,
            id: "32".into(),
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
            scale: Scale::from_milli(1500),
        });
        sync.desk.arrangement = Arrangement {
            a_side: Side::Right,
            offset: 120,
        };
        sync.lock_shortcut = vec!["ControlLeft".into(), "KeyL".into()];
        let out = with_desk(PC, &sync).unwrap();
        assert!(out.starts_with("# La PC.\n"), "{out}");
        // El comentario del 34" sigue: se actualizó en su lugar.
        assert!(out.contains("# El 34\", medido a mano."), "{out}");
        assert!(!out.contains("laptop"), "{out}");
        let cfg = AgentConfig::from_toml(&out).unwrap();
        assert_eq!(cfg.machine, Machine::B);
        assert_eq!(cfg.desk, sync.desk);
        assert_eq!(cfg.input.lock_shortcut, sync.lock_shortcut);
        assert_eq!(cfg.desk_rev, 3);
        assert_eq!(cfg.desk_origin, Some(Machine::A));
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
        let old = "machine = \"b\"\ndesk_rev = 2\n\n[[monitors]]\nid = \"m34\"\nlabel = \"34\"\nwiring = { pc = \"displayport\" }\n";
        let sync = DeskSync {
            rev: 3,
            origin: Machine::B,
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
        let text = format!("{PC}\n[input]\nserver = \"a\"\nclipboard = false\n");
        let out = with_keyboard(&text, Machine::B).unwrap();
        let cfg = AgentConfig::from_toml(&out).unwrap();
        assert_eq!(cfg.input.keyboard, Some(Machine::B));
        assert!(!cfg.input.clipboard);
        assert!(!out.contains("server"), "{out}");
        assert!(out.starts_with("# La PC.\n"));
        // Sin sección [input], la crea.
        let out = with_keyboard(PC, Machine::A).unwrap();
        assert_eq!(
            AgentConfig::from_toml(&out).unwrap().input.keyboard,
            Some(Machine::A)
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
            set_keyboard(&path, Machine::B),
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

    #[test]
    fn the_machines_are_written_and_removed() {
        let mut sync = sync_of(PC, 4);
        sync.desk.machines.a = Some(MachineInfo::new(
            Machine::A,
            tandem_core::Os::MacOs,
            Some("MacBook".into()),
        ));
        let out = with_desk(PC, &sync).unwrap();
        assert!(out.contains("[machines.a]"), "{out}");
        assert!(!out.contains("[machines]\n"), "sin cabecera vacía: {out}");
        let cfg = AgentConfig::from_toml(&out).unwrap();
        assert_eq!(cfg.desk.machines, sync.desk.machines);
        sync.desk.machines.a = None;
        let out = with_desk(&out, &sync).unwrap();
        assert!(!out.contains("machines"), "{out}");
    }

    /// Cambiar de lado reescribe todo lo que nombra un lado y deja lo demás.
    #[test]
    fn the_sides_are_rewritten_in_place() {
        let text = format!("{PC}\n[input]\nserver = \"a\"\nclipboard = false\n");
        let cfg = AgentConfig::from_toml(&text).unwrap().with_side(Machine::A);
        let sync = DeskSync {
            rev: 1,
            origin: Machine::A,
            desk: cfg.desk.clone(),
            lock_shortcut: cfg.input.lock_shortcut.clone(),
        };
        let out = with_sides(&text, cfg.machine, cfg.input.keyboard, &sync).unwrap();
        assert!(out.starts_with("# La PC.\n"), "{out}");
        let back = AgentConfig::from_toml(&out).unwrap();
        assert!(!back.rewrite_sides);
        assert_eq!(back.machine, Machine::A);
        assert_eq!(back.input.keyboard, Some(Machine::B));
        assert!(!back.input.clipboard);
        assert_eq!(back.desk, cfg.desk);
        assert_eq!(back.desk.screens_of(Machine::A).next().unwrap().id, "34");
    }

    /// Un archivo de v0.2 se reescribe en el formato nuevo, sin rastros de `mac`/`pc`.
    #[test]
    fn an_old_file_is_rewritten_with_the_new_sides() {
        let old = include_str!("../../../docs/examples/v0.2/tandem.pc.toml");
        let cfg = AgentConfig::from_toml(old).unwrap();
        assert!(cfg.rewrite_sides);
        let sync = DeskSync {
            rev: cfg.desk_rev,
            origin: cfg.desk_origin.unwrap_or(cfg.machine),
            desk: cfg.desk.clone(),
            lock_shortcut: cfg.input.lock_shortcut.clone(),
        };
        let out = with_sides(old, cfg.machine, cfg.input.keyboard, &sync).unwrap();
        let back = AgentConfig::from_toml(&out).unwrap();
        assert!(!back.rewrite_sides, "{out}");
        assert_eq!(back.desk, cfg.desk);
        assert_eq!(back.input.keyboard, cfg.input.keyboard);
        for line in out.lines().filter(|l| !l.trim_start().starts_with('#')) {
            assert!(
                !line.contains("\"mac\"") && !line.contains("\"pc\"") && !line.contains("mac_side"),
                "{line}"
            );
        }
    }
}
