//! `tandem.toml` de antes de los lados A/B (v0.2, T-097): los lados se llamaban `mac` y
//! `pc` y cada máquina decía cuál era (`machine = "mac"`).
//!
//! La migración pasa en dos tiempos, porque al leer el archivo todavía no se conocen las
//! claves:
//!
//! 1. Al leer ([`sides_v02`]): esta máquina queda como `A` y la otra como `B`, de forma
//!    provisional. Las pantallas, el acomodo, el teclado y el autor del último cambio se
//!    traducen con esa misma regla, así que el puesto sigue diciendo lo mismo.
//! 2. Al arrancar el agente, con la identidad y el par de la confianza
//!    ([`crate::config::AgentConfig::with_side`]): si por las claves esta máquina es `B`, se
//!    cambian los lados. El archivo se reescribe en el formato nuevo.

use toml::{Table, Value};

/// Los nombres de antes.
const MAC: &str = "mac";
const PC: &str = "pc";

fn is_old(v: Option<&Value>) -> bool {
    matches!(v.and_then(Value::as_str), Some(MAC | PC))
}

/// Si `table` (un `tandem.toml` entero) usa los nombres de antes en algún lado.
pub fn is_v02(table: &Table) -> bool {
    let screens = table
        .get("screens")
        .and_then(Value::as_array)
        .is_some_and(|s| {
            s.iter()
                .any(|t| is_old(t.as_table().and_then(|t| t.get("machine"))))
        });
    let input = table.get("input").and_then(Value::as_table);
    let arrangement = table.get("arrangement").and_then(Value::as_table);
    is_old(table.get("machine"))
        || is_old(table.get("desk_origin"))
        || screens
        || input.is_some_and(|i| is_old(i.get("keyboard")) || is_old(i.get("server")))
        || arrangement.is_some_and(|a| a.contains_key("mac_side"))
}

/// Traduce `table` al formato de lados A/B, con esta máquina como `A` (paso 1 de arriba).
/// Devuelve si había algo que traducir. Lo que no sea de los nombres de antes se deja como
/// está (y la validación de siempre lo rechaza si está mal).
pub fn sides_v02(table: &mut Table) -> bool {
    if !is_v02(table) {
        return false;
    }
    // Sin `machine` (o con uno ya nuevo), las máquinas de antes se toman por la Mac: era lo
    // que suponía v0.2 cuando faltaba (el teclado arrancaba en la Mac).
    let local = match table.get("machine").and_then(Value::as_str) {
        Some(PC) => PC,
        _ => MAC,
    };
    let side = |old: &str| -> Option<&'static str> {
        match old {
            o if o == local => Some("a"),
            MAC | PC => Some("b"),
            _ => None,
        }
    };
    let relabel = |v: &mut Value| {
        if let Some(new) = v.as_str().and_then(side) {
            *v = Value::String(new.to_owned());
        }
    };
    table.insert("machine".into(), Value::String("a".into()));
    if let Some(v) = table.get_mut("desk_origin") {
        relabel(v);
    }
    if let Some(Value::Array(screens)) = table.get_mut("screens") {
        for s in screens {
            if let Some(v) = s.as_table_mut().and_then(|t| t.get_mut("machine")) {
                relabel(v);
            }
        }
    }
    if let Some(Value::Table(input)) = table.get_mut("input") {
        // `server` es el nombre de antes de `keyboard`; si están los dos, vale `keyboard`.
        let server = input.remove("server");
        if !input.contains_key("keyboard")
            && let Some(s) = server
        {
            input.insert("keyboard".into(), s);
        }
        if let Some(v) = input.get_mut("keyboard") {
            relabel(v);
        }
    }
    if let Some(Value::Table(arr)) = table.get_mut("arrangement")
        && let Some(mac_side) = arr.remove("mac_side")
    {
        // `mac_side` era el lado de la PC en que está la Mac; `a_side`, el de B en que
        // está A. Si la Mac quedó como B, el lado se invierte, y el desplazamiento también.
        let mac_is_a = local == MAC;
        let side = match (mac_side.as_str(), mac_is_a) {
            (Some("left"), true) | (Some("right"), false) => Value::String("left".into()),
            (Some("right"), true) | (Some("left"), false) => Value::String("right".into()),
            _ => mac_side,
        };
        arr.insert("a_side".into(), side);
        if !mac_is_a && let Some(Value::Integer(off)) = arr.get_mut("offset") {
            *off = off.saturating_neg();
        }
    }
    true
}

