use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

use serde::{Deserialize, Serialize};

use crate::{MacAddr, PowerError};

/// 6 `0xFF` bytes and the MAC repeated 16 times.
pub const MAGIC_PACKET_LEN: usize = 6 + 16 * 6;

/// Usual Wake-on-LAN port (discard). The card does not look at the port, only the content.
pub const DEFAULT_WOL_PORT: u16 = 9;

/// The magic packet that wakes the card with MAC `mac`.
pub fn magic_packet(mac: &MacAddr) -> [u8; MAGIC_PACKET_LEN] {
    let mut p = [0xFF; MAGIC_PACKET_LEN];
    let (copies, rest) = p[6..].as_chunks_mut::<6>();
    debug_assert!(rest.is_empty());
    copies.fill(mac.0);
    p
}

/// To whom and through where the Wake-on-LAN is sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WolTarget {
    /// MAC of the PC's Ethernet card.
    pub mac: MacAddr,
    /// IP of the Mac on the direct Ethernet link: the packet leaves through that interface
    /// (DESIGN §6: Wake-on-LAN only over Ethernet). Without it, the system chooses.
    #[serde(default)]
    pub source: Option<IpAddr>,
    /// Destination. On macOS the broadcast of the link subnet is preferable (e.g.
    /// `10.77.2.3` for `10.77.2.0/30`): the global broadcast leaves through the
    /// main interface, which is usually Wi-Fi.
    #[serde(default = "default_broadcast")]
    pub broadcast: IpAddr,
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_broadcast() -> IpAddr {
    IpAddr::V4(Ipv4Addr::BROADCAST)
}

fn default_port() -> u16 {
    DEFAULT_WOL_PORT
}

impl WolTarget {
    pub fn new(mac: MacAddr) -> Self {
        WolTarget {
            mac,
            source: None,
            broadcast: default_broadcast(),
            port: DEFAULT_WOL_PORT,
        }
    }

    pub fn destination(&self) -> SocketAddr {
        SocketAddr::new(self.broadcast, self.port)
    }
}

/// Sends a Wake-on-LAN. Each call is one send; retrying is up to the caller.
pub trait Wake: Send + Sync {
    fn wake(&self) -> Result<(), PowerError>;
}

/// Wake-on-LAN over UDP.
#[derive(Debug, Clone)]
pub struct UdpWake {
    target: WolTarget,
}

impl UdpWake {
    pub fn new(target: WolTarget) -> Self {
        UdpWake { target }
    }

    pub fn target(&self) -> &WolTarget {
        &self.target
    }
}

impl Wake for UdpWake {
    fn wake(&self) -> Result<(), PowerError> {
        let t = &self.target;
        let to = t.destination();
        let from = match t.source {
            Some(ip) => SocketAddr::new(ip, 0),
            None if to.is_ipv6() => SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 0),
            None => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        };
        let sock = UdpSocket::bind(from).map_err(|error| PowerError::Bind {
            source_ip: from.ip(),
            error,
        })?;
        let send = |source| PowerError::Send { target: to, source };
        sock.set_broadcast(true).map_err(send)?;
        let packet = magic_packet(&t.mac);
        let sent = sock.send_to(&packet, to).map_err(send)?;
        if sent != packet.len() {
            return Err(send(std::io::Error::other(format!(
                "sent {sent} of {} bytes",
                packet.len()
            ))));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use proptest::prelude::*;

    use super::*;

    const PC: MacAddr = MacAddr([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);

    #[test]
    fn magic_packet_format() {
        let p = magic_packet(&PC);
        assert_eq!(p.len(), 102);
        assert_eq!(p[..6], [0xFF; 6]);
        for i in 0..16 {
            let at = 6 + i * 6;
            assert_eq!(p[at..at + 6], PC.0, "repetition {i}");
        }
    }

    #[test]
    fn known_packet_bytes() {
        // Hand-written reference: header and the first two repetitions.
        let p = magic_packet(&"01:23:45:67:89:ab".parse().unwrap());
        assert_eq!(
            p[..18],
            [
                0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0x01, 0x23,
                0x45, 0x67, 0x89, 0xab
            ]
        );
        assert_eq!(p[96..], [0x01, 0x23, 0x45, 0x67, 0x89, 0xab]);
    }

    proptest! {
        #[test]
        fn packet_is_header_and_sixteen_copies(bytes in any::<[u8; 6]>()) {
            let p = magic_packet(&MacAddr(bytes));
            prop_assert!(p[..6].iter().all(|b| *b == 0xFF));
            let (copies, rest) = p[6..].as_chunks::<6>();
            prop_assert!(rest.is_empty());
            prop_assert_eq!(copies.len(), 16);
            prop_assert!(copies.iter().all(|c| *c == bytes));
        }
    }

    #[test]
    fn target_defaults() {
        let t: WolTarget = toml::from_str("mac = \"aa:bb:cc:dd:ee:ff\"").unwrap();
        assert_eq!(t, WolTarget::new(PC));
        assert_eq!(t.destination(), "255.255.255.255:9".parse().unwrap());
        let t: WolTarget = toml::from_str(
            "mac = \"aa:bb:cc:dd:ee:ff\"\nsource = \"10.77.2.1\"\nbroadcast = \"10.77.2.3\"\nport = 7",
        )
        .unwrap();
        assert_eq!(t.source, Some("10.77.2.1".parse().unwrap()));
        assert_eq!(t.destination(), "10.77.2.3:7".parse().unwrap());
        assert!(toml::from_str::<WolTarget>("mac = \"aa:bb:cc:dd:ee:ff\"\nprot = 7").is_err());
    }

    #[test]
    fn sends_the_packet_from_the_configured_source() {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let wake = UdpWake::new(WolTarget {
            mac: PC,
            source: Some("127.0.0.1".parse().unwrap()),
            broadcast: "127.0.0.1".parse().unwrap(),
            port: rx.local_addr().unwrap().port(),
        });
        wake.wake().unwrap();
        let mut buf = [0u8; 256];
        let (n, from) = rx.recv_from(&mut buf).unwrap();
        assert_eq!(buf[..n], magic_packet(&PC));
        assert_eq!(from.ip(), "127.0.0.1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn bad_source_is_a_clear_error() {
        // 192.0.2.0/24 is for documentation: no interface has it.
        let mut t = WolTarget::new(PC);
        t.source = Some("192.0.2.77".parse().unwrap());
        let err = UdpWake::new(t).wake().unwrap_err();
        assert!(matches!(err, PowerError::Bind { .. }), "{err}");
        assert!(err.to_string().contains("192.0.2.77"), "{err}");
    }
}
