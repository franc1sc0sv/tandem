//! Atajos de teclado globales: cuáles hay, cómo se escriben y dónde se guardan.
//!
//! Se escriben como el acelerador de Tauri con los nombres físicos de las teclas
//! (`KeyboardEvent.code`): `Ctrl+Alt+Digit1`. Por código físico porque en macOS ⌥ cambia el
//! carácter de la tecla y en la distribución ES/LatAm varios símbolos se mueven.
//!
//! Se guardan en `ui.toml`, junto a la configuración del agente. Un archivo dañado no deja
//! a la app sin atajos: se usan los de siempre y la ventana dice por qué.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Nombre del archivo de preferencias de la UI, en el directorio de configuración.
pub const UI_FILE: &str = "ui.toml";

/// Lo que puede disparar un atajo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutAction {
    SoloMac,
    Shared,
    SoloPc,
    /// Mostrar u ocultar la ventana principal.
    ToggleWindow,
}

impl ShortcutAction {
    pub const ALL: [ShortcutAction; 4] = [
        ShortcutAction::SoloMac,
        ShortcutAction::Shared,
        ShortcutAction::SoloPc,
        ShortcutAction::ToggleWindow,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ShortcutAction::SoloMac => "Solo Mac",
            ShortcutAction::Shared => "Compartido",
            ShortcutAction::SoloPc => "Solo Windows",
            ShortcutAction::ToggleWindow => "Mostrar u ocultar Tandem",
        }
    }
}

/// Teclas que se aceptan como tecla principal. Todas las entiende el registro de atajos de
/// Tauri (`global-hotkey`) en los tres sistemas.
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
                // Sin ceros a la izquierda ni signos: una sola forma por tecla.
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

/// Una combinación de teclas en forma canónica: `Ctrl+Alt+Shift+Super+<tecla>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Accel {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// ⌘ en macOS, la tecla Windows en Windows.
    pub sup: bool,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccelError {
    #[error("atajo vacío")]
    Empty,
    #[error("«{0}» no es una tecla que Tandem sepa registrar")]
    UnknownKey(String),
    #[error("«{0}» repite un modificador")]
    RepeatedModifier(String),
    #[error("«{0}» lleva más de una tecla principal o la pone antes de los modificadores")]
    Shape(String),
    #[error("«{0}» necesita Ctrl, Alt o {sup}: solo con Mayúsculas se comería lo que escribes", sup = SUPER_NAME)]
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

/// Qué atajo tiene cada acción (`None`: sin atajo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcuts(BTreeMap<ShortcutAction, Option<Accel>>);

/// Cómo viajan los atajos entre la ventana y el núcleo, y cómo se guardan: acción →
/// combinación canónica, o `null` / cadena vacía si no tiene.
pub type Bindings = BTreeMap<ShortcutAction, Option<String>>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BindingsError {
    #[error("{}: {}", .0.label(), .1)]
    Invalid(ShortcutAction, AccelError),
    #[error("{} y {} usan el mismo atajo ({})", .0.label(), .1.label(), .2)]
    Duplicate(ShortcutAction, ShortcutAction, String),
}

impl Default for Shortcuts {
    /// ⌃⌥1/2/3 para los modos (los de la escena) y ⌃⌥0 para la ventana.
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
        Shortcuts(BTreeMap::from([
            (ShortcutAction::SoloMac, accel("Digit1")),
            (ShortcutAction::Shared, accel("Digit2")),
            (ShortcutAction::SoloPc, accel("Digit3")),
            (ShortcutAction::ToggleWindow, accel("Digit0")),
        ]))
    }
}

impl Shortcuts {
    /// Lee lo que manda la ventana o el archivo. Las acciones que no aparecen conservan su
    /// atajo de siempre; para quitar uno hay que mandarlo vacío.
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

    /// Las acciones con atajo, en orden.
    pub fn active(&self) -> impl Iterator<Item = (ShortcutAction, &Accel)> {
        self.0
            .iter()
            .filter_map(|(&a, accel)| accel.as_ref().map(|accel| (a, accel)))
    }
}

// `BTreeMap<&Accel, _>` para detectar duplicados con orden estable en el mensaje.
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

/// Forma de `ui.toml`. Solo la sección de atajos por ahora; las demás claves se ignoran.
#[derive(Debug, Default, Serialize, Deserialize)]
struct UiFile {
    #[serde(default)]
    shortcuts: BTreeMap<ShortcutAction, String>,
}

/// Lo que se leyó del archivo, y por qué no se pudo usar si no se pudo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub shortcuts: Shortcuts,
    pub warning: Option<String>,
}

/// `ui.toml` en el mismo directorio que la configuración del agente.
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
    let file: UiFile = match toml::from_str(&text) {
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
            "no se pudo leer {}: {why}; se usan los atajos de siempre",
            path.display()
        )),
    }
}

