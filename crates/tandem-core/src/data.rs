//! Preferencias del puesto para los datos (túneles, archivos) entre las dos máquinas
//! (DECISIONS A2, DESIGN §6).
//!
//! Los datos van por cualquier camino, pero en Wi-Fi llevan un tope para no meterle
//! latencia al teclado y al mouse. El tope sale de aquí; quien lo aplica es `tandem-link`.

use serde::{Deserialize, Serialize};

/// Porcentaje por omisión del último ancho de banda medido del Wi-Fi.
pub const DEFAULT_WIFI_CAP_PERCENT: u8 = 50;

/// `[data]` de `tandem.toml`, compartido entre las dos máquinas.
///
/// Viaja con postcard dentro del puesto: ningún campo se salta al serializar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataPrefs {
    /// Tope de los datos por Wi-Fi, en % del último ancho de banda medido (1–100).
    #[serde(default = "default_percent")]
    pub wifi_cap_percent: u8,
    /// Tope fijo por Wi-Fi en Mbit/s (la unidad con que se habla del Wi-Fi). Si está,
    /// manda sobre el porcentaje.
    #[serde(default)]
    pub wifi_cap_mbps: Option<u32>,
}

fn default_percent() -> u8 {
    DEFAULT_WIFI_CAP_PERCENT
}

impl Default for DataPrefs {
    fn default() -> Self {
        DataPrefs {
            wifi_cap_percent: DEFAULT_WIFI_CAP_PERCENT,
            wifi_cap_mbps: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DataPrefsError {
    #[error("[data] wifi_cap_percent = {0}: tiene que estar entre 1 y 100")]
    Percent(u8),
    #[error("[data] wifi_cap_mbps = 0: para no limitar, quita la clave y sube el porcentaje")]
    ZeroMbps,
}

impl DataPrefs {
    pub fn validate(&self) -> Result<(), DataPrefsError> {
        if !(1..=100).contains(&self.wifi_cap_percent) {
            return Err(DataPrefsError::Percent(self.wifi_cap_percent));
        }
        if self.wifi_cap_mbps == Some(0) {
            return Err(DataPrefsError::ZeroMbps);
        }
        Ok(())
    }

    /// El tope fijo en bytes por segundo, si hay uno.
    pub fn fixed_cap_bytes(&self) -> Option<u64> {
        self.wifi_cap_mbps.map(|m| u64::from(m) * 125_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_default_half_of_the_measured_bandwidth() {
        let p: DataPrefs = toml::from_str("").unwrap();
        assert_eq!(p, DataPrefs::default());
        assert_eq!(p.wifi_cap_percent, 50);
        assert_eq!(p.fixed_cap_bytes(), None);
        assert!(p.validate().is_ok());
    }

    #[test]
    fn a_fixed_cap_is_read_in_megabits() {
        let p: DataPrefs = toml::from_str("wifi_cap_mbps = 80").unwrap();
        assert_eq!(p.fixed_cap_bytes(), Some(10_000_000));
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        let p: DataPrefs = toml::from_str("wifi_cap_percent = 0").unwrap();
        assert_eq!(p.validate(), Err(DataPrefsError::Percent(0)));
        let p: DataPrefs = toml::from_str("wifi_cap_percent = 101").unwrap();
        assert_eq!(p.validate(), Err(DataPrefsError::Percent(101)));
        let p: DataPrefs = toml::from_str("wifi_cap_mbps = 0").unwrap();
        assert_eq!(p.validate(), Err(DataPrefsError::ZeroMbps));
        assert!(toml::from_str::<DataPrefs>("cap = 1").is_err());
    }
}
