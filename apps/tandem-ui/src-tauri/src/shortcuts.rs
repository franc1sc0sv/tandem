//! Global keyboard shortcuts: which exist, how they are written and where they are stored.
//!
//! They are written as the Tauri accelerator with the physical key names
//! (`KeyboardEvent.code`): `Ctrl+Alt+Digit1`. By physical code because on macOS ⌥ changes the
//! key's character and on the ES/LatAm layout several symbols move.
//!
//! They are stored in `ui.toml`, next to the agent's configuration. A damaged file does not leave
//! the app without shortcuts: the usual ones are used and the window says why.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Name of the UI preferences file, in the configuration directory.
pub const UI_FILE: &str = "ui.toml";

/// What a shortcut can trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutAction {
    /// Show or hide the main window. The shortcuts of the monitor modes
    /// went away with them (DESIGN §8); a `ui.toml` that still has them is read all the same.
    ToggleWindow,
}

/// The shortcuts of the v0.1 modes: ignored when reading `ui.toml`.
const RETIRED: [&str; 3] = ["solo_mac", "shared", "solo_pc"];

impl ShortcutAction {
    pub const ALL: [ShortcutAction; 1] = [ShortcutAction::ToggleWindow];

    pub fn label(self) -> &'static str {
        match self {
            ShortcutAction::ToggleWindow => "Show or hide Tandem",
        }
    }
}

/// Keys accepted as the main key. All of them are understood by Tauri's shortcut
/// registry (`global-hotkey`) on the three systems.
const KEYS: &[&str] = &[
    "Backquote",
    "Minus",
    "Equal",
    "BracketLeft",
    "BracketRight",
    "Backslash",
    "Semicolon",
    "Quote",
    "Comma",
    "Period",
    "Slash",
    "Space",
    "Enter",
    "Tab",
    "Escape",
    "Backspace",
    "Delete",
    "Insert",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
];

fn is_key(code: &str) -> bool {
    let numbered = |prefix: &str, max: u32| {
        code.strip_prefix(prefix)
            .and_then(|n| {
                // No leading zeros or signs: a single form per key.
                (!n.starts_with('0') || n == "0")
                    .then(|| n.parse::<u32>().ok())
                    .flatten()
            })
            .is_some_and(|n| n <= max)
    };
    let letter = code
        .strip_prefix("Key")
        .is_some_and(|l| l.len() == 1 && l.bytes().all(|b| b.is_ascii_uppercase()));
    KEYS.contains(&code)
        || letter
        || numbered("Digit", 9)
        || numbered("Numpad", 9)
        || (numbered("F", 24) && code != "F0")
}

/// A key combination in canonical form: `Ctrl+Alt+Shift+Super+<key>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Accel {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// ⌘ on macOS, the Windows key on Windows.
    pub sup: bool,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccelError {
    #[error("empty shortcut")]
    Empty,
    #[error("\"{0}\" is not a key Tandem knows how to register")]
    UnknownKey(String),
    #[error("\"{0}\" repeats a modifier")]
    RepeatedModifier(String),
    #[error("\"{0}\" has more than one main key or puts it before the modifiers")]
    Shape(String),
    #[error("\"{0}\" needs Ctrl, Alt or {sup}: with Shift alone it would swallow what you type", sup = SUPER_NAME)]
    NeedsModifier(String),
}

#[cfg(target_os = "macos")]
const SUPER_NAME: &str = "⌘";
#[cfg(not(target_os = "macos"))]
const SUPER_NAME: &str = "Win";

