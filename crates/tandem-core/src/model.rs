use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::data::{DataPrefs, DataPrefsError};
use crate::desk::{InputPrefs, Modules, Shortcuts, valid_color};
use crate::monitor::MonitorCable;
use crate::scene::Scene;

/// Los dos lados del puesto. Tandem está pensado para exactamente dos máquinas, de igual a
/// igual (DECISIONS D1, A1): ningún lado supone qué sistema tiene; eso lo dice su
/// [`MachineInfo`].
///
/// El lado sale de las claves públicas Ed25519 ([`Machine::of_keys`]): la menor es `A`. Las
/// dos máquinas llegan al mismo resultado sin coordinarse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Machine {
    /// Se escribe `a`; al leer también vale `A` (como se muestra).
    #[serde(alias = "A")]
    A,
    #[serde(alias = "B")]
    B,
}

impl Machine {
    pub const ALL: [Machine; 2] = [Machine::A, Machine::B];

    pub fn other(self) -> Self {
        match self {
            Machine::A => Machine::B,
            Machine::B => Machine::A,
        }
    }

    /// Como se escribe en `tandem.toml` y en JSON: `a` o `b`.
    pub fn as_str(self) -> &'static str {
        match self {
            Machine::A => "a",
            Machine::B => "b",
        }
    }

    /// Lo inverso de [`Machine::as_str`].
    pub fn parse(s: &str) -> Option<Machine> {
        match s {
            "a" => Some(Machine::A),
            "b" => Some(Machine::B),
            _ => None,
        }
    }

    /// El lado de quien tiene la clave `own` frente a quien tiene `peer`: la menor, en orden
    /// de bytes, es `A`. Es simétrica (`of_keys(p, o) == of_keys(o, p).other()`), así que
    /// cada máquina calcula la suya y coinciden. `None` si las dos claves son la misma (no
    /// hay dos máquinas).
    pub fn of_keys(own: &[u8; 32], peer: &[u8; 32]) -> Option<Machine> {
        match own.cmp(peer) {
            std::cmp::Ordering::Less => Some(Machine::A),
            std::cmp::Ordering::Greater => Some(Machine::B),
            std::cmp::Ordering::Equal => None,
        }
    }
}

impl fmt::Display for Machine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Machine::A => "A",
            Machine::B => "B",
        })
    }
}

/// El sistema de una máquina. Lo que cambia según el sistema (los backends) se compila con
/// `#[cfg]`; el resto del código pregunta por esto o por las capacidades, nunca por el lado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum Os {
    #[serde(rename = "macos")]
    MacOs,
    #[serde(rename = "windows")]
    Windows,
}

impl Os {
    pub const ALL: [Os; 2] = [Os::MacOs, Os::Windows];

    /// El de este binario. Linux solo corre pruebas y simulaciones (sin backends de
    /// plataforma): se presenta como Windows, y cada prueba elige el que necesite.
    pub fn local() -> Os {
        if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Windows
        }
    }

    /// El nombre de la máquina cuando no hay otro (ni el del sistema ni uno elegido).
    pub fn default_name(self) -> &'static str {
        match self {
            Os::MacOs => "Mac",
            Os::Windows => "PC",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Os::MacOs => "macOS",
            Os::Windows => "Windows",
        }
    }
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// La paleta de los mockups v0.3 (DECISIONS D17): azul, violeta, verde azulado y ámbar.
pub const PALETTE: [&str; 4] = ["#7aa7e6", "#a99be8", "#5fbfb0", "#d9a75a"];

/// Los colores que se asignan solos a cada lado al conocerlo. El usuario puede cambiarlos
/// por cualquiera distinto del de la otra.
pub const DEFAULT_COLORS: [&str; 2] = [PALETTE[0], PALETTE[1]];

/// El color por omisión de `side`.
pub fn default_color(side: Machine) -> &'static str {
    match side {
        Machine::A => DEFAULT_COLORS[0],
        Machine::B => DEFAULT_COLORS[1],
    }
}

