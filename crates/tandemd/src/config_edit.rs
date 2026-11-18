//! Cambios que el agente escribe en `tandem.toml` (T-084: el puesto que llegó de la otra
//! máquina; T-087: dónde está el teclado), sin tocar el resto del archivo: los comentarios
//! y el orden del usuario se conservan.
//!
//! El archivo nuevo se valida como configuración completa antes de escribirlo, y se
//! escribe aparte y se renombra: un error o un corte a medias dejan el anterior intacto.

use std::path::Path;

use tandem_core::{
    Align, DataPrefs, DeskVersion, Machine, MachineInfo, Machines, ScreenSpec, Side,
};
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
    sync: &DeskVersion,
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
    with_desk(&doc.to_string(), machine, sync)
}

/// Lo mismo sobre el archivo.
pub fn set_sides(
    path: &Path,
    machine: Machine,
    keyboard: Option<Machine>,
    sync: &DeskVersion,
) -> Result<(), ConfigEditError> {
    rewrite(path, |text| with_sides(text, machine, keyboard, sync))
}

/// Lo mismo sobre el archivo.
pub fn set_keyboard(path: &Path, machine: Machine) -> Result<(), ConfigEditError> {
    rewrite(path, |text| with_keyboard(text, machine))
}

/// `[power.wol] mac = "..."`: a quién despierta esta máquina (T-159), aprendido al emparejar
/// o escrito en la UI. `None` quita la MAC y anota `cleared = true`. Lo demás de `[power.wol]` (`source`,
/// `broadcast`, `port`) se conserva.
pub fn with_wol_mac(
    text: &str,
    mac: Option<tandem_power::MacAddr>,
) -> Result<String, ConfigEditError> {
    let mut doc = parse(text)?;
    let power = section(&mut doc, "power")?;
    match mac {
        Some(mac) => {
            let wol = power
                .entry("wol")
                .or_insert_with(|| Item::Table(Table::new()));
            let wol = wol
                .as_table_like_mut()
                .ok_or_else(|| ConfigEditError::Invalid("[power.wol] no es una tabla".into()))?;
            wol.remove("cleared");
            wol.insert("mac", toml_edit::value(mac.to_string()));
        }
        None => {
            // No se borra la tabla: `cleared = true` recuerda que fue una decisión del
            // usuario y que no hay que volver a aprender la MAC.
            let wol = power
                .entry("wol")
                .or_insert_with(|| Item::Table(Table::new()));
            let wol = wol
                .as_table_like_mut()
                .ok_or_else(|| ConfigEditError::Invalid("[power.wol] no es una tabla".into()))?;
            wol.remove("mac");
            wol.insert("cleared", toml_edit::value(true));
        }
    }
    validated(&doc)
}

/// Lo mismo sobre el archivo.
pub fn set_wol_mac(path: &Path, mac: Option<tandem_power::MacAddr>) -> Result<(), ConfigEditError> {
    rewrite(path, |text| with_wol_mac(text, mac))
}