impl FromStr for Accel {
    type Err = AccelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(AccelError::Empty);
        }
        let mut a = Accel {
            ctrl: false,
            alt: false,
            shift: false,
            sup: false,
            key: String::new(),
        };
        let tokens: Vec<&str> = s.split('+').map(str::trim).collect();
        let (key, mods) = tokens.split_last().ok_or(AccelError::Empty)?;
        for m in mods {
            let flag = match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut a.ctrl,
                "alt" | "option" => &mut a.alt,
                "shift" => &mut a.shift,
                "super" | "cmd" | "command" | "meta" | "win" => &mut a.sup,
                _ if m.is_empty() => return Err(AccelError::Shape(s.into())),
                _ if is_key(m) => return Err(AccelError::Shape(s.into())),
                _ => return Err(AccelError::UnknownKey((*m).into())),
            };
            if *flag {
                return Err(AccelError::RepeatedModifier(s.into()));
            }
            *flag = true;
        }
        if !is_key(key) {
            return Err(if key.is_empty() {
                AccelError::Shape(s.into())
            } else {
                AccelError::UnknownKey((*key).into())
            });
        }
        a.key = (*key).into();
        if !(a.ctrl || a.alt || a.sup) {
            return Err(AccelError::NeedsModifier(s.into()));
        }
        Ok(a)
    }
}

impl fmt::Display for Accel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.sup, "Super"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&self.key)
    }
}

/// Which shortcut each action has (`None`: no shortcut).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcuts(BTreeMap<ShortcutAction, Option<Accel>>);

/// How shortcuts travel between the window and the core, and how they are stored: action →
/// canonical combination, or `null` / empty string if it has none.
pub type Bindings = BTreeMap<ShortcutAction, Option<String>>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BindingsError {
    #[error("{}: {}", .0.label(), .1)]
    Invalid(ShortcutAction, AccelError),
    #[error("{} and {} use the same shortcut ({})", .0.label(), .1.label(), .2)]
    Duplicate(ShortcutAction, ShortcutAction, String),
}

impl Default for Shortcuts {
    /// ⌃⌥0 for the window.
    fn default() -> Self {
        let accel = |key: &str| {
            Some(Accel {
                ctrl: true,
                alt: true,
                shift: false,
                sup: false,
                key: key.into(),
            })
        };
        Shortcuts(BTreeMap::from([(
            ShortcutAction::ToggleWindow,
            accel("Digit0"),
        )]))
    }
}

impl Shortcuts {
    /// Reads what the window or the file sends. Actions that do not appear keep their
    /// usual shortcut; to remove one it has to be sent empty.
    pub fn from_bindings(b: &Bindings) -> Result<Self, BindingsError> {
        let mut out = Shortcuts::default();
        for (&action, accel) in b {
            let accel = match accel.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(s) => Some(s.parse().map_err(|e| BindingsError::Invalid(action, e))?),
            };
            out.0.insert(action, accel);
        }
        let mut seen: BTreeMap<&Accel, ShortcutAction> = BTreeMap::new();
        for (&action, accel) in &out.0 {
            if let Some(accel) = accel {
                if let Some(&first) = seen.get(accel) {
                    return Err(BindingsError::Duplicate(first, action, accel.to_string()));
                }
                seen.insert(accel, action);
            }
        }
        Ok(out)
    }

    pub fn bindings(&self) -> Bindings {
        self.0
            .iter()
            .map(|(&a, accel)| (a, accel.as_ref().map(Accel::to_string)))
            .collect()
    }

    pub fn get(&self, action: ShortcutAction) -> Option<&Accel> {
        self.0.get(&action).and_then(Option::as_ref)
    }

    /// The actions with a shortcut, in order.
    pub fn active(&self) -> impl Iterator<Item = (ShortcutAction, &Accel)> {
        self.0
            .iter()
            .filter_map(|(&a, accel)| accel.as_ref().map(|accel| (a, accel)))
    }
}

// `BTreeMap<&Accel, _>` to detect duplicates with a stable order in the message.
impl PartialOrd for Accel {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Accel {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.ctrl, self.alt, self.shift, self.sup, &self.key).cmp(&(
            other.ctrl,
            other.alt,
            other.shift,
            other.sup,
            &other.key,
        ))
    }
}

/// Shape of `ui.toml`. Only the shortcuts section for now; the other keys are ignored.
#[derive(Debug, Default, Serialize, Deserialize)]
struct UiFile {
    #[serde(default)]
    shortcuts: BTreeMap<ShortcutAction, String>,
}

