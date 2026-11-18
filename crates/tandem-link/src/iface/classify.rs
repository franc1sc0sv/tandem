use std::net::{IpAddr, SocketAddr};

use super::{InterfaceKind, NetInterface};
use crate::manager::{LinkCandidate, LinkKind};

/// Tipo de una interfaz por su nombre (en Windows, el amigable del adaptador).
///
/// Solo se acepta como cable lo que se reconoce con seguridad. En macOS `en0` puede ser
/// Wi-Fi o Ethernet según el equipo: no se adivina y queda `Other`, que cuenta como Wi-Fi.
/// `bridge*` es el Thunderbolt Bridge (ahí se agrupan los `en*` de Thunderbolt).
pub fn classify(name: &str) -> InterfaceKind {
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| n.starts_with(p));
    if has(&["usb4", "thunderbolt"]) || starts(&["bridge"]) {
        InterfaceKind::Usb4
    } else if has(&["wi-fi", "wifi", "wireless", "wlan", "airport"]) || starts(&["wl"]) {
        InterfaceKind::Wifi
    } else if has(&["ethernet"]) || starts(&["eth", "enp", "eno", "ens"]) {
        InterfaceKind::Ethernet
    } else {
        InterfaceKind::Other
    }
}

/// La peor de dos clasificaciones (`Wifi` < `Ethernet` < `Usb4` en calidad): un camino con
/// un tramo de Wi-Fi no es un cable. El orden de [`LinkKind`] pone primero al mejor, así que
/// la peor es la mayor.
pub fn worst(a: LinkKind, b: LinkKind) -> LinkKind {
    a.max(b)
}

/// Kind of the local interface that routes to `addr`, which is what the link really is,
/// whatever the configuration or mDNS said:
///
/// - IPv6 link-local: the interface of its `scope_id` (without one it cannot be routed).
/// - IPv4 link-local (`169.254.0.0/16`, the USB4 / Thunderbolt cable without DHCP): an
///   interface with an address in that range. If several have one, the best kind wins:
///   a cable network is link-local by design, while on Wi-Fi or Ethernet it only means
///   DHCP failed.
/// - Other IPv4: an interface in the same `/24`.
///
/// A dual-stack socket reports IPv4 peers as `::ffff:a.b.c.d`; they count as IPv4.
pub fn kind_for_addr(interfaces: &[NetInterface], addr: SocketAddr) -> Option<LinkKind> {
    let up = || interfaces.iter().filter(|i| i.up);
    match unmapped(addr.ip()) {
        IpAddr::V6(_) => match addr {
            SocketAddr::V6(v6) if v6.scope_id() != 0 => up()
                .find(|i| i.index == v6.scope_id())
                .map(|i| i.kind.link_kind()),
            _ => None,
        },
        IpAddr::V4(v4) if v4.is_link_local() => up()
            .filter(|i| {
                i.addrs
                    .iter()
                    .any(|a| matches!(unmapped(*a), IpAddr::V4(a) if a.is_link_local()))
            })
            .map(|i| i.kind.link_kind())
            .min(),
        IpAddr::V4(v4) => up()
            .find(|i| {
                i.addrs
                    .iter()
                    .any(|a| matches!(unmapped(*a), IpAddr::V4(a) if a.octets()[..3] == v4.octets()[..3]))
            })
            .map(|i| i.kind.link_kind()),
    }
}

fn unmapped(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// Reetiqueta los candidatos con el tipo de su interfaz. Los que no caen en ninguna
/// interfaz conocida se quedan como están.
pub fn relabel(candidates: &[LinkCandidate], interfaces: &[NetInterface]) -> Vec<LinkCandidate> {
    candidates
        .iter()
        .map(|c| LinkCandidate {
            kind: kind_for_addr(interfaces, c.addr).unwrap_or(c.kind),
            addr: c.addr,
        })
        .collect()
}
