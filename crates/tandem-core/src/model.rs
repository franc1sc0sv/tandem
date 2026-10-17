use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

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

/// Entrada física de un monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Input {
    Hdmi,
    DisplayPort,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MonitorId(pub String);

impl fmt::Display for MonitorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for MonitorId {
    fn from(s: &str) -> Self {
        MonitorId(s.to_owned())
    }
}

/// Códigos DDC/CI (VCP 0x60) que el monitor obedece de verdad.
///
/// Los Samsung declaran unos valores en su capabilities string y obedecen otros,
/// así que se configuran a mano a partir de pruebas (ver `docs/research/`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DdcProfile {
    pub hdmi: u8,
    pub display_port: u8,
}

impl DdcProfile {
    pub fn code_for(&self, input: Input) -> u8 {
        match input {
            Input::Hdmi => self.hdmi,
            Input::DisplayPort => self.display_port,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorConfig {
    pub id: MonitorId,
    /// Nombre para la UI, por ejemplo `34"`.
    pub label: String,
    /// Modelo tal como lo reporta el EDID (por ejemplo `LC34G55T`); sirve para encontrar el monitor.
    pub edid_model: Option<String>,
    /// Otros id PnP del mismo monitor físico (T-087). Los Samsung exponen un EDID distinto
    /// por entrada: el 34" es `SAM711A` para la PC (DisplayPort) y `SAM7119` para la Mac
    /// (HDMI). `edid_model` es el principal; aquí van los demás.
    #[serde(default)]
    pub edid_aliases: Vec<String>,
    /// Qué entrada del monitor va cableada a cada máquina.
    pub wiring: BTreeMap<Machine, Input>,
    pub ddc: Option<DdcProfile>,
}

impl MonitorConfig {
    pub fn input_for(&self, machine: Machine) -> Option<Input> {
        self.wiring.get(&machine).copied()
    }

    /// Todas las formas de reconocer este monitor: `edid_model` y sus alias.
    pub fn models(&self) -> impl Iterator<Item = &str> {
        self.edid_model
            .iter()
            .chain(&self.edid_aliases)
            .map(String::as_str)
            .filter(|m| !m.trim().is_empty())
    }

    /// Si `model` (un id PnP o un modelo de EDID) es de este monitor, sin distinguir
    /// mayúsculas.
    pub fn is_model(&self, model: &str) -> bool {
        self.models()
            .any(|m| m.trim().eq_ignore_ascii_case(model.trim()))
    }
}

/// Configuración estática del puesto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Monitores ordenados de izquierda a derecha sobre el escritorio.
    pub monitors: Vec<MonitorConfig>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("configuración inválida: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("el monitor `{0}` está repetido")]
    DuplicateMonitor(MonitorId),
    #[error("el monitor `{0}` no está cableado a ninguna máquina")]
    Unwired(MonitorId),
}

impl Config {
    pub fn from_toml(s: &str) -> Result<Self, ConfigError> {
        let cfg: Config = toml::from_str(s)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut seen = std::collections::BTreeSet::new();
        for m in &self.monitors {
            if !seen.insert(&m.id) {
                return Err(ConfigError::DuplicateMonitor(m.id.clone()));
            }
            if m.wiring.is_empty() {
                return Err(ConfigError::Unwired(m.id.clone()));
            }
        }
        Ok(())
    }

    pub fn monitor(&self, id: &MonitorId) -> Option<&MonitorConfig> {
        self.monitors.iter().find(|m| &m.id == id)
    }

    /// Quita `id` del puesto; `true` si estaba.
    pub fn remove_monitor(&mut self, id: &MonitorId) -> bool {
        let before = self.monitors.len();
        self.monitors.retain(|m| &m.id != id);
        self.monitors.len() != before
    }

    /// El puesto sin los monitores que no están en `keep`.
    pub fn only(&self, keep: impl Fn(&MonitorConfig) -> bool) -> Config {
        Config {
            monitors: self.monitors.iter().filter(|m| keep(m)).cloned().collect(),
        }
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    /// El puesto real: 34" a la izquierda, 32" a la derecha; DP a la PC, HDMI a la Mac.
    pub fn desk() -> Config {
        Config::from_toml(
            r#"
            [[monitors]]
            id = "m34"
            label = '34"'
            edid_model = "LC34G55T"
            wiring = { pc = "displayport", mac = "hdmi" }
            ddc = { hdmi = 0x06, display_port = 0x0F }

            [[monitors]]
            id = "m32"
            label = '32"'
            wiring = { pc = "displayport", mac = "hdmi" }
            "#,
        )
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_desk_config() {
        let cfg = fixtures::desk();
        let m34 = cfg.monitor(&"m34".into()).unwrap();
        assert_eq!(m34.input_for(Machine::Pc), Some(Input::DisplayPort));
        assert_eq!(m34.ddc.as_ref().unwrap().code_for(Input::Hdmi), 0x06);
        assert!(cfg.monitor(&"m32".into()).unwrap().ddc.is_none());
    }

    #[test]
    fn rejects_duplicate_monitor() {
        let err = Config::from_toml(
            r#"
            [[monitors]]
            id = "a"
            label = "a"
            wiring = { pc = "hdmi" }
            [[monitors]]
            id = "a"
            label = "b"
            wiring = { pc = "hdmi" }
            "#,
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::DuplicateMonitor(_)));
    }
}