/// What was read from the file, and why it could not be used if it could not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub shortcuts: Shortcuts,
    pub warning: Option<String>,
}

/// `ui.toml` in the same directory as the agent's configuration.
pub fn ui_file_for(agent_config: &Path) -> PathBuf {
    agent_config
        .parent()
        .map_or_else(|| PathBuf::from(UI_FILE), |d| d.join(UI_FILE))
}

pub fn load(path: &Path) -> Loaded {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Loaded {
                shortcuts: Shortcuts::default(),
                warning: None,
            };
        }
        Err(e) => return fallback(path, &e.to_string()),
    };
    let mut table: toml::Table = match toml::from_str(&text) {
        Ok(t) => t,
        Err(e) => return fallback(path, e.message()),
    };
    if let Some(toml::Value::Table(shortcuts)) = table.get_mut("shortcuts") {
        shortcuts.retain(|k, _| !RETIRED.contains(&k));
    }
    let file: UiFile = match UiFile::deserialize(table) {
        Ok(f) => f,
        Err(e) => return fallback(path, e.message()),
    };
    let bindings: Bindings = file
        .shortcuts
        .into_iter()
        .map(|(a, s)| (a, Some(s)))
        .collect();
    match Shortcuts::from_bindings(&bindings) {
        Ok(shortcuts) => Loaded {
            shortcuts,
            warning: None,
        },
        Err(e) => fallback(path, &e.to_string()),
    }
}

fn fallback(path: &Path, why: &str) -> Loaded {
    Loaded {
        shortcuts: Shortcuts::default(),
        warning: Some(format!(
            "could not read {}: {why}; the usual shortcuts are used",
            path.display()
        )),
    }
}