/// El puesto compartido de `sync` sobre el texto de `tandem.toml` (T-084, T-099): los
/// `[[screens]]` (se actualizan en su lugar los que ya estaban, se agregan los nuevos al
/// final y se quitan los que ya no están), `[machines]`, `[arrangement]`, `[[scenes]]`,
/// `[shortcuts]`, `[prefs]`, `[modules]`, `[data]` y el sello (`desk_rev`, `desk_at`, `desk_origin`,
/// `desk_sum`). Lo solo-local (máquina, red, energía, `[input]`) no se toca. Los
/// `[[monitors]]` de v0.1 y lo del puesto que antes vivía en `[input]` se borran.
///
/// Se escribe aparte y se renombra ([`rewrite`]): un corte a medias deja el anterior.
pub fn with_desk(
    text: &str,
    machine: Machine,
    sync: &DeskVersion,
) -> Result<String, ConfigEditError> {
    let mut doc = parse(text)?;
    // El lado va siempre con el puesto: sus pantallas están etiquetadas desde él.
    doc["machine"] = toml_edit::value(machine.as_str());
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
    let first = old.iter().filter_map(Table::position).min();
    let mut screens = ArrayOfTables::new();
    for s in &sync.desk.screens {
        let at = old.iter().position(|t| {
            t.get("id").and_then(Item::as_str) == Some(s.id.as_str())
                && t.get("machine").and_then(Item::as_str) == Some(s.machine.as_str())
        });
        let mut table = at.map(|i| old.remove(i)).unwrap_or_default();
        // Todas en el lugar de la primera de antes: toml_edit ordena las tablas por su
        // posición (de forma estable), y con la de cada una, una reutilizada quedaría antes
        // que las nuevas aunque el puesto diga otro orden (y la huella del archivo ya no
        // sería la del puesto).
        table.set_position(first);
        fill_screen(&mut table, s);
        screens.push(table);
    }
    if !screens.is_empty() {
        doc.insert("screens", Item::ArrayOfTables(screens));
    }
    let desk = &sync.desk;
    match &desk.name {
        Some(name) => doc["desk_name"] = toml_edit::value(name.as_str()),
        None => {
            doc.remove("desk_name");
        }
    }
    fill_machines(&mut doc, &desk.machines)?;
    match desk.arrangement {
        Some(a) => {
            let arrangement = section(&mut doc, "arrangement")?;
            arrangement.remove("offset");
            arrangement.insert("a_side", toml_edit::value(side_str(a.a_side)));
            arrangement.insert("align", toml_edit::value(align_str(a.align)));
        }
        None => {
            doc.remove("arrangement");
        }
    }
    // `scenes = []` ya escrito se deja donde está: reescribir lo mismo no mueve nada.
    let already_empty = desk.scenes.is_empty()
        && doc
            .get("scenes")
            .and_then(Item::as_array)
            .is_some_and(toml_edit::Array::is_empty);
    if !already_empty {
        write_scenes(&mut doc, &desk.scenes);
    }
    let keys: toml_edit::Array = desk.shortcuts.lock.iter().map(String::as_str).collect();
    section(&mut doc, "shortcuts")?.insert("lock", Item::Value(Value::Array(keys)));
    let prefs = section(&mut doc, "prefs")?;
    prefs.insert("cmd_ctrl", toml_edit::value(desk.prefs.cmd_ctrl));
    prefs.insert("drag_files", toml_edit::value(desk.prefs.drag_files));
    prefs.remove("clipboard");
    fill_modules(&mut doc, &desk.modules)?;
    fill_data(&mut doc, &desk.data)?;
    write_cables(&mut doc, &desk.cables);
    // Lo que antes guardaba el puesto en `[input]`.
    if let Some(input) = doc.get_mut("input").and_then(Item::as_table_like_mut) {
        input.remove("lock_shortcut");
        input.remove("clipboard");
    }
    let int = |n: u64| toml_edit::value(i64::try_from(n).unwrap_or(i64::MAX));
    doc.insert("desk_rev", int(sync.stamp.rev));
    if sync.stamp.at > 0 {
        doc.insert("desk_at", int(sync.stamp.at));
    } else {
        doc.remove("desk_at");
    }
    doc.insert("desk_origin", toml_edit::value(sync.stamp.by.as_str()));
    doc.insert(
        "desk_sum",
        toml_edit::value(crate::desk_sync::fingerprint(desk)),
    );
    validated(&doc)
}

