use std::fmt;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

/// Tipo de enlace entre las dos máquinas (DESIGN §6).
///
/// El orden de las variantes es la prioridad: `Usb4 < Ethernet < Wifi`, así
/// que el mejor enlace sano es el menor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkKind {
    /// USB4 / Thunderbolt directo.
    Usb4,
    /// Ethernet 2.5G directo.
    Ethernet,
    /// Wi-Fi de la LAN de casa.
    Wifi,
}

impl LinkKind {
    pub const ALL: [LinkKind; 3] = [LinkKind::Usb4, LinkKind::Ethernet, LinkKind::Wifi];

    /// Prioridad de la tabla de DESIGN §6 (1 = la mejor).
    pub fn priority(self) -> u8 {
        match self {
            LinkKind::Usb4 => 1,
            LinkKind::Ethernet => 2,
            LinkKind::Wifi => 3,
        }
    }

    pub fn is_wired(self) -> bool {
        !matches!(self, LinkKind::Wifi)
    }
}

impl fmt::Display for LinkKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LinkKind::Usb4 => "USB4",
            LinkKind::Ethernet => "Ethernet",
            LinkKind::Wifi => "Wi-Fi",
        })
    }
}

impl From<LinkKind> for tandem_proto::Link {
    fn from(k: LinkKind) -> Self {
        match k {
            LinkKind::Usb4 => tandem_proto::Link::Usb4,
            LinkKind::Ethernet => tandem_proto::Link::Ethernet,
            LinkKind::Wifi => tandem_proto::Link::Wifi,
        }
    }
}

impl From<tandem_proto::Link> for LinkKind {
    fn from(l: tandem_proto::Link) -> Self {
        match l {
            tandem_proto::Link::Usb4 => LinkKind::Usb4,
            tandem_proto::Link::Ethernet => LinkKind::Ethernet,
            tandem_proto::Link::Wifi => LinkKind::Wifi,
        }
    }
}

/// Dirección del otro agente por un enlace concreto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkCandidate {
    pub kind: LinkKind,
    pub addr: SocketAddr,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_prioridad_sigue_el_diseño() {
        let mut kinds = LinkKind::ALL;
        kinds.reverse();
        kinds.sort();
        assert_eq!(kinds, [LinkKind::Usb4, LinkKind::Ethernet, LinkKind::Wifi]);
        assert!(kinds.windows(2).all(|w| w[0].priority() < w[1].priority()));
    }

    #[test]
    fn el_camino_de_la_ui_va_y_vuelve() {
        for k in LinkKind::ALL {
            assert_eq!(LinkKind::from(tandem_proto::Link::from(k)), k);
        }
    }

    #[test]
    fn el_tipo_se_lee_de_la_config_en_minúsculas() {
        let c: LinkCandidate =
            toml::from_str("kind = \"ethernet\"\naddr = \"10.77.2.2:47770\"").unwrap();
        assert_eq!(c.kind, LinkKind::Ethernet);
        assert_eq!(c.addr, "10.77.2.2:47770".parse().unwrap());
        assert!(toml::from_str::<LinkCandidate>("kind = \"wifi\"\naddr = \"1.2.3.4:1\"").is_ok());
        assert!(toml::from_str::<LinkCandidate>("kind = \"lte\"\naddr = \"1.2.3.4:1\"").is_err());
    }
}