/// Saves the shortcuts. A removed shortcut is saved empty so the usual one does not come back.
pub fn save(path: &Path, shortcuts: &Shortcuts) -> io::Result<()> {
    let file = UiFile {
        shortcuts: shortcuts
            .bindings()
            .into_iter()
            .map(|(a, s)| (a, s.unwrap_or_default()))
            .collect(),
    };
    let text = toml::to_string(&file).map_err(io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Write separately and rename: a half-done cut does not leave the file empty.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// What the window shows of the shortcuts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShortcutsView {
    pub bindings: Bindings,
    /// Those the system did not let register (another app uses them, Wayland...), with the reason.
    /// The window handles them itself while it has focus.
    pub failed: BTreeMap<ShortcutAction, String>,
    /// Notice when reading the file, if there was one.
    pub warning: Option<String>,
    pub file: Option<String>,
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn accel(s: &str) -> Accel {
        s.parse().unwrap()
    }

    #[test]
    fn canonical_form() {
        assert_eq!(accel("Ctrl+Alt+Digit1").to_string(), "Ctrl+Alt+Digit1");
        // Modifiers in any order and with other names.
        assert_eq!(accel("alt + control+Digit1").to_string(), "Ctrl+Alt+Digit1");
        assert_eq!(accel("Cmd+Shift+KeyK").to_string(), "Shift+Super+KeyK");
        assert_eq!(accel("Option+F12").to_string(), "Alt+F12");
        assert_eq!(accel("Ctrl+Numpad3").to_string(), "Ctrl+Numpad3");
        assert_eq!(accel("Ctrl+ArrowLeft").to_string(), "Ctrl+ArrowLeft");
    }

    #[test]
    fn rejects_what_cannot_be_registered() {
        let err = |s: &str| s.parse::<Accel>().unwrap_err();
        assert_eq!(err(""), AccelError::Empty);
        assert_eq!(err("  "), AccelError::Empty);
        assert!(matches!(err("Ctrl+Alt+1"), AccelError::UnknownKey(k) if k == "1"));
        assert!(matches!(err("Ctrl+Alt+Digit10"), AccelError::UnknownKey(_)));
        assert!(matches!(err("Ctrl+Keya"), AccelError::UnknownKey(_)));
        assert!(matches!(err("Ctrl+F0"), AccelError::UnknownKey(_)));
        assert!(matches!(err("Ctrl+F25"), AccelError::UnknownKey(_)));
        assert!(matches!(err("Ctrl+F01"), AccelError::UnknownKey(_)));
        assert!(matches!(err("Hyper+KeyA"), AccelError::UnknownKey(_)));
        assert!(matches!(
            err("Ctrl+Ctrl+KeyA"),
            AccelError::RepeatedModifier(_)
        ));
        assert!(matches!(err("Ctrl+KeyA+KeyB"), AccelError::Shape(_)));
        assert!(matches!(err("Ctrl++KeyA"), AccelError::Shape(_)));
        assert!(matches!(err("Ctrl+"), AccelError::Shape(_)));
        assert!(matches!(err("Shift+KeyA"), AccelError::NeedsModifier(_)));
        assert!(matches!(err("KeyA"), AccelError::NeedsModifier(_)));
        assert!(matches!(err("Ctrl"), AccelError::UnknownKey(_)));
    }

    #[test]
    fn the_default_is_the_window_shortcut() {
        let d = Shortcuts::default();
        assert_eq!(
            d.get(ShortcutAction::ToggleWindow).map(Accel::to_string),
            Some("Ctrl+Alt+Digit0".into())
        );
        assert_eq!(d.active().count(), 1);
    }

    #[test]
    fn bindings_round_trip() {
        let b: Bindings = [(ShortcutAction::ToggleWindow, Some("cmd+alt+KeyP".into()))].into();
        let s = Shortcuts::from_bindings(&b).unwrap();
        let back = s.bindings();
        assert_eq!(
            back[&ShortcutAction::ToggleWindow].as_deref(),
            Some("Alt+Super+KeyP")
        );
        assert_eq!(Shortcuts::from_bindings(&back).unwrap(), s);
    }

    #[test]
    fn empty_string_removes_a_shortcut() {
        let b: Bindings = [(ShortcutAction::ToggleWindow, Some("  ".into()))].into();
        let s = Shortcuts::from_bindings(&b).unwrap();
        assert_eq!(s.get(ShortcutAction::ToggleWindow), None);
    }

    #[test]
    fn invalid_binding_names_the_action() {
        let b: Bindings = [(ShortcutAction::ToggleWindow, Some("Shift+KeyA".into()))].into();
        let e = Shortcuts::from_bindings(&b).unwrap_err();
        assert!(e.to_string().starts_with("Show or hide Tandem: "), "{e}");
    }

    #[test]
    fn json_is_what_the_frontend_reads() {
        let view = ShortcutsView {
            bindings: Shortcuts::default().bindings(),
            failed: [(ShortcutAction::ToggleWindow, "busy".into())].into(),
            warning: None,
            file: Some("/x/ui.toml".into()),
        };
        assert_eq!(
            serde_json::to_value(&view).unwrap(),
            serde_json::json!({
                "bindings": { "toggle_window": "Ctrl+Alt+Digit0" },
                "failed": { "toggle_window": "busy" },
                "warning": null,
                "file": "/x/ui.toml"
            })
        );
        let from: Bindings =
            serde_json::from_value(serde_json::json!({ "toggle_window": null })).unwrap();
        assert_eq!(from[&ShortcutAction::ToggleWindow], None);
    }

    /// A v0.1 `ui.toml`, with the modes' shortcuts, is read without notices.
    #[test]
    fn the_old_mode_shortcuts_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui.toml");
        std::fs::write(
            &path,
            "[shortcuts]\nsolo_mac = \"Ctrl+Alt+Digit1\"\nshared = \"Ctrl+Alt+Digit2\"\nsolo_pc = \"Ctrl+Alt+Digit3\"\ntoggle_window = \"Ctrl+Alt+KeyT\"\n",
        )
        .unwrap();
        let l = load(&path);
        assert_eq!(l.warning, None);
        assert_eq!(
            l.shortcuts
                .get(ShortcutAction::ToggleWindow)
                .map(Accel::to_string),
            Some("Ctrl+Alt+KeyT".into())
        );
    }

    #[test]
    fn missing_file_means_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let l = load(&dir.path().join("ui.toml"));
        assert_eq!(l.shortcuts, Shortcuts::default());
        assert_eq!(l.warning, None);
    }

    #[test]
    fn saved_shortcuts_load_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("ui.toml");
        let b: Bindings = [(ShortcutAction::ToggleWindow, None)].into();
        let s = Shortcuts::from_bindings(&b).unwrap();
        save(&path, &s).unwrap();
        let l = load(&path);
        assert_eq!(l.warning, None);
        assert_eq!(l.shortcuts, s);
        // Removed stays removed: the usual one does not come back.
        assert_eq!(l.shortcuts.get(ShortcutAction::ToggleWindow), None);
        assert!(!path.with_extension("toml.tmp").exists());
    }

    #[test]
    fn a_hand_edited_file_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui.toml");
        std::fs::write(
            &path,
            "other = 1\n[shortcuts]\ntoggle_window = \"Alt+Ctrl+KeyW\"\n",
        )
        .unwrap();
        let l = load(&path);
        assert_eq!(l.warning, None);
        assert_eq!(
            l.shortcuts
                .get(ShortcutAction::ToggleWindow)
                .map(Accel::to_string),
            Some("Ctrl+Alt+KeyW".into())
        );
    }

    #[test]
    fn a_broken_file_falls_back_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui.toml");
        for bad in [
            "[shortcuts\n",
            "[shortcuts]\ntoggle_window = \"KeyA\"\n",
            "[shortcuts]\ndoes_not_exist = \"Ctrl+KeyA\"\n",
        ] {
            std::fs::write(&path, bad).unwrap();
            let l = load(&path);
            assert_eq!(l.shortcuts, Shortcuts::default(), "{bad}");
            let w = l.warning.unwrap();
            assert!(w.contains("ui.toml") && w.contains("the usual"), "{w}");
        }
    }

    #[test]
    fn ui_file_sits_next_to_the_agent_config() {
        assert_eq!(
            ui_file_for(Path::new("/c/Tandem/tandem.toml")),
            Path::new("/c/Tandem/ui.toml")
        );
    }

    fn any_accel() -> impl Strategy<Value = Accel> {
        let keys = prop_oneof![
            (0u8..10).prop_map(|d| format!("Digit{d}")),
            (b'A'..=b'Z').prop_map(|c| format!("Key{}", c as char)),
            (1u8..=24).prop_map(|n| format!("F{n}")),
            proptest::sample::select(KEYS).prop_map(str::to_owned),
        ];
        (any::<[bool; 4]>(), keys)
            .prop_filter("Ctrl, Alt or Super is needed", |(m, _)| {
                m[0] || m[1] || m[3]
            })
            .prop_map(|(m, key)| Accel {
                ctrl: m[0],
                alt: m[1],
                shift: m[2],
                sup: m[3],
                key,
            })
    }

    proptest! {
        /// The canonical form is read back the same, and in any modifier order.
        #[test]
        fn canonical_round_trip(a in any_accel(), rev in any::<bool>()) {
            let text = a.to_string();
            prop_assert_eq!(text.parse::<Accel>().unwrap(), a.clone());
            if rev {
                let mut parts: Vec<&str> = text.split('+').collect();
                let key = parts.pop().unwrap();
                parts.reverse();
                parts.push(key);
                prop_assert_eq!(parts.join("+").parse::<Accel>().unwrap(), a);
            }
        }

        /// What comes in through the window never makes the reader fall over: it is accepted or explained.
        #[test]
        fn parsing_never_panics(s in "\\PC{0,24}") {
            let _ = s.parse::<Accel>();
        }
    }
}