/// Las `[[scenes]]` del puesto, en el lugar de las de antes: reescribir lo mismo no mueve nada.
fn write_scenes(doc: &mut DocumentMut, scenes_of: &[tandem_core::Scene]) {
    let at = match doc.remove("scenes") {
        Some(Item::ArrayOfTables(old)) => old.iter().filter_map(Table::position).min(),
        _ => None,
    };
    let mut scenes = ArrayOfTables::new();
    for s in scenes_of {
        let mut t = Table::new();
        t.set_position(at);
        t.insert("id", toml_edit::value(s.id.as_str()));
        t.insert("name", toml_edit::value(s.name.as_str()));
        t.insert("icon", toml_edit::value(s.icon.as_str()));
        if !s.monitors.is_empty() {
            let monitors: toml_edit::Array = s
                .monitors
                .iter()
                .map(|(monitor, machine)| {
                    Value::Array([monitor.as_str(), machine.as_str()].into_iter().collect())
                })
                .collect();
            t.insert("monitors", Item::Value(Value::Array(monitors)));
        }
        if let Some(layout) = s.layout {
            let mut l = toml_edit::InlineTable::new();
            l.insert("a_side", side_str(layout.a_side).into());
            l.insert("align", align_str(layout.align).into());
            t.insert("layout", Item::Value(Value::InlineTable(l)));
        }
        if let Some(machine) = s.require_on {
            t.insert("require_on", toml_edit::value(machine.as_str()));
        }
        if !s.shortcut.is_empty() {
            let keys: toml_edit::Array = s.shortcut.iter().map(String::as_str).collect();
            t.insert("shortcut", Item::Value(Value::Array(keys)));
        }
        scenes.push(t);
    }
    if scenes.is_empty() {
        // Explícito: sin la clave, al arrancar volverían las escenas por omisión.
        doc.insert("scenes", Item::Value(Value::Array(toml_edit::Array::new())));
    } else {
        doc.insert("scenes", Item::ArrayOfTables(scenes));
    }
}

/// Los `[[cables]]` del puesto (T-111), en el lugar de los de antes; sin ninguno, sin la clave.
fn write_cables(doc: &mut DocumentMut, cables: &[tandem_core::MonitorCable]) {
    let at = match doc.remove("cables") {
        Some(Item::ArrayOfTables(old)) => old.iter().filter_map(Table::position).min(),
        _ => None,
    };
    if cables.is_empty() {
        return;
    }
    let input_str = |i: tandem_core::MonitorInput| match i {
        tandem_core::MonitorInput::DisplayPort => "display_port",
        tandem_core::MonitorInput::Hdmi => "hdmi",
    };
    let mut out = ArrayOfTables::new();
    for c in cables {
        let mut t = Table::new();
        t.set_position(at);
        t.insert("monitor", toml_edit::value(c.monitor.as_str()));
        for side in Machine::ALL {
            let end = c.end(side);
            let mut e = toml_edit::InlineTable::new();
            if let Some(input) = end.input {
                e.insert("input", input_str(input).into());
            }
            if let Some(screen) = &end.screen {
                e.insert("screen", screen.as_str().into());
            }
            if !e.is_empty() {
                t.insert(side.as_str(), Item::Value(Value::InlineTable(e)));
            }
        }
        out.push(t);
    }
    doc.insert("cables", Item::ArrayOfTables(out));
}

/// Lo mismo sobre el archivo.
pub fn set_desk(path: &Path, machine: Machine, sync: &DeskVersion) -> Result<(), ConfigEditError> {
    rewrite(path, |text| with_desk(text, machine, sync))
}

fn section<'a>(
    doc: &'a mut DocumentMut,
    name: &str,
) -> Result<&'a mut dyn toml_edit::TableLike, ConfigEditError> {
    doc.entry(name)
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or_else(|| ConfigEditError::Invalid(format!("[{name}] no es una tabla")))
}

/// `[data]`: el tope de los datos por Wi-Fi (T-101). Con los valores de omisión y sin la
/// tabla en el archivo, no se agrega.
fn fill_data(doc: &mut DocumentMut, data: &DataPrefs) -> Result<(), ConfigEditError> {
    if *data == DataPrefs::default() && !doc.contains_key("data") {
        return Ok(());
    }
    let t = section(doc, "data")?;
    t.insert(
        "wifi_cap_percent",
        toml_edit::value(i64::from(data.wifi_cap_percent)),
    );
    match data.wifi_cap_mbps {
        Some(m) => {
            t.insert("wifi_cap_mbps", toml_edit::value(i64::from(m)));
        }
        None => {
            t.remove("wifi_cap_mbps");
        }
    }
    Ok(())
}