/// `tandem.toml` de antes del puesto editable desde la UI (T-099): el atajo de bloqueo y el
/// portapapeles estaban en `[input]` (ahora `[shortcuts] lock` y `[prefs] clipboard`, del
/// puesto) y `[arrangement]` llevaba un desplazamiento a mano (`offset`), que ya no existe
/// (D18). El acomodo que escribía el agente de T-097 sin que nadie lo declarara (A a la
/// izquierda) se toma por no declarado; uno a la derecha queda con alineación al centro.
/// Uno de v0.2 (`mac_side`) dice dónde está la Mac y se conserva tal cual: lo traduce
/// [`sides_v02`] después.
///
/// El portapapeles es un módulo de cada máquina, y lo encendido o apagado de cada una está en
/// el puesto (`[modules.<lado>] clipboard`, D4): si esta lo tenía apagado, eso es un cambio
/// suyo (`local`), para que la otra lo adopte. Si no, las dos podrían quedar con la misma
/// versión y distinto puesto. Encendido es lo de omisión y no se escribe.
pub fn desk_v03(table: &mut Table) -> DeskMigration {
    let mut changed = false;
    if let Some(Value::Table(arr)) = table.get_mut("arrangement")
        && arr.remove("offset").is_some()
    {
        changed = true;
        let left = !arr.contains_key("mac_side")
            && arr
                .get("a_side")
                .and_then(Value::as_str)
                .is_none_or(|s| s == "left");
        if left {
            table.remove("arrangement");
        }
    }
    let (lock, clipboard) = match table.get_mut("input") {
        Some(Value::Table(input)) => (input.remove("lock_shortcut"), input.remove("clipboard")),
        _ => (None, None),
    };
    // `true` si el valor pasó a su sección nueva.
    let mut move_to = |section: &str, key: &str, value: Option<Value>| {
        let Some(value) = value else { return false };
        changed = true;
        if table.contains_key(section) {
            // Lo nuevo manda: lo de antes sobra.
            return false;
        }
        let mut t = Table::new();
        t.insert(key.into(), value);
        table.insert(section.into(), Value::Table(t));
        true
    };
    move_to("shortcuts", "lock", lock);
    if clipboard.is_some() {
        changed = true;
    }
    let mut local = false;
    if clipboard.as_ref().and_then(Value::as_bool) == Some(false) {
        // El lado de esta máquina; uno de v0.2 (`mac`/`pc`) queda como `a` en
        // [`sides_v02`], igual que ella.
        // `B` también vale (como se muestra).
        let side = match table.get("machine").and_then(Value::as_str) {
            Some("b" | "B") => "b",
            _ => "a",
        };
        let modules = table
            .entry("modules")
            .or_insert_with(|| Value::Table(Table::new()));
        if let Some(modules) = modules.as_table_mut() {
            let mine = modules
                .entry(side)
                .or_insert_with(|| Value::Table(Table::new()));
            if let Some(mine) = mine.as_table_mut()
                && !mine.contains_key("clipboard")
            {
                mine.insert("clipboard".into(), Value::Boolean(false));
                local = true;
            }
        }
    }
    DeskMigration { changed, local }
}

