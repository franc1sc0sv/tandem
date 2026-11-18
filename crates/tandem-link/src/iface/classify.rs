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

/// La interfaz por la que se llega a `addr`: la que tiene una dirección de su misma red
/// (IPv4 con prefijo `/24`, o link-local con su `scope_id`). El tipo del enlace sale de ahí
/// y no de lo que diga la configuración.
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