/// Lo que identifica a una máquina en la UI: su sistema, su nombre y su color. Viaja en el
/// anuncio de capacidades y se guarda en el puesto compartido ([`Config::machines`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct MachineInfo {
    pub os: Os,
    pub name: String,
    /// Color de acento en CSS (`#rrggbb`).
    pub color: String,
}

impl MachineInfo {
    /// Lo de una máquina recién vista: el nombre que da el sistema (o, sin él, uno derivado
    /// del sistema operativo) y el color por omisión de su lado.
    pub fn new(side: Machine, os: Os, name: Option<String>) -> Self {
        let name = name
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| os.default_name().to_owned());
        MachineInfo {
            os,
            name,
            color: default_color(side).to_owned(),
        }
    }
}

/// Lo que se sabe de cada lado. Vacío hasta que cada máquina se anuncia.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Machines {
    #[serde(default)]
    pub a: Option<MachineInfo>,
    #[serde(default)]
    pub b: Option<MachineInfo>,
}

impl Machines {
    pub fn get(&self, side: Machine) -> Option<&MachineInfo> {
        match side {
            Machine::A => self.a.as_ref(),
            Machine::B => self.b.as_ref(),
        }
    }

    pub fn set(&mut self, side: Machine, info: Option<MachineInfo>) {
        match side {
            Machine::A => self.a = info,
            Machine::B => self.b = info,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.a.is_none() && self.b.is_none()
    }

    /// El nombre de `side` para los textos: el guardado o, sin él, «máquina A».
    pub fn name(&self, side: Machine) -> String {
        self.get(side)
            .map(|i| i.name.clone())
            .unwrap_or_else(|| format!("máquina {side}"))
    }

    /// Los lados cambiados (ver [`Config::swap_sides`]).
    pub fn swapped(self) -> Self {
        Machines {
            a: self.b,
            b: self.a,
        }
    }

    /// Deja a las dos máquinas con colores distintos (D17): si coinciden, B toma el primero
    /// de la paleta que no tenga A. Es la misma regla en las dos máquinas, así que no
    /// discuten. `true` si cambió algo.
    pub fn distinct_colors(&mut self) -> bool {
        let (Some(a), Some(b)) = (&self.a, &mut self.b) else {
            return false;
        };
        if !a.color.eq_ignore_ascii_case(&b.color) {
            return false;
        }
        let free = PALETTE
            .iter()
            .find(|c| !c.eq_ignore_ascii_case(&a.color))
            .unwrap_or(&PALETTE[1]);
        b.color = (*free).to_owned();
        true
    }
}

/// Lado de un escritorio (DECISIONS D18: cualquiera de los cuatro).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, PartialOrd, Ord,
)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// Sin acomodo declarado, A va a la izquierda de B.
    #[default]
    Left,
    Right,
    Up,
    Down,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Left, Side::Right, Side::Up, Side::Down];

    pub fn opposite(self) -> Self {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
            Side::Up => Side::Down,
            Side::Down => Side::Up,
        }
    }

    /// El borde de este lado es vertical (izquierda o derecha): corre a lo largo de `y`.
    pub fn is_vertical_edge(self) -> bool {
        matches!(self, Side::Left | Side::Right)
    }
}

/// Cómo se alinean los dos escritorios a lo largo del borde que comparten: inicio (arriba o
/// izquierda), centro o fin (abajo o derecha). Es simétrica: vale lo mismo vista desde
/// cualquiera de las dos máquinas.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, PartialOrd, Ord,
)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Start,
    #[default]
    Center,
    End,
}

impl Align {
    pub const ALL: [Align; 3] = [Align::Start, Align::Center, Align::End];

    /// Qué parte de la diferencia de largos queda antes del escritorio más corto.
    pub fn factor(self) -> f64 {
        match self {
            Align::Start => 0.0,
            Align::Center => 0.5,
            Align::End => 1.0,
        }
    }
}

/// Unidades nativas del cursor por punto lógico, guardada en milésimas para que el puesto
/// se pueda comparar (y viajar) exacto. En `tandem.toml` se escribe como número: `1.5`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Scale(u32);

impl Scale {
    pub const ONE: Scale = Scale(1000);

    pub fn from_milli(milli: u32) -> Self {
        Scale(milli)
    }