/// Lo que hizo [`desk_v03`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeskMigration {
    /// Había algo del formato de antes.
    pub changed: bool,
    /// Pasó al puesto un valor que era de esta máquina y no el de omisión.
    pub local: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desk_from_before_t099_moves_to_its_sections() {
        let mut t: Table = r#"
            machine = "a"
            [input]
            keyboard = "a"
            lock_shortcut = ["KeyL"]
            clipboard = false
            [arrangement]
            a_side = "left"
            offset = 0
            "#
        .parse()
        .unwrap();
        assert_eq!(
            desk_v03(&mut t),
            DeskMigration {
                changed: true,
                local: true
            },
            "el portapapeles apagado era de esta máquina"
        );
        assert!(
            t.get("arrangement").is_none(),
            "el de omisión no se declara"
        );
        assert_eq!(t["shortcuts"]["lock"].as_array().unwrap().len(), 1);
        assert_eq!(t["modules"]["a"]["clipboard"].as_bool(), Some(false));
        assert_eq!(t["input"].as_table().unwrap().len(), 1);
        // Una segunda vez no hay nada que hacer.
        assert_eq!(desk_v03(&mut t), DeskMigration::default());

        let mut right: Table = "[arrangement]\na_side = \"right\"\noffset = 40\n[shortcuts]\nlock = []\n[input]\nlock_shortcut = [\"KeyL\"]\n"
            .parse()
            .unwrap();
        assert_eq!(
            desk_v03(&mut right),
            DeskMigration {
                changed: true,
                local: false
            }
        );
        // Encendido (lo de omisión) no es un cambio de nadie.
        let mut on: Table = "machine = \"b\"\n[input]\nclipboard = true\n"
            .parse()
            .unwrap();
        assert!(!desk_v03(&mut on).local);
        assert!(on.get("modules").is_none() && on.get("prefs").is_none());
        // Apagado en B va al lado B.
        let mut off: Table = "machine = \"b\"\n[input]\nclipboard = false\n"
            .parse()
            .unwrap();
        assert!(desk_v03(&mut off).local);
        assert_eq!(off["modules"]["b"]["clipboard"].as_bool(), Some(false));
        assert_eq!(right["arrangement"]["a_side"].as_str(), Some("right"));
        assert!(right["arrangement"].get("offset").is_none());
        assert!(right["shortcuts"]["lock"].as_array().unwrap().is_empty());
        // Un archivo nuevo queda igual.
        let text = "[arrangement]\na_side = \"up\"\nalign = \"end\"\n";
        let mut new: Table = text.parse().unwrap();
        assert_eq!(desk_v03(&mut new), DeskMigration::default());
        assert_eq!(new, text.parse::<Table>().unwrap());
    }

    fn migrated(text: &str) -> (bool, Table) {
        let mut t: Table = text.parse().unwrap();
        let changed = sides_v02(&mut t);
        (changed, t)
    }

    #[test]
    fn a_new_config_is_left_alone() {
        let text = "machine = \"b\"\n[[screens]]\nmachine = \"a\"\nid = \"x\"\n";
        let (changed, t) = migrated(text);
        assert!(!changed);
        assert_eq!(t, text.parse::<Table>().unwrap());
    }

    #[test]
    fn the_pc_becomes_a_and_the_desk_keeps_saying_the_same() {
        let (changed, t) = migrated(
            r#"
            machine = "pc"
            desk_origin = "mac"
            [input]
            server = "mac"
            [[screens]]
            machine = "pc"
            id = "34"
            [[screens]]
            machine = "mac"
            id = "laptop"
            [arrangement]
            mac_side = "left"
            offset = 30
            "#,
        );
        assert!(changed);
        assert_eq!(t["machine"].as_str(), Some("a"));
        assert_eq!(t["desk_origin"].as_str(), Some("b"));
        assert_eq!(t["input"]["keyboard"].as_str(), Some("b"));
        assert!(t["input"].get("server").is_none());
        let screens = t["screens"].as_array().unwrap();
        assert_eq!(screens[0]["machine"].as_str(), Some("a"));
        assert_eq!(screens[1]["machine"].as_str(), Some("b"));
        // La Mac (ahora B) a la izquierda de la PC (ahora A): A queda a la derecha de B.
        assert_eq!(t["arrangement"]["a_side"].as_str(), Some("right"));
        assert_eq!(t["arrangement"]["offset"].as_integer(), Some(-30));
        assert!(t["arrangement"].get("mac_side").is_none());
    }

    #[test]
    fn the_mac_becomes_a_with_its_arrangement_as_is() {
        let (changed, t) = migrated(
            "machine = \"mac\"\n[input]\nkeyboard = \"pc\"\n[arrangement]\nmac_side = \"right\"\noffset = 5\n",
        );
        assert!(changed);
        assert_eq!(t["input"]["keyboard"].as_str(), Some("b"));
        assert_eq!(t["arrangement"]["a_side"].as_str(), Some("right"));
        assert_eq!(t["arrangement"]["offset"].as_integer(), Some(5));
    }
}
