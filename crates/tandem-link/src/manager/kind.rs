use std::fmt;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

/// Type of link between the two machines (DESIGN §6).
///
/// The order of the variants is the priority: `Usb4 < Ethernet < Wifi`, so
/// the best healthy link is the smallest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkKind {
    /// Direct USB4 / Thunderbolt.
    Usb4,
    /// Direct 2.5G Ethernet.
    Ethernet,
    /// Home LAN Wi-Fi.
    Wifi,
}

impl LinkKind {
    pub const ALL: [LinkKind; 3] = [LinkKind::Usb4, LinkKind::Ethernet, LinkKind::Wifi];

    /// Priority from the table in DESIGN §6 (1 = the best).
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

/// Address of the other agent over a specific link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkCandidate {
    pub kind: LinkKind,
    pub addr: SocketAddr,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_follows_the_design() {
        let mut kinds = LinkKind::ALL;
        kinds.reverse();
        kinds.sort();
        assert_eq!(kinds, [LinkKind::Usb4, LinkKind::Ethernet, LinkKind::Wifi]);
        assert!(kinds.windows(2).all(|w| w[0].priority() < w[1].priority()));
    }

    #[test]
    fn the_ui_path_round_trips() {
        for k in LinkKind::ALL {
            assert_eq!(LinkKind::from(tandem_proto::Link::from(k)), k);
        }
    }

    #[test]
    fn the_kind_is_read_from_the_config_in_lowercase() {
        let c: LinkCandidate =
            toml::from_str("kind = \"ethernet\"\naddr = \"10.77.2.2:47770\"").unwrap();
        assert_eq!(c.kind, LinkKind::Ethernet);
        assert_eq!(c.addr, "10.77.2.2:47770".parse().unwrap());
        assert!(toml::from_str::<LinkCandidate>("kind = \"wifi\"\naddr = \"1.2.3.4:1\"").is_ok());
        assert!(toml::from_str::<LinkCandidate>("kind = \"lte\"\naddr = \"1.2.3.4:1\"").is_err());
    }
}
