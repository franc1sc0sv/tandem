use std::net::{IpAddr, SocketAddr};

use super::{InterfaceKind, NetInterface};
use crate::manager::{LinkCandidate, LinkKind};

/// Type of an interface by its name (on Windows, the adapter's friendly one).
///
/// Only what is recognized with certainty is accepted as a cable. On macOS `en0` can be
/// Wi-Fi or Ethernet depending on the machine: it is not guessed and stays `Other`, which counts as Wi-Fi.
/// `bridge*` is the Thunderbolt Bridge (that is where the Thunderbolt `en*` are grouped).
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

/// The worse of two classifications (`Wifi` < `Ethernet` < `Usb4` in quality): a path with
/// a Wi-Fi stretch is not a cable. The order of [`LinkKind`] puts the best first, so
/// the worst is the greatest.
pub fn worst(a: LinkKind, b: LinkKind) -> LinkKind {
    a.max(b)
}

/// The interface through which `addr` is reached: the one that has an address on the same network
/// (IPv4 with a `/24` prefix, or link-local with its `scope_id`). The link's type comes from there
/// and not from what the configuration says.
pub fn kind_for_addr(interfaces: &[NetInterface], addr: SocketAddr) -> Option<LinkKind> {
    interfaces
        .iter()
        .filter(|i| i.up)
        .find(|i| match addr {
            SocketAddr::V6(v6) if v6.scope_id() != 0 => v6.scope_id() == i.index,
            _ => i.addrs.iter().any(|a| same_net(*a, addr.ip())),
        })
        .map(|i| i.kind.link_kind())
}

fn same_net(a: IpAddr, b: IpAddr) -> bool {
    match (a, b) {
        (IpAddr::V4(a), IpAddr::V4(b)) => a.octets()[..3] == b.octets()[..3],
        _ => false,
    }
}

/// Relabels the candidates with the type of their interface. Those that fall on no
/// known interface stay as they are.
pub fn relabel(candidates: &[LinkCandidate], interfaces: &[NetInterface]) -> Vec<LinkCandidate> {
    candidates
        .iter()
        .map(|c| LinkCandidate {
            kind: kind_for_addr(interfaces, c.addr).unwrap_or(c.kind),
            addr: c.addr,
        })
        .collect()
}
