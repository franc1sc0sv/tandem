//! Lo mínimo de los monitores que el núcleo necesita para planear (DESIGN §10): qué
//! entradas existen. El control real vive en `tandem-monitor`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Una entrada de video de un monitor compatible (el estándar del firmware propio).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorInput {
    #[serde(alias = "dp")]
    DisplayPort,
    Hdmi,
}

impl MonitorInput {
    pub const ALL: [MonitorInput; 2] = [MonitorInput::DisplayPort, MonitorInput::Hdmi];

    pub fn as_str(self) -> &'static str {
        match self {
            MonitorInput::DisplayPort => "DP",
            MonitorInput::Hdmi => "HDMI",
        }
    }
}

impl fmt::Display for MonitorInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_read_and_show_as_the_user_knows_them() {
        assert_eq!(MonitorInput::Hdmi.to_string(), "HDMI");
        let dp: MonitorInput = serde_json::from_str("\"dp\"").unwrap();
        assert_eq!(dp, MonitorInput::DisplayPort);
        assert_eq!(
            serde_json::to_string(&MonitorInput::DisplayPort).unwrap(),
            "\"display_port\""
        );
    }
}