/// Guarda los atajos. Un atajo quitado se guarda vacío para que no vuelva el de siempre.
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
    // Escribir aparte y renombrar: un corte a medias no deja el archivo vacío.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Lo que la ventana muestra de los atajos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShortcutsView {
    pub bindings: Bindings,
    /// Los que el sistema no dejó registrar (otra app los usa, Wayland...), con la razón.
    /// La ventana los atiende ella misma mientras tenga el foco.
    pub failed: BTreeMap<ShortcutAction, String>,
    /// Aviso al leer el archivo, si lo hubo.
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
        // Los modificadores en cualquier orden y con otros nombres.
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
    fn defaults_are_the_scene_shortcuts() {
        let d = Shortcuts::default();
        assert_eq!(
            d.get(ShortcutAction::SoloMac).map(Accel::to_string),
            Some("Ctrl+Alt+Digit1".into())
        );
        assert_eq!(
            d.get(ShortcutAction::Shared).map(Accel::to_string),
            Some("Ctrl+Alt+Digit2".into())
        );
        assert_eq!(
            d.get(ShortcutAction::SoloPc).map(Accel::to_string),
            Some("Ctrl+Alt+Digit3".into())
        );
        assert_eq!(d.active().count(), 4);
    }

    #[test]
    fn bindings_round_trip_and_keep_missing_defaults() {
        let b: Bindings = [
            (ShortcutAction::SoloPc, Some("cmd+alt+KeyP".into())),
            (ShortcutAction::ToggleWindow, None),
        ]
        .into();
        let s = Shortcuts::from_bindings(&b).unwrap();
        let back = s.bindings();
        assert_eq!(
            back[&ShortcutAction::SoloPc].as_deref(),
            Some("Alt+Super+KeyP")
        );
        assert_eq!(back[&ShortcutAction::ToggleWindow], None);
        assert_eq!(
            back[&ShortcutAction::SoloMac].as_deref(),
            Some("Ctrl+Alt+Digit1")
        );
        assert_eq!(Shortcuts::from_bindings(&back).unwrap(), s);
    }

    #[test]
    fn empty_string_removes_a_shortcut() {
        let b: Bindings = [(ShortcutAction::Shared, Some("  ".into()))].into();
        let s = Shortcuts::from_bindings(&b).unwrap();
        assert_eq!(s.get(ShortcutAction::Shared), None);
    }

    #[test]
    fn duplicates_are_rejected() {
        let b: Bindings = [(ShortcutAction::ToggleWindow, Some("Alt+Ctrl+Digit1".into()))].into();
        let e = Shortcuts::from_bindings(&b).unwrap_err();
        assert_eq!(
            e,
            BindingsError::Duplicate(
                ShortcutAction::SoloMac,
                ShortcutAction::ToggleWindow,
                "Ctrl+Alt+Digit1".into()
            )
        );
        assert!(
            e.to_string()
                .contains("Solo Mac y Mostrar u ocultar Tandem")
        );
    }

    #[test]
    fn invalid_binding_names_the_action() {
        let b: Bindings = [(ShortcutAction::Shared, Some("Shift+KeyA".into()))].into();
        let e = Shortcuts::from_bindings(&b).unwrap_err();
        assert!(e.to_string().starts_with("Compartido: "), "{e}");
    }

    #[test]
    fn json_is_what_the_frontend_reads() {
        let view = ShortcutsView {
            bindings: Shortcuts::default().bindings(),
            failed: [(ShortcutAction::Shared, "ocupado".into())].into(),
            warning: None,
            file: Some("/x/ui.toml".into()),
        };
        assert_eq!(
            serde_json::to_value(&view).unwrap(),
            serde_json::json!({
                "bindings": {
                    "solo_mac": "Ctrl+Alt+Digit1",
                    "shared": "Ctrl+Alt+Digit2",
                    "solo_pc": "Ctrl+Alt+Digit3",
                    "toggle_window": "Ctrl+Alt+Digit0"
                },
                "failed": { "shared": "ocupado" },
                "warning": null,
                "file": "/x/ui.toml"
            })
        );
        let from: Bindings = serde_json::from_value(
            serde_json::json!({ "solo_mac": "Ctrl+Shift+KeyM", "toggle_window": null }),
        )
        .unwrap();
        assert_eq!(from[&ShortcutAction::ToggleWindow], None);
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
        let b: Bindings = [
            (ShortcutAction::Shared, Some("Ctrl+Shift+KeyS".into())),
            (ShortcutAction::ToggleWindow, None),
        ]
        .into();
        let s = Shortcuts::from_bindings(&b).unwrap();
        save(&path, &s).unwrap();
        let l = load(&path);
        assert_eq!(l.warning, None);
        assert_eq!(l.shortcuts, s);
        // Quitado sigue quitado: no vuelve el de siempre.
        assert_eq!(l.shortcuts.get(ShortcutAction::ToggleWindow), None);
        assert!(!path.with_extension("toml.tmp").exists());
    }

    #[test]
    fn a_hand_edited_file_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui.toml");
        std::fs::write(
            &path,
            "otra = 1\n[shortcuts]\nsolo_pc = \"Alt+Ctrl+KeyW\"\n",
        )
        .unwrap();
        let l = load(&path);
        assert_eq!(l.warning, None);
        assert_eq!(
            l.shortcuts
                .get(ShortcutAction::SoloPc)
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
            "[shortcuts]\nsolo_mac = \"KeyA\"\n",
            "[shortcuts]\nsolo_mac = \"Ctrl+KeyA\"\nshared = \"Ctrl+KeyA\"\n",
            "[shortcuts]\nno_existe = \"Ctrl+KeyA\"\n",
        ] {
            std::fs::write(&path, bad).unwrap();
            let l = load(&path);
            assert_eq!(l.shortcuts, Shortcuts::default(), "{bad}");
            let w = l.warning.unwrap();
            assert!(w.contains("ui.toml") && w.contains("de siempre"), "{w}");
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
            .prop_filter("hace falta Ctrl, Alt o Super", |(m, _)| {
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
        /// La forma canónica se vuelve a leer igual, y en cualquier orden de modificadores.
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

        /// Lo que entra por la ventana nunca hace caer al lector: o se acepta o se explica.
        #[test]
        fn parsing_never_panics(s in "\\PC{0,24}") {
            let _ = s.parse::<Accel>();
        }
    }
}