fn side_str(side: Side) -> &'static str {
    match side {
        Side::Left => "left",
        Side::Right => "right",
        Side::Up => "up",
        Side::Down => "down",
    }
}

fn align_str(align: Align) -> &'static str {
    match align {
        Align::Start => "start",
        Align::Center => "center",
        Align::End => "end",
    }
}

/// `[modules.a]` y `[modules.b]`, de a uno por módulo; sin nada, sin la tabla.
fn fill_modules(
    doc: &mut DocumentMut,
    modules: &tandem_core::Modules,
) -> Result<(), ConfigEditError> {
    // Cada `[modules.x]` en el lugar del de antes: reescribir lo mismo no mueve nada.
    let old = doc.remove("modules");
    let at = |side: Machine| {
        old.as_ref()
            .and_then(|o| o.get(side.as_str()))
            .and_then(Item::as_table)
            .and_then(Table::position)
    };
    if modules.a.is_empty() && modules.b.is_empty() {
        return Ok(());
    }
    let mut table = Table::new();
    table.set_implicit(true);
    for side in Machine::ALL {
        let of = modules.of(side);
        if of.is_empty() {
            continue;
        }
        let mut t = Table::new();
        t.set_position(at(side));
        for (name, on) in of {
            t.insert(name, toml_edit::value(*on));
        }
        table.insert(side.as_str(), Item::Table(t));
    }
    doc.insert("modules", Item::Table(table));
    Ok(())
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

/// Una escritura de `tandem.toml` a la vez: cada cambio lee, cambia y escribe, y dos a la
/// vez (cambiar de lado mientras llega el puesto o el teclado) se pisarían.
static REWRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Lee, cambia y escribe aparte + renombra: un error deja el archivo como estaba.
fn rewrite(
    path: &Path,
    change: impl FnOnce(&str) -> Result<String, ConfigEditError>,
) -> Result<(), ConfigEditError> {
    let _one_at_a_time = REWRITING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let shown = path.display().to_string();
    let text = std::fs::read_to_string(path).map_err(|source| ConfigEditError::Read {
        path: shown.clone(),
        source,
    })?;
    let out = change(&text)?;
    // Un temporal por escritura: dos procesos (o dos agentes de una prueba) no comparten
    // nombre aunque escriban archivos vecinos.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("toml.{}.{n}.tmp", std::process::id()));
    std::fs::write(&tmp, out)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|source| ConfigEditError::Write {
            path: shown,
            source,
        })
}

#[cfg(test)]
mod tests {
    use tandem_core::{Arrangement, Config, DeskStamp, Scale, Scene};

    use super::*;

    #[test]
    fn the_wol_mac_is_written_and_removed_keeping_the_rest() {
        let text = "machine = \"a\"\n# energía\n[power]\nwakeable = true\n";
        let mac: tandem_power::MacAddr = "AA-BB-CC-DD-EE-FF".parse().unwrap();
        let with = with_wol_mac(text, Some(mac)).unwrap();
        assert!(with.contains("# energía"), "{with}");
        let cfg = AgentConfig::from_toml(&with).unwrap();
        assert_eq!(cfg.power.wol.unwrap().mac, mac);
        // Cambiar la MAC deja lo que el usuario puso en la misma tabla.
        let custom = format!("{with}source = \"10.77.2.1\"\n");
        let other: tandem_power::MacAddr = "11:22:33:44:55:66".parse().unwrap();
        let cfg = AgentConfig::from_toml(&with_wol_mac(&custom, Some(other)).unwrap()).unwrap();
        let wol = cfg.power.wol.unwrap();
        assert_eq!(wol.mac, other);
        assert_eq!(wol.source, Some("10.77.2.1".parse().unwrap()));
        let without = AgentConfig::from_toml(&with_wol_mac(&with, None).unwrap()).unwrap();
        assert_eq!(without.power.wol, None);
        // Borrarla queda anotado para que no se vuelva a aprender; escribir otra lo quita.
        assert!(without.power.wol_cleared);
        let back = AgentConfig::from_toml(
            &with_wol_mac(&with_wol_mac(&with, None).unwrap(), Some(mac)).unwrap(),
        )
        .unwrap();
        assert_eq!(back.power.wol.unwrap().mac, mac);
        assert!(!back.power.wol_cleared);
    }

