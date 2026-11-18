//! `tandem.toml` from before sides A/B (v0.2, T-097): the sides were called `mac` and
//! `pc` and each machine said which it was (`machine = "mac"`).
//!
//! The migration happens in two steps, because the keys are not yet known when
//! reading the file:
//!
//! 1. When reading ([`sides_v02`]): this machine is left as `A` and the other as `B`, provisionally.
//!    The screens, the arrangement, the keyboard and the author of the last change are
//!    translated with that same rule, so the desk keeps saying the same.
//! 2. When the agent starts, with the identity and the trusted peer
//!    ([`crate::config::AgentConfig::with_side`]): if by the keys this machine is `B`, the
//!    sides are switched. The file is rewritten in the new format.

use toml::{Table, Value};

/// The former names.
const MAC: &str = "mac";
const PC: &str = "pc";

fn is_old(v: Option<&Value>) -> bool {
    matches!(v.and_then(Value::as_str), Some(MAC | PC))
}

/// Whether `table` (a whole `tandem.toml`) uses the former names on any side.
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

/// Translates `table` to the A/B sides format, with this machine as `A` (step 1 above).
/// Returns whether there was anything to translate. What is not one of the former names is left as
/// is (and the usual validation rejects it if wrong).
pub fn sides_v02(table: &mut Table) -> bool {
    if !is_v02(table) {
        return false;
    }
    // Without `machine` (or with an already new one), the former machines are taken as the Mac: that was
    // what v0.2 assumed when it was missing (the keyboard started on the Mac).
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
        // `server` is the former name of `keyboard`; if both are present, `keyboard` holds.
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
        // `mac_side` was the side of the PC the Mac is on; `a_side`, the side of B A is
        // on. If the Mac ended up as B, the side is inverted, and so is the offset.
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

/// `tandem.toml` from before the desk was editable from the UI (T-099): the lock shortcut and the
/// clipboard were in `[input]` (now `[shortcuts] lock` and `[prefs] clipboard`, of the
/// desk) and `[arrangement]` carried a manual offset (`offset`), which no longer exists
/// (D18). The arrangement the T-097 agent wrote without anyone declaring it (A on the
/// left) is taken as undeclared; one on the right is left with center alignment.
/// A v0.2 one (`mac_side`) says where the Mac is and is kept as is: it is translated by
/// [`sides_v02`] afterwards.
///
/// The clipboard is a module of each machine, and what is on or off for each is in
/// the desk (`[modules.<side>] clipboard`, D4): if this one had it off, that is a change
/// by it (`local`), so the other adopts it. Otherwise, both could end up with the same
/// version and a different desk. On is the default and is not written.
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
    // `true` if the value moved to its new section.
    let mut move_to = |section: &str, key: &str, value: Option<Value>| {
        let Some(value) = value else { return false };
        changed = true;
        if table.contains_key(section) {
            // The new one rules: the old one is redundant.
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
        // This machine's side; a v0.2 one (`mac`/`pc`) is left as `a` in
        // [`sides_v02`], just like it.
        // `B` also holds (as shown).
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

/// What [`desk_v03`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeskMigration {
    /// There was something in the old format.
    pub changed: bool,
    /// A value that belonged to this machine and was not the default moved into the desk.
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
            "the turned-off clipboard belonged to this machine"
        );
        assert!(
            t.get("arrangement").is_none(),
            "the default is not declared"
        );
        assert_eq!(t["shortcuts"]["lock"].as_array().unwrap().len(), 1);
        assert_eq!(t["modules"]["a"]["clipboard"].as_bool(), Some(false));
        assert_eq!(t["input"].as_table().unwrap().len(), 1);
        // A second time there is nothing to do.
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
        // On (the default) is nobody's change.
        let mut on: Table = "machine = \"b\"\n[input]\nclipboard = true\n"
            .parse()
            .unwrap();
        assert!(!desk_v03(&mut on).local);
        assert!(on.get("modules").is_none() && on.get("prefs").is_none());
        // Off on B goes to side B.
        let mut off: Table = "machine = \"b\"\n[input]\nclipboard = false\n"
            .parse()
            .unwrap();
        assert!(desk_v03(&mut off).local);
        assert_eq!(off["modules"]["b"]["clipboard"].as_bool(), Some(false));
        assert_eq!(right["arrangement"]["a_side"].as_str(), Some("right"));
        assert!(right["arrangement"].get("offset").is_none());
        assert!(right["shortcuts"]["lock"].as_array().unwrap().is_empty());
        // A new file stays the same.
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
        // The Mac (now B) to the left of the PC (now A): A ends up to the right of B.
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