    pub fn milli(self) -> u32 {
        self.0
    }

    pub fn as_f64(self) -> f64 {
        f64::from(self.0) / 1000.0
    }

    /// `None` si no es un número positivo representable.
    pub fn from_f64(v: f64) -> Option<Self> {
        let milli = (v * 1000.0).round();
        let valid = v.is_finite() && milli >= 1.0 && milli <= f64::from(u32::MAX);
        // `as` satura; solo se usa con el valor ya comprobado.
        valid.then_some(Scale(milli as u32))
    }
}

impl Default for Scale {
    fn default() -> Self {
        Scale::ONE
    }
}

impl Serialize for Scale {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(self.as_f64())
    }
}

impl<'de> Deserialize<'de> for Scale {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = f64::deserialize(d)?;
        Scale::from_f64(v).ok_or_else(|| serde::de::Error::custom(format!("escala inválida: {v}")))
    }
}

/// Una pantalla del acomodo declarado, en las coordenadas nativas del cursor de su
/// máquina: puntos en macOS, píxeles físicos del escritorio virtual en Windows (DESIGN §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct ScreenSpec {
    pub machine: Machine,
    /// Para reconocerla en la UI y en los logs; único por máquina.
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub scale: Scale,
}

/// Dónde queda un escritorio respecto del otro (DECISIONS D18): de qué lado y con qué
/// alineación, sin coordenadas a mano. El borde de cruce es el tramo que comparten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Arrangement {
    /// El lado de B en que está A.
    #[serde(default)]
    pub a_side: Side,
    #[serde(default)]
    pub align: Align,
}

impl Arrangement {
    /// El mismo acomodo dicho con los lados cambiados: si A estaba a la izquierda de B, la
    /// que ahora se llama B (la de antes) sigue a la izquierda, es decir, la nueva A queda a
    /// la derecha. La alineación es simétrica y no cambia.
    pub fn swapped(self) -> Self {
        Arrangement {
            a_side: self.a_side.opposite(),
            align: self.align,
        }
    }
}

/// El puesto compartido entre las dos máquinas (DESIGN §4.4): qué máquina es cada lado, el
/// acomodo, las escenas, los atajos, las preferencias de teclado y mouse y los módulos
/// encendidos. Lo edita la UI con parches ([`crate::DeskPatch`]) y lo sincronizan los dos
/// agentes ([`crate::DeskVersion`]). Sin `[[screens]]` de una máquina valen las que ella
/// misma lee; sin acomodo, A a la izquierda de B con el cruce proporcional de siempre.
///
/// Viaja con postcard (`ControlMsg::Desk`): ningún campo puede saltarse al serializar.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, PartialOrd, Ord)]
pub struct Desk {
    /// El nombre del puesto (T-159); `None`: el de omisión de la UI. Viaja con el puesto a
    /// la otra máquina. En `tandem.toml` es `desk_name`.
    #[serde(default, rename = "desk_name")]
    pub name: Option<String>,
    #[serde(default)]
    pub machines: Machines,
    #[serde(default)]
    pub screens: Vec<ScreenSpec>,
    /// `None`: nunca se declaró.
    #[serde(default)]
    pub arrangement: Option<Arrangement>,
    #[serde(default)]
    pub scenes: Vec<Scene>,
    #[serde(default)]
    pub shortcuts: Shortcuts,
    #[serde(default)]
    pub prefs: InputPrefs,
    #[serde(default)]
    pub modules: Modules,
    /// El tope de los datos por Wi-Fi (`[data]`, T-101).
    #[serde(default)]
    pub data: DataPrefs,
    /// A qué entrada de cada monitor compatible va cada máquina y qué pantalla suya es
    /// (`[[cables]]`, T-111). Un monitor sin cable declarado se resuelve con el EDID.
    #[serde(default)]
    pub cables: Vec<MonitorCable>,
}

