use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// Los colores que se asignan solos a cada lado (paleta de los mockups v0.3, DECISIONS
/// D17). El usuario puede cambiarlos.
pub const DEFAULT_COLORS: [&str; 2] = ["#7aa7e6", "#a99be8"];

/// El color por omisión de `side`.
pub fn default_color(side: Machine) -> &'static str {
    match side {
        Machine::A => DEFAULT_COLORS[0],
        Machine::B => DEFAULT_COLORS[1],
    }
}

/// Lo que identifica a una máquina en la UI: su sistema, su nombre y su color. Viaja en el
/// anuncio de capacidades y se guarda en el puesto compartido ([`Config::machines`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
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
}

/// Lado de un escritorio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// Sin acomodo declarado, A va a la izquierda de B.
    #[default]
    Left,
    Right,
}

impl Side {
    pub fn opposite(self) -> Self {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Dónde queda un escritorio respecto del otro.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Arrangement {
    /// El lado de B en que está A.
    #[serde(default)]
    pub a_side: Side,
    /// Desplazamiento vertical de A respecto de B, en puntos lógicos (positivo: más abajo).
    /// Lo dibuja el editor del acomodo; el cruce sigue siendo proporcional a lo largo del
    /// borde compartido, así que hoy no lo cambia.
    #[serde(default)]
    pub offset: i32,
}

impl Arrangement {
    /// El mismo acomodo dicho con los lados cambiados: si A estaba a la izquierda de B, la
    /// que ahora se llama B (la de antes) sigue a la izquierda, es decir, la nueva A queda a
    /// la derecha.
    pub fn swapped(self) -> Self {
        Arrangement {
            a_side: self.a_side.opposite(),
            offset: self.offset.saturating_neg(),
        }
    }
}

/// El puesto compartido entre las dos máquinas (DESIGN §4): qué máquina es cada lado y el
/// acomodo de pantallas para el motor de entrada. Sin `[[screens]]` de una máquina valen
/// las que ella misma lee.
///
/// Viaja con postcard (`ControlMsg::Desk`): ningún campo puede saltarse al serializar.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub machines: Machines,
    #[serde(default)]
    pub screens: Vec<ScreenSpec>,
    #[serde(default)]
    pub arrangement: Arrangement,
}

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
}

impl Config {
    pub fn from_toml(s: &str) -> Result<Self, ConfigError> {
        let cfg: Config = toml::from_str(s)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
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
        Ok(())
    }

    /// Las pantallas declaradas de `machine`, en el orden del archivo.
    pub fn screens_of(&self, machine: Machine) -> impl Iterator<Item = &ScreenSpec> {
        self.screens.iter().filter(move |s| s.machine == machine)
    }

    /// El mismo puesto con los lados cambiados (A↔B): lo que se hace cuando una máquina
    /// descubre, al emparejarse, que es el otro lado. Nada físico cambia: cada pantalla
    /// sigue siendo de su máquina y el acomodo sigue siendo el mismo.
    pub fn swap_sides(self) -> Self {
        Config {
            machines: self.machines.swapped(),
            screens: self
                .screens
                .into_iter()
                .map(|s| ScreenSpec {
                    machine: s.machine.other(),
                    ..s
                })
                .collect(),
            arrangement: self.arrangement.swapped(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_desk_is_valid_and_puts_a_on_the_left() {
        let cfg = Config::from_toml("").unwrap();
        assert!(cfg.screens.is_empty());
        assert!(cfg.machines.is_empty());
        assert_eq!(cfg.arrangement.a_side, Side::Left);
        assert_eq!(cfg.arrangement.offset, 0);
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
            a_side = "right"
            offset = -120
            "##,
        )
        .unwrap();
        let b: Vec<_> = cfg.screens_of(Machine::B).map(|s| s.id.as_str()).collect();
        assert_eq!(b, ["34", "32"]);
        assert_eq!(cfg.screens[0].scale, Scale::ONE);
        assert_eq!(cfg.screens[1].scale.milli(), 1500);
        assert_eq!(cfg.screens[2].scale.as_f64(), 2.0);
        assert_eq!(cfg.arrangement.a_side, Side::Right);
        assert_eq!(cfg.arrangement.offset, -120);
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
            arrangement: Arrangement {
                a_side: Side::Right,
                offset: 40,
            },
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
        assert_eq!(Side::Left.opposite(), Side::Right);
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
            arrangement: Arrangement {
                a_side: Side::Left,
                offset: 7,
            },
        };
        let swapped = cfg.clone().swap_sides();
        assert_eq!(swapped.machines.b.as_ref().unwrap().os, Os::MacOs);
        assert_eq!(swapped.screens[0].machine, Machine::B);
        // La Mac sigue a la izquierda: ahora es B, así que A (la PC) queda a la derecha.
        assert_eq!(swapped.arrangement.a_side, Side::Right);
        assert_eq!(swapped.arrangement.offset, -7);
        assert_eq!(swapped.swap_sides(), cfg);
    }
}
