use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};

use tandem_core::{Machine, Os};

use super::NetInterface;

/// Prefijo de la red de respaldo: `10.77.1.0/30`.
pub const FALLBACK_PREFIX: u8 = 30;

/// Dirección de respaldo de cada lado en `10.77.1.0/30`: `.1` para el lado A y `.2` para el B.
/// Son los lados del puesto (A/B), que los dos agentes conocen igual.
pub fn fallback_ip(side: Machine) -> Ipv4Addr {
    match side {
        Machine::A => Ipv4Addr::new(10, 77, 1, 1),
        Machine::B => Ipv4Addr::new(10, 77, 1, 2),
    }
}

fn is_link_local(ip: &Ipv6Addr) -> bool {
    ip.segments()[0] & 0xffc0 == 0xfe80
}

/// La IPv6 link-local de la interfaz, si el sistema se la dio.
pub fn link_local_of(iface: &NetInterface) -> Option<Ipv6Addr> {
    iface.addrs.iter().find_map(|a| match a {
        IpAddr::V6(v6) if is_link_local(v6) => Some(*v6),
        _ => None,
    })
}

/// Dirección para conectar a `ip` por la interfaz `scope_id`. Una link-local solo vale con
/// su `scope_id` (sin él, el sistema no sabe por qué interfaz salir).
pub fn scoped_addr(ip: IpAddr, scope_id: u32, port: u16) -> SocketAddr {
    match ip {
        IpAddr::V6(v6) if is_link_local(&v6) => {
            SocketAddr::V6(SocketAddrV6::new(v6, port, 0, scope_id))
        }
        ip => SocketAddr::new(ip, port),
    }
}

/// La orden exacta para asignar `ip` a la interfaz, para mostrarla cuando el agente no tiene
/// permisos de hacerlo solo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignOrder {
    pub command: String,
}

pub fn assign_order(os: Os, iface: &str, ip: Ipv4Addr) -> AssignOrder {
    let command = match os {
        Os::MacOs => format!("sudo ifconfig {iface} inet {ip} netmask 255.255.255.252"),
        Os::Windows => {
            format!("netsh interface ip set address name=\"{iface}\" static {ip} 255.255.255.252")
        }
    };
    AssignOrder { command }
}

/// Dónde está cada punta del cable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddrPlan {
    /// La dirección de esta máquina en el enlace.
    pub local: IpAddr,
    /// Dónde está el otro agente; `None` si todavía no se sabe (su link-local sale de
    /// mDNS en la interfaz).
    pub peer: Option<SocketAddr>,
    /// Si hay que asignar la IP de respaldo a mano: la orden.
    pub assign: Option<AssignOrder>,
}

/// Direcciones del enlace por `iface`. `peer_ip` es la del otro agente si el
/// descubrimiento ya la dio.
///
/// 1. Si la interfaz tiene IPv6 link-local, se usa con su `scope_id`.
/// 2. Si no, o si ya tiene la IP de `10.77.1.0/30`, se usa el respaldo: A es `.1` y B `.2`.
pub fn plan(
    iface: &NetInterface,
    side: Machine,
    os: Os,
    peer_ip: Option<IpAddr>,
    port: u16,
) -> AddrPlan {
    let mine = fallback_ip(side);
    let theirs = fallback_ip(side.other());
    let has_fallback = iface.addrs.contains(&IpAddr::V4(mine));
    if let (Some(ll), false) = (link_local_of(iface), has_fallback) {
        return AddrPlan {
            local: IpAddr::V6(ll),
            peer: peer_ip.map(|ip| scoped_addr(ip, iface.index, port)),
            assign: None,
        };
    }
    AddrPlan {
        local: IpAddr::V4(mine),
        peer: Some(SocketAddr::new(peer_ip.unwrap_or(IpAddr::V4(theirs)), port)),
        assign: (!has_fallback).then(|| assign_order(os, &iface.name, mine)),
    }
}