/// El nombre de antes de [`Desk`].
pub type Config = Desk;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("configuración inválida: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("la pantalla `{id}` de la máquina {machine} está repetida")]
    DuplicateScreen { machine: Machine, id: String },
    #[error("la pantalla `{id}` de la máquina {machine} no tiene área")]
    EmptyScreen { machine: Machine, id: String },
    #[error("una pantalla de la máquina {0} no tiene id")]
    UnnamedScreen(Machine),
    #[error(transparent)]
    Data(#[from] DataPrefsError),
    #[error("el color `{color}` de la máquina {machine} no es #rrggbb")]
    BadColor { machine: Machine, color: String },
    #[error("la máquina {0} no tiene nombre")]
    UnnamedMachine(Machine),
    #[error("una escena no tiene id o nombre")]
    UnnamedScene,
    #[error("la escena `{0}` está repetida")]
    DuplicateScene(String),
    #[error("{0}")]
    BadScene(String),
    #[error("el atajo {} ya es de otra escena o del bloqueo", .0.join("+"))]
    DuplicateShortcut(Vec<String>),
    #[error("un módulo de la máquina {0} no tiene id")]
    UnnamedModule(Machine),
    #[error("{0}")]
    BadCable(String),
}

impl Desk {
    pub fn from_toml(s: &str) -> Result<Self, ConfigError> {
        let cfg: Desk = toml::from_str(s)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        self.data.validate()?;
        let mut seen = BTreeSet::new();
        for s in &self.screens {
            if s.id.trim().is_empty() {
                return Err(ConfigError::UnnamedScreen(s.machine));
            }
            if s.width == 0 || s.height == 0 {
                return Err(ConfigError::EmptyScreen {
                    machine: s.machine,
                    id: s.id.clone(),
                });
            }
            if !seen.insert((s.machine, s.id.as_str())) {
                return Err(ConfigError::DuplicateScreen {
                    machine: s.machine,
                    id: s.id.clone(),
                });
            }
        }
        for side in Machine::ALL {
            if let Some(info) = self.machines.get(side) {
                if info.name.trim().is_empty() {
                    return Err(ConfigError::UnnamedMachine(side));
                }
                if !valid_color(&info.color) {
                    return Err(ConfigError::BadColor {
                        machine: side,
                        color: info.color.clone(),
                    });
                }
            }
            if self.modules.of(side).keys().any(|m| m.trim().is_empty()) {
                return Err(ConfigError::UnnamedModule(side));
            }
        }
        let mut scenes = BTreeSet::new();
        // El mismo atajo para dos cosas haría las dos a la vez: no se acepta. El orden de
        // las teclas no importa.
        let mut shortcuts = BTreeSet::new();
        let mut keys = |k: &[String]| {
            let set: BTreeSet<String> = k.iter().cloned().collect();
            set.is_empty() || shortcuts.insert(set)
        };
        keys(&self.shortcuts.lock);
        for s in &self.scenes {
            if s.id.trim().is_empty() || s.name.trim().is_empty() {
                return Err(ConfigError::UnnamedScene);
            }
            if !scenes.insert(s.id.as_str()) {
                return Err(ConfigError::DuplicateScene(s.id.clone()));
            }
            if let Some(problem) = s.problem() {
                return Err(ConfigError::BadScene(problem));
            }
            if !keys(&s.shortcut) {
                return Err(ConfigError::DuplicateShortcut(s.shortcut.clone()));
            }
        }
        let mut cabled = BTreeSet::new();
        for c in &self.cables {
            if let Some(problem) = c.problem() {
                return Err(ConfigError::BadCable(problem));
            }
            if !cabled.insert(c.monitor.as_str()) {
                return Err(ConfigError::BadCable(format!(
                    "el monitor `{}` tiene dos cableados",
                    c.monitor
                )));
            }
        }
        Ok(())
    }

    /// El cableado declarado de `monitor`.
    pub fn cable(&self, monitor: &str) -> Option<&MonitorCable> {
        self.cables.iter().find(|c| c.monitor == monitor)
    }

    /// Las pantallas declaradas de `machine`, en el orden del archivo.
    pub fn screens_of(&self, machine: Machine) -> impl Iterator<Item = &ScreenSpec> {
        self.screens.iter().filter(move |s| s.machine == machine)
    }

    /// El mismo puesto con los lados cambiados (A↔B): lo que se hace cuando una máquina
    /// descubre, al emparejarse, que es el otro lado. Nada físico cambia: cada pantalla
    /// sigue siendo de su máquina y el acomodo sigue siendo el mismo.
    pub fn swap_sides(self) -> Self {
        Desk {
            machines: self.machines.swapped(),
            screens: self
                .screens
                .into_iter()
                .map(|s| ScreenSpec {
                    machine: s.machine.other(),
                    ..s
                })
                .collect(),
            arrangement: self.arrangement.map(Arrangement::swapped),
            scenes: self.scenes.into_iter().map(Scene::swapped).collect(),
            modules: self.modules.swapped(),
            cables: self.cables.into_iter().map(MonitorCable::swapped).collect(),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wifi_data_cap_belongs_to_the_desk_and_has_its_patch() {
        let desk = Desk::from_toml("[data]\nwifi_cap_percent = 30\nwifi_cap_mbps = 80").unwrap();
        assert_eq!(desk.data.wifi_cap_percent, 30);
        assert_eq!(desk.data.fixed_cap_bytes(), Some(10_000_000));
        assert_eq!(desk.clone().swap_sides().data, desk.data);
        assert!(matches!(
            Desk::from_toml("[data]\nwifi_cap_percent = 0"),
            Err(ConfigError::Data(_))
        ));

        let mut edited = Desk::default();
        edited.apply(crate::DeskPatch::Data(desk.data)).unwrap();
        assert_eq!(edited.data, desk.data);
        assert_eq!(
            Desk::default().summary_of_changes(&edited).as_deref(),
            Some("Datos por Wi-Fi")
        );
        let bad = DataPrefs {
            wifi_cap_percent: 0,
            wifi_cap_mbps: None,
        };
        assert!(edited.apply(crate::DeskPatch::Data(bad)).is_err());
    }

    #[test]
    fn an_empty_desk_is_valid_and_puts_a_on_the_left() {
        let cfg = Config::from_toml("").unwrap();
        assert!(cfg.screens.is_empty());
        assert!(cfg.machines.is_empty());
        assert_eq!(cfg.arrangement, None);
        assert_eq!(cfg.shortcuts.lock, ["ScrollLock"]);
        assert!(cfg.prefs.cmd_ctrl && cfg.prefs.drag_files);
        assert!(cfg.scenes.is_empty());
    }

    #[test]
    fn parses_declared_screens_arrangement_and_machines() {
        let cfg = Config::from_toml(
            r##"
            [machines.a]
            os = "windows"
            name = "Torre"
            color = "#7aa7e6"

            [[screens]]
            machine = "b"
            id = "34"
            x = 0
            y = 0
            width = 3440
            height = 1440

            [[screens]]
            machine = "b"
            id = "32"
            x = 3440
            y = 0
            width = 3840
            height = 2160
            scale = 1.5

            [[screens]]
            machine = "a"
            id = "laptop"
            x = 0
            y = 0
            width = 1512
            height = 982
            scale = 2

            [arrangement]
            a_side = "up"
            align = "end"

            [[scenes]]
            id = "trabajo"
            name = "Trabajo"

            [shortcuts]
            lock = ["ControlLeft", "KeyL"]

            [prefs]
            drag_files = false

            [modules.b]
            clipboard = false
            "##,
        )
        .unwrap();
        let b: Vec<_> = cfg.screens_of(Machine::B).map(|s| s.id.as_str()).collect();
        assert_eq!(b, ["34", "32"]);
        assert_eq!(cfg.screens[0].scale, Scale::ONE);
        assert_eq!(cfg.screens[1].scale.milli(), 1500);
        assert_eq!(cfg.screens[2].scale.as_f64(), 2.0);
        assert_eq!(
            cfg.arrangement,
            Some(Arrangement {
                a_side: Side::Up,
                align: Align::End,
            })
        );
        assert_eq!(cfg.scenes[0].name, "Trabajo");
        assert_eq!(cfg.shortcuts.lock, ["ControlLeft", "KeyL"]);
        assert!(!cfg.prefs.drag_files && cfg.prefs.cmd_ctrl);
        assert_eq!(cfg.modules.get(Machine::B, "clipboard"), Some(false));
        assert_eq!(cfg.modules.get(Machine::A, "clipboard"), None);
        let a = cfg.machines.get(Machine::A).unwrap();
        assert_eq!((a.os, a.name.as_str()), (Os::Windows, "Torre"));
        assert_eq!(cfg.machines.get(Machine::B), None);
        assert_eq!(cfg.machines.name(Machine::B), "máquina B");
    }

    #[test]
    fn rejects_bad_screens() {
        let screen = |machine: &str, id: &str, w: u32, scale: &str| {
            format!(
                "[[screens]]\nmachine = \"{machine}\"\nid = \"{id}\"\nx = 0\ny = 0\nwidth = {w}\nheight = 10\nscale = {scale}\n"
            )
        };
        let twice = format!("{}{}", screen("b", "a", 10, "1"), screen("b", "a", 10, "1"));
        assert!(matches!(
            Config::from_toml(&twice),
            Err(ConfigError::DuplicateScreen { .. })
        ));
        // El mismo id en cada máquina vale.
        let both = format!("{}{}", screen("b", "a", 10, "1"), screen("a", "a", 10, "1"));
        assert!(Config::from_toml(&both).is_ok());
        assert!(matches!(
            Config::from_toml(&screen("a", "a", 0, "1")),
            Err(ConfigError::EmptyScreen { .. })
        ));
        assert!(matches!(
            Config::from_toml(&screen("a", " ", 10, "1")),
            Err(ConfigError::UnnamedScreen(Machine::A))
        ));
        for bad in ["0", "-1", "nan"] {
            assert!(
                matches!(
                    Config::from_toml(&screen("a", "a", 10, bad)),
                    Err(ConfigError::Parse(_))
                ),
                "{bad}"
            );
        }
        assert!(matches!(
            Config::from_toml("[arrangement]\nside = \"left\""),
            Err(ConfigError::Parse(_))
        ));
        // El desplazamiento a mano de antes ya no existe (D18): lo quita la migración.
        assert!(matches!(
            Config::from_toml("[arrangement]\na_side = \"left\"\noffset = 0"),
            Err(ConfigError::Parse(_))
        ));
        let color = "[machines.a]\nos = \"macos\"\nname = \"Mac\"\ncolor = \"azul\"";
        assert!(matches!(
            Config::from_toml(color),
            Err(ConfigError::BadColor { .. })
        ));
        let unnamed = "[machines.a]\nos = \"macos\"\nname = \" \"\ncolor = \"#7aa7e6\"";
        assert!(matches!(
            Config::from_toml(unnamed),
            Err(ConfigError::UnnamedMachine(Machine::A))
        ));
        let scenes = "[[scenes]]\nid = \"x\"\nname = \"X\"\n[[scenes]]\nid = \"x\"\nname = \"Y\"";
        assert!(matches!(
            Config::from_toml(scenes),
            Err(ConfigError::DuplicateScene(_))
        ));
        assert!(matches!(
            Config::from_toml("[[scenes]]\nid = \"x\"\nname = \"\""),
            Err(ConfigError::UnnamedScene)
        ));
        assert!(matches!(
            Config::from_toml("[modules.a]\n\"\" = true"),
            Err(ConfigError::UnnamedModule(Machine::A))
        ));
        // Los nombres de antes ya no son lados: los migra el agente, no el núcleo.
        assert!(matches!(
            Config::from_toml(&screen("mac", "a", 10, "1")),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn the_desk_round_trips_through_toml() {
        let cfg = Config {
            machines: Machines {
                a: Some(MachineInfo::new(Machine::A, Os::MacOs, None)),
                b: None,
            },
            screens: vec![ScreenSpec {
                machine: Machine::A,
                id: "laptop".into(),
                x: -1512,
                y: 10,
                width: 1512,
                height: 982,
                scale: Scale::from_milli(1250),
            }],
            arrangement: Some(Arrangement {
                a_side: Side::Down,
                align: Align::Start,
            }),
            scenes: vec![Scene::new("solo", "Solo A")],
            ..Config::default()
        };
        let text = toml::to_string(&cfg).unwrap();
        assert_eq!(Config::from_toml(&text).unwrap(), cfg);
    }

    #[test]
    fn scale_conversions() {
        assert_eq!(Scale::from_f64(1.25), Some(Scale::from_milli(1250)));
        assert_eq!(Scale::from_f64(0.0), None);
        assert_eq!(Scale::from_f64(f64::INFINITY), None);
        assert_eq!(Scale::default(), Scale::ONE);
        for side in Side::ALL {
            assert_ne!(side.opposite(), side);
            assert_eq!(side.opposite().opposite(), side);
            assert_eq!(side.opposite().is_vertical_edge(), side.is_vertical_edge());
        }
        assert_eq!(Align::Start.factor(), 0.0);
        assert_eq!(Align::default(), Align::Center);
    }

    #[test]
    fn sides_come_from_the_keys() {
        let (low, high) = ([1u8; 32], [2u8; 32]);
        assert_eq!(Machine::of_keys(&low, &high), Some(Machine::A));
        assert_eq!(Machine::of_keys(&high, &low), Some(Machine::B));
        assert_eq!(Machine::of_keys(&low, &low), None);
        // Orden de bytes, no de números: el primer byte manda.
        let mut a = [0xffu8; 32];
        a[0] = 0;
        let mut b = [0u8; 32];
        b[0] = 1;
        assert_eq!(Machine::of_keys(&a, &b), Some(Machine::A));
    }

    #[test]
    fn names_and_strings_of_each_side() {
        for m in Machine::ALL {
            assert_eq!(Machine::parse(m.as_str()), Some(m));
            assert_eq!(m.other().other(), m);
        }
        assert_eq!(Machine::parse("mac"), None);
        assert_eq!(Machine::A.to_string(), "A");
        assert_eq!(
            serde_json::to_string(&Machine::B).unwrap(),
            "\"b\"",
            "en archivos y JSON van en minúscula"
        );
    }

    #[test]
    fn machine_info_defaults_come_from_the_system() {
        let mac = MachineInfo::new(Machine::A, Os::MacOs, None);
        assert_eq!(mac.name, "Mac");
        assert_eq!(mac.color, DEFAULT_COLORS[0]);
        let pc = MachineInfo::new(Machine::B, Os::Windows, Some("  ".into()));
        assert_eq!(pc.name, "PC");
        assert_eq!(pc.color, DEFAULT_COLORS[1]);
        let named = MachineInfo::new(Machine::B, Os::MacOs, Some(" Air ".into()));
        assert_eq!(named.name, "Air");
        assert_eq!(serde_json::to_string(&Os::MacOs).unwrap(), "\"macos\"");
        assert_eq!(Os::Windows.to_string(), "Windows");
    }

    #[test]
    fn swapping_sides_keeps_the_physical_desk() {
        let cfg = Config {
            machines: Machines {
                a: Some(MachineInfo::new(Machine::A, Os::MacOs, None)),
                b: Some(MachineInfo::new(Machine::B, Os::Windows, None)),
            },
            screens: vec![ScreenSpec {
                machine: Machine::A,
                id: "laptop".into(),
                x: 0,
                y: 0,
                width: 10,
                height: 10,
                scale: Scale::ONE,
            }],
            arrangement: Some(Arrangement {
                a_side: Side::Left,
                align: Align::End,
            }),
            ..Config::default()
        };
        let mut cfg = cfg;
        cfg.modules.set(Machine::A, "clipboard", false);
        let swapped = cfg.clone().swap_sides();
        assert_eq!(swapped.machines.b.as_ref().unwrap().os, Os::MacOs);
        assert_eq!(swapped.screens[0].machine, Machine::B);
        // La Mac sigue a la izquierda: ahora es B, así que A (la PC) queda a la derecha.
        let arr = swapped.arrangement.unwrap();
        assert_eq!(arr.a_side, Side::Right);
        assert_eq!(arr.align, Align::End);
        assert_eq!(swapped.modules.get(Machine::B, "clipboard"), Some(false));
        assert_eq!(swapped.swap_sides(), cfg);
    }
}