    #[test]
    fn the_desk_name_is_saved_and_read_back() {
        let sync = DeskVersion {
            stamp: DeskStamp {
                rev: 1,
                at: 0,
                by: Machine::A,
            },
            desk: tandem_core::Desk {
                name: Some("Oficina".into()),
                ..Default::default()
            },
        };
        let text = with_desk("machine = \"a\"\n", Machine::A, &sync).unwrap();
        assert_eq!(
            AgentConfig::from_toml(&text).unwrap().desk.name.as_deref(),
            Some("Oficina")
        );
        let cleared = DeskVersion {
            desk: tandem_core::Desk::default(),
            ..sync
        };
        let text = with_desk(&text, Machine::A, &cleared).unwrap();
        assert_eq!(AgentConfig::from_toml(&text).unwrap().desk.name, None);
    }

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

    fn stamp(rev: u64, by: Machine) -> DeskStamp {
        DeskStamp { rev, at: 0, by }
    }

    fn version(desk: Config, rev: u64, by: Machine) -> DeskVersion {
        DeskVersion {
            stamp: stamp(rev, by),
            desk,
        }
    }

    fn sync_of(text: &str, rev: u64) -> DeskVersion {
        version(AgentConfig::from_toml(text).unwrap().desk, rev, Machine::A)
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
        sync.desk.arrangement = Some(Arrangement {
            a_side: Side::Up,
            align: Align::End,
        });
        sync.desk.shortcuts.lock = vec!["ControlLeft".into(), "KeyL".into()];
        sync.desk.prefs.drag_files = false;
        sync.desk.scenes = tandem_core::default_scenes(&sync.desk.machines, None);
        sync.desk.scenes[1].monitors = vec![("lc34-01".into(), Machine::A)];
        sync.desk.modules.set(Machine::B, "clipboard", false);
        sync.stamp.at = 1_763_400_000_000;
        let out = with_desk(PC, Machine::B, &sync).unwrap();
        assert!(out.starts_with("# La PC.\n"), "{out}");
        // El comentario del 34" sigue: se actualizó en su lugar.
        assert!(out.contains("# El 34\", medido a mano."), "{out}");
        assert!(!out.contains("id = \"laptop\""), "{out}");
        let cfg = AgentConfig::from_toml(&out).unwrap();
        assert_eq!(cfg.machine, Machine::B);
        assert_eq!(cfg.desk, sync.desk);
        assert!(!cfg.legacy_desk, "{out}");
        assert_eq!(cfg.desk_rev, 3);
        assert_eq!(cfg.desk_at, 1_763_400_000_000);
        assert_eq!(cfg.desk_origin, Some(Machine::A));
        assert_eq!(
            cfg.desk_sum,
            Some(crate::desk_sync::fingerprint(&sync.desk))
        );
        assert!(out.contains("[modules.b]"), "{out}");
        assert!(!out.contains("[modules]\n"), "sin cabecera vacía: {out}");
        // Y otra vez no cambia nada.
        assert_eq!(with_desk(&out, Machine::B, &sync).unwrap(), out);
        // Sin acomodo, sin escenas ni módulos, sus secciones se van.
        let mut bare = sync.clone();
        bare.desk.arrangement = None;
        bare.desk.scenes.clear();
        bare.desk.modules = Default::default();
        let out = with_desk(&out, Machine::B, &bare).unwrap();
        for gone in ["[arrangement]", "[[scenes]]", "modules"] {
            assert!(!out.contains(gone), "{gone}: {out}");
        }
        assert_eq!(AgentConfig::from_toml(&out).unwrap().desk, bare.desk);
    }

