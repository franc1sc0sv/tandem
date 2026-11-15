use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Las dos máquinas del puesto. Tandem está pensado para exactamente dos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Machine {
    Mac,
    Pc,
}

impl Machine {
    pub fn other(self) -> Self {
        match self {
            Machine::Mac => Machine::Pc,
            Machine::Pc => Machine::Mac,
        }
    }
}

impl fmt::Display for Machine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Machine::Mac => "mac",
            Machine::Pc => "pc",
        })
    }
}

/// Lado de un escritorio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// Sin acomodo declarado, la Mac va a la izquierda de la PC (DESIGN §7).
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
    /// El lado de la PC en que está la Mac.
    #[serde(default)]
    pub mac_side: Side,
    /// Desplazamiento vertical de la Mac respecto de la PC, en puntos lógicos (positivo:
    /// más abajo). Lo dibuja el editor del acomodo (T-096); el cruce sigue siendo
    /// proporcional a lo largo del borde compartido, así que hoy no lo cambia.
    #[serde(default)]
    pub offset: i32,
}

/// El puesto compartido entre las dos máquinas (DESIGN §4): el acomodo de pantallas para el
/// motor de entrada. Sin `[[screens]]` de una máquina valen las que ella misma lee.
///
/// Viaja con postcard (`ControlMsg::Desk`): ningún campo puede saltarse al serializar.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub screens: Vec<ScreenSpec>,
    #[serde(default)]
    pub arrangement: Arrangement,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("configuración inválida: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("la pantalla `{id}` de la {machine} está repetida")]
    DuplicateScreen { machine: Machine, id: String },
    #[error("la pantalla `{id}` de la {machine} no tiene área")]
    EmptyScreen { machine: Machine, id: String },
    #[error("una pantalla de la {0} no tiene id")]
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_desk_is_valid_and_puts_the_mac_on_the_left() {
        let cfg = Config::from_toml("").unwrap();
        assert!(cfg.screens.is_empty());
        assert_eq!(cfg.arrangement.mac_side, Side::Left);
        assert_eq!(cfg.arrangement.offset, 0);
    }

    #[test]
    fn parses_declared_screens_and_arrangement() {
        let cfg = Config::from_toml(
            r#"
            [[screens]]
            machine = "pc"
            id = "34"
            x = 0
            y = 0
            width = 3440
            height = 1440

            [[screens]]
            machine = "pc"
            id = "32"
            x = 3440
            y = 0
            width = 3840
            height = 2160
            scale = 1.5

            [[screens]]
            machine = "mac"
            id = "laptop"
            x = 0
            y = 0
            width = 1512
            height = 982
            scale = 2

            [arrangement]
            mac_side = "right"
            offset = -120
            "#,
        )
        .unwrap();
        let pc: Vec<_> = cfg.screens_of(Machine::Pc).map(|s| s.id.as_str()).collect();
        assert_eq!(pc, ["34", "32"]);
        assert_eq!(cfg.screens[0].scale, Scale::ONE);
        assert_eq!(cfg.screens[1].scale.milli(), 1500);
        assert_eq!(cfg.screens[2].scale.as_f64(), 2.0);
        assert_eq!(cfg.arrangement.mac_side, Side::Right);
        assert_eq!(cfg.arrangement.offset, -120);
    }

    #[test]
    fn rejects_bad_screens() {
        let screen = |machine: &str, id: &str, w: u32, scale: &str| {
            format!(
                "[[screens]]\nmachine = \"{machine}\"\nid = \"{id}\"\nx = 0\ny = 0\nwidth = {w}\nheight = 10\nscale = {scale}\n"
            )
        };
        let twice = format!(
            "{}{}",
            screen("pc", "a", 10, "1"),
            screen("pc", "a", 10, "1")
        );
        assert!(matches!(
            Config::from_toml(&twice),
            Err(ConfigError::DuplicateScreen { .. })
        ));
        // El mismo id en cada máquina vale.
        let both = format!(
            "{}{}",
            screen("pc", "a", 10, "1"),
            screen("mac", "a", 10, "1")
        );
        assert!(Config::from_toml(&both).is_ok());
        assert!(matches!(
            Config::from_toml(&screen("mac", "a", 0, "1")),
            Err(ConfigError::EmptyScreen { .. })
        ));
        assert!(matches!(
            Config::from_toml(&screen("mac", " ", 10, "1")),
            Err(ConfigError::UnnamedScreen(Machine::Mac))
        ));
        for bad in ["0", "-1", "nan"] {
            assert!(
                matches!(
                    Config::from_toml(&screen("mac", "a", 10, bad)),
                    Err(ConfigError::Parse(_))
                ),
                "{bad}"
            );
        }
        assert!(matches!(
            Config::from_toml("[arrangement]\nside = \"left\""),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn the_desk_round_trips_through_toml() {
        let cfg = Config {
            screens: vec![ScreenSpec {
                machine: Machine::Mac,
                id: "laptop".into(),
                x: -1512,
                y: 10,
                width: 1512,
                height: 982,
                scale: Scale::from_milli(1250),
            }],
            arrangement: Arrangement {
                mac_side: Side::Right,
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
}
