use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

use serde::{Deserialize, Serialize};

use crate::{MacAddr, PowerError};

/// 6 bytes `0xFF` y la MAC repetida 16 veces.
pub const MAGIC_PACKET_LEN: usize = 6 + 16 * 6;

/// Puerto habitual del Wake-on-LAN (discard). La tarjeta no mira el puerto, solo el contenido.
pub const DEFAULT_WOL_PORT: u16 = 9;

/// El paquete mágico que despierta la tarjeta con la MAC `mac`.
pub fn magic_packet(mac: &MacAddr) -> [u8; MAGIC_PACKET_LEN] {
    let mut p = [0xFF; MAGIC_PACKET_LEN];
    let (copies, rest) = p[6..].as_chunks_mut::<6>();
    debug_assert!(rest.is_empty());
    copies.fill(mac.0);
    p
}

/// A quién y por dónde se manda el Wake-on-LAN.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WolTarget {
    /// MAC de la tarjeta Ethernet de la PC.
    pub mac: MacAddr,
    /// IP de la Mac en el enlace Ethernet directo: el paquete sale por esa interfaz
    /// (DESIGN §6: Wake-on-LAN solo por Ethernet). Sin ella, el sistema elige.
    #[serde(default)]
    pub source: Option<IpAddr>,
    /// Destino. En macOS conviene el broadcast de la subred del enlace (p. ej.
    /// `10.77.2.3` para `10.77.2.0/30`): el broadcast global sale por la interfaz
    /// principal, que suele ser el Wi-Fi.
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

/// Manda un Wake-on-LAN. Cada llamada es un envío; reintentar es cosa de quien llama.
pub trait Wake: Send + Sync {
    fn wake(&self) -> Result<(), PowerError>;
}

/// Wake-on-LAN por UDP.
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
                "se mandaron {sent} de {} bytes",
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

    const PC: MacAddr = MacAddr([0x9c, 0x6b, 0x00, 0x1a, 0x2b, 0xff]);

    #[test]
    fn magic_packet_format() {
        let p = magic_packet(&PC);
        assert_eq!(p.len(), 102);
        assert_eq!(p[..6], [0xFF; 6]);
        for i in 0..16 {
            let at = 6 + i * 6;
            assert_eq!(p[at..at + 6], PC.0, "repetición {i}");
        }
    }

    #[test]
    fn known_packet_bytes() {
        // Referencia escrita a mano: cabecera y las dos primeras repeticiones.
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
        let t: WolTarget = toml::from_str("mac = \"9c:6b:00:1a:2b:ff\"").unwrap();
        assert_eq!(t, WolTarget::new(PC));
        assert_eq!(t.destination(), "255.255.255.255:9".parse().unwrap());
        let t: WolTarget = toml::from_str(
            "mac = \"9c:6b:00:1a:2b:ff\"\nsource = \"10.77.2.1\"\nbroadcast = \"10.77.2.3\"\nport = 7",
        )
        .unwrap();
        assert_eq!(t.source, Some("10.77.2.1".parse().unwrap()));
        assert_eq!(t.destination(), "10.77.2.3:7".parse().unwrap());
        assert!(toml::from_str::<WolTarget>("mac = \"9c:6b:00:1a:2b:ff\"\nprot = 7").is_err());
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
        // 192.0.2.0/24 es de documentación: ninguna interfaz la tiene.
        let mut t = WolTarget::new(PC);
        t.source = Some("192.0.2.77".parse().unwrap());
        let err = UdpWake::new(t).wake().unwrap_err();
        assert!(matches!(err, PowerError::Bind { .. }), "{err}");
        assert!(err.to_string().contains("192.0.2.77"), "{err}");
    }
}
