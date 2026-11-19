//! Desk preferences for data (tunnels, files) between the two machines
//! (DECISIONS A2, DESIGN §6).
//!
//! Data goes over any path, but over Wi-Fi it carries a cap so as not to add
//! latency to the keyboard and mouse. The cap comes from here; whoever applies it is `tandem-link`.

use serde::{Deserialize, Serialize};

/// Default percentage of the last measured Wi-Fi bandwidth.
pub const DEFAULT_WIFI_CAP_PERCENT: u8 = 50;

/// `[data]` of `tandem.toml`, shared between the two machines.
///
/// Travels with postcard inside the desk: no field is skipped when serializing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataPrefs {
    /// Cap on data over Wi-Fi, in % of the last measured bandwidth (1–100).
    #[serde(default = "default_percent")]
    pub wifi_cap_percent: u8,
    /// Fixed cap over Wi-Fi in Mbit/s (the unit Wi-Fi is talked about in). If present,
    /// it overrides the percentage.
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
    #[error("[data] wifi_cap_percent = {0}: must be between 1 and 100")]
    Percent(u8),
    #[error("[data] wifi_cap_mbps = 0: to not limit, remove the key and raise the percentage")]
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

    /// The fixed cap in bytes per second, if there is one.
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