    #[test]
    fn writing_the_desk_drops_the_old_monitors() {
        let old = "machine = \"b\"\ndesk_rev = 2\n\n[[monitors]]\nid = \"m34\"\nlabel = \"34\"\nwiring = { pc = \"displayport\" }\n";
        let sync = version(Config::default(), 3, Machine::B);
        let out = with_desk(old, Machine::B, &sync).unwrap();
        assert!(!out.contains("monitors"), "{out}");
        let cfg = AgentConfig::from_toml(&out).unwrap();
        assert!(cfg.warnings().is_empty());
        assert_eq!(cfg.desk, Config::default());
    }

    /// Lo que antes era del puesto en `[input]` (atajo, portapapeles) y el desplazamiento a
    /// mano del acomodo pasan a sus secciones al escribir el puesto.
    #[test]
    fn the_desk_from_before_t099_is_rewritten_in_its_sections() {
        let text = format!(
            "{PC}\n[input]\nkeyboard = \"a\"\nlock_shortcut = [\"KeyL\"]\nclipboard = false\n\n[arrangement]\na_side = \"right\"\noffset = 30\n"
        );
        let cfg = AgentConfig::from_toml(&text).unwrap();
        assert!(cfg.legacy_desk);
        let out = with_desk(&text, Machine::B, &version(cfg.desk.clone(), 1, Machine::B)).unwrap();
        for gone in ["lock_shortcut", "offset"] {
            assert!(!out.contains(gone), "{gone}: {out}");
        }
        let back = AgentConfig::from_toml(&out).unwrap();
        assert!(!back.legacy_desk, "{out}");
        assert_eq!(back.desk, cfg.desk);
        assert_eq!(back.desk.shortcuts.lock, ["KeyL"]);
        assert_eq!(back.desk.modules.get(Machine::B, "clipboard"), Some(false));
        assert_eq!(back.input.keyboard, Some(Machine::A));
    }

    #[test]
    fn the_wifi_data_cap_travels_with_the_desk() {
        // Con los valores de omisión no aparece una tabla nueva.
        let plain = with_desk(PC, Machine::B, &sync_of(PC, 2)).unwrap();
        assert!(!plain.contains("[data]"));

        let mut sync = sync_of(PC, 3);
        sync.desk.data = DataPrefs {
            wifi_cap_percent: 30,
            wifi_cap_mbps: Some(80),
        };
        let out = with_desk(PC, Machine::B, &sync).unwrap();
        let cfg = AgentConfig::from_toml(&out).unwrap();
        assert_eq!(cfg.desk.data, sync.desk.data);

        // Volver al porcentaje quita el tope fijo.
        sync.desk.data.wifi_cap_mbps = None;
        let back = with_desk(&out, Machine::B, &sync).unwrap();
        assert!(!back.contains("wifi_cap_mbps"));
        assert_eq!(
            AgentConfig::from_toml(&back).unwrap().desk.data,
            sync.desk.data
        );
    }

    #[test]
    fn the_keyboard_is_saved_without_touching_the_rest() {
        let text = format!("{PC}\n[input]\nserver = \"a\"\n\n[prefs]\ndrag_files = false\n");
        let out = with_keyboard(&text, Machine::B).unwrap();
        let cfg = AgentConfig::from_toml(&out).unwrap();
        assert_eq!(cfg.input.keyboard, Some(Machine::B));
        assert!(!cfg.desk.prefs.drag_files);
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
        let out = with_desk(PC, Machine::B, &sync).unwrap();
        assert!(
            AgentConfig::from_toml(&out)
                .unwrap()
                .desk
                .screens
                .is_empty()
        );
        let back = with_desk(&out, Machine::B, &sync_of(PC, 2)).unwrap();
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
            set_desk(&path, Machine::B, &sync),
            Err(ConfigEditError::Invalid(_))
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), PC);
        assert!(matches!(
            set_desk(
                &dir.path().join("no-existe.toml"),
                Machine::B,
                &sync_of(PC, 2)
            ),
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
        let out = with_desk(PC, Machine::B, &sync).unwrap();
        assert!(out.contains("[machines.a]"), "{out}");
        assert!(!out.contains("[machines]\n"), "sin cabecera vacía: {out}");
        let cfg = AgentConfig::from_toml(&out).unwrap();
        assert_eq!(cfg.desk.machines, sync.desk.machines);
        sync.desk.machines.a = None;
        let out = with_desk(&out, Machine::B, &sync).unwrap();
        assert!(!out.contains("machines"), "{out}");
    }

    /// Cambiar de lado reescribe todo lo que nombra un lado y deja lo demás.
    #[test]
    fn the_sides_are_rewritten_in_place() {
        let text = format!("{PC}\n[input]\nserver = \"a\"\n\n[prefs]\ndrag_files = false\n");
        let cfg = AgentConfig::from_toml(&text).unwrap().with_side(Machine::A);
        let sync = version(cfg.desk.clone(), 1, Machine::A);
        let out = with_sides(&text, cfg.machine, cfg.input.keyboard, &sync).unwrap();
        assert!(out.starts_with("# La PC.\n"), "{out}");
        let back = AgentConfig::from_toml(&out).unwrap();
        assert!(!back.rewrite_sides);
        assert_eq!(back.machine, Machine::A);
        assert_eq!(back.input.keyboard, Some(Machine::B));
        assert!(!back.desk.prefs.drag_files);
        assert_eq!(back.desk, cfg.desk);
        assert_eq!(back.desk.screens_of(Machine::A).next().unwrap().id, "34");
    }

    /// Un archivo de v0.2 se reescribe en el formato nuevo, sin rastros de `mac`/`pc`.
    #[test]
    fn an_old_file_is_rewritten_with_the_new_sides() {
        let old = include_str!("../../../docs/examples/v0.2/tandem.pc.toml");
        let cfg = AgentConfig::from_toml(old).unwrap();
        assert!(cfg.rewrite_sides);
        let sync = version(
            cfg.desk.clone(),
            cfg.desk_rev,
            cfg.desk_origin.unwrap_or(cfg.machine),
        );
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

    fn any_desk() -> impl proptest::strategy::Strategy<Value = Config> {
        use proptest::prelude::*;
        let side = || prop_oneof![Just(Machine::A), Just(Machine::B)];
        let info = |os: tandem_core::Os, color: &'static str| {
            proptest::option::of("[A-Za-z][A-Za-z0-9 ]{0,10}[a-z]").prop_map(move |name| {
                name.map(|name| MachineInfo {
                    os,
                    name,
                    color: color.to_owned(),
                })
            })
        };
        let screen = (side(), "[a-z0-9]{1,5}", -5000i32..5000, 1u32..6000, 1u32..4).prop_map(
            |(machine, id, x, width, s)| ScreenSpec {
                machine,
                id,
                x,
                y: -x / 2,
                width,
                height: width / 2 + 1,
                scale: Scale::from_milli(s * 500),
            },
        );
        let arrangement = proptest::option::of(
            (
                proptest::sample::select(tandem_core::Side::ALL.to_vec()),
                proptest::sample::select(Align::ALL.to_vec()),
            )
                .prop_map(|(a_side, align)| Arrangement { a_side, align }),
        );
        (
            (
                info(tandem_core::Os::MacOs, "#7aa7e6"),
                info(tandem_core::Os::Windows, "#5fbfb0"),
            ),
            proptest::collection::vec(screen, 0..4),
            arrangement,
            proptest::collection::btree_set("[a-z]{1,5}", 0..3),
            proptest::sample::subsequence(vec!["ScrollLock", "ControlLeft", "KeyL"], 0..=3),
            (any::<bool>(), any::<bool>(), any::<bool>()),
            proptest::collection::btree_map("[a-z]{1,6}", any::<bool>(), 0..3),
            (1u8..=100, proptest::option::of(1u32..10_000)),
            (
                proptest::collection::btree_map("[a-z0-9-]{1,8}", (0u8..3, any::<bool>()), 0..3),
                proptest::option::of("[A-Za-z0-9áñ]{1,20}"),
            ),
        )
            .prop_map(
                |(
                    (a, b),
                    screens,
                    arrangement,
                    scenes,
                    lock,
                    prefs,
                    modules,
                    data,
                    (cables, name),
                )| {
                    let mut seen = std::collections::BTreeSet::new();
                    Config {
                        name,
                        machines: Machines { a, b },
                        screens: screens
                            .into_iter()
                            .filter(|s| seen.insert((s.machine, s.id.clone())))
                            .collect(),
                        arrangement,
                        scenes: scenes
                            .into_iter()
                            .enumerate()
                            .map(|(i, id)| Scene {
                                icon: "play".into(),
                                monitors: vec![(format!("m{i}"), Machine::ALL[i % 2])],
                                layout: arrangement,
                                require_on: (i == 1).then_some(Machine::B),
                                shortcut: vec!["AltLeft".into(), format!("Digit{i}")],
                                ..Scene::new(id.clone(), id.to_uppercase())
                            })
                            .collect(),
                        shortcuts: tandem_core::Shortcuts {
                            lock: lock.into_iter().map(str::to_owned).collect(),
                        },
                        prefs: tandem_core::InputPrefs {
                            cmd_ctrl: prefs.0,
                            drag_files: prefs.1,
                        },
                        modules: tandem_core::Modules {
                            a: modules.clone(),
                            b: modules.into_iter().map(|(k, v)| (k, !v)).collect(),
                        },
                        data: DataPrefs {
                            wifi_cap_percent: data.0,
                            wifi_cap_mbps: data.1,
                        },
                        cables: cables
                            .into_iter()
                            .map(|(monitor, (input, screen))| {
                                let mut c = tandem_core::MonitorCable::new(monitor);
                                c.a.input = tandem_core::MonitorInput::ALL
                                    .get(usize::from(input))
                                    .copied();
                                c.b.input = c.a.input.map(tandem_core::MonitorInput::other);
                                c.b.screen = screen.then(|| "pantalla-1".to_owned());
                                c
                            })
                            .collect(),
                    }
                },
            )
    }

    proptest::proptest! {
        // Cada caso valida el archivo entero dos veces: con 64 alcanza y no frena la puerta.
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]

        /// Cualquier puesto escrito en `tandem.toml` (sobre el de la PC, con sus comentarios)
        /// se vuelve a leer igual, con su sello, y escribirlo otra vez no cambia el archivo.
        #[test]
        fn any_desk_round_trips_through_the_file(desk in any_desk(), rev in 0u64..1000) {
            let sync = version(desk, rev, Machine::A);
            let out = with_desk(PC, Machine::B, &sync).unwrap();
            let back = AgentConfig::from_toml(&out).unwrap();
            proptest::prop_assert_eq!(&back.desk, &sync.desk);
            proptest::prop_assert_eq!(back.desk_rev, rev);
            proptest::prop_assert!(!back.legacy_desk);
            proptest::prop_assert_eq!(with_desk(&out, Machine::B, &sync).unwrap(), out);
        }
    }

    /// Hallazgo del CI de Windows (PR #68): dos cambios del archivo a la vez (cambiar de lado
    /// y guardar el teclado) leían el mismo texto y el segundo pisaba al primero.
    #[test]
    fn concurrent_changes_do_not_lose_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tandem.toml");
        for i in 0..40 {
            std::fs::write(&path, "machine = \"a\"\n").unwrap();
            let cfg = AgentConfig::load(&path).unwrap().with_side(Machine::B);
            let sync = version(cfg.desk.clone(), 1, Machine::B);
            let barrier = std::sync::Barrier::new(2);
            std::thread::scope(|s| {
                s.spawn(|| {
                    barrier.wait();
                    set_sides(&path, Machine::B, None, &sync).unwrap();
                });
                s.spawn(|| {
                    barrier.wait();
                    set_keyboard(&path, Machine::A).unwrap();
                });
            });
            let saved = AgentConfig::load(&path).unwrap();
            assert_eq!(saved.machine, Machine::B, "vuelta {i}");
        }
    }
}
