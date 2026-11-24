use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};

use tandem_core::{Machine, Os};

use super::NetInterface;

/// Prefix of the fallback network: `10.77.1.0/30`.
pub const FALLBACK_PREFIX: u8 = 30;

/// Fallback address of each side in `10.77.1.0/30`: `.1` for side A and `.2` for B.
/// They are the desk's sides (A/B), which both agents know the same way.
pub fn fallback_ip(side: Machine) -> Ipv4Addr {
    match side {
        Machine::A => Ipv4Addr::new(10, 77, 1, 1),
        Machine::B => Ipv4Addr::new(10, 77, 1, 2),
    }
}

fn is_link_local(ip: &Ipv6Addr) -> bool {
    ip.segments()[0] & 0xffc0 == 0xfe80
}

/// The interface's link-local IPv6, if the system gave it one.
pub fn link_local_of(iface: &NetInterface) -> Option<Ipv6Addr> {
    iface.addrs.iter().find_map(|a| match a {
        IpAddr::V6(v6) if is_link_local(v6) => Some(*v6),
        _ => None,
    })
}

/// Address to connect to `ip` through interface `scope_id`. A link-local one is only valid with
/// its `scope_id` (without it, the system does not know which interface to leave through).
pub fn scoped_addr(ip: IpAddr, scope_id: u32, port: u16) -> SocketAddr {
    match ip {
        IpAddr::V6(v6) if is_link_local(&v6) => {
            SocketAddr::V6(SocketAddrV6::new(v6, port, 0, scope_id))
        }
        ip => SocketAddr::new(ip, port),
    }
}

/// The exact command to assign `ip` to the interface, to show it when the agent has no
/// permission to do it itself.
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

/// Where each end of the cable is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddrPlan {
    /// This machine's address on the link.
    pub local: IpAddr,
    /// Where the other agent is; `None` if not yet known (its link-local comes from
    /// mDNS on the interface).
    pub peer: Option<SocketAddr>,
    /// Whether the fallback IP has to be assigned by hand: the command.
    pub assign: Option<AssignOrder>,
}

/// Link addresses through `iface`. `peer` is the other agent's (with the port it
/// announces) if discovery already gave it.
///
/// 1. If the interface has a link-local IPv6, it is used with its `scope_id`.
/// 2. Otherwise, or if it already has the IP of `10.77.1.0/30`, the fallback is used: A is `.1` and B `.2`.
pub fn plan(
    iface: &NetInterface,
    side: Machine,
    os: Os,
    peer: Option<SocketAddr>,
    port: u16,
) -> AddrPlan {
    let mine = fallback_ip(side);
    let theirs = fallback_ip(side.other());
    let has_fallback = iface.addrs.contains(&IpAddr::V4(mine));
    if let (Some(ll), false) = (link_local_of(iface), has_fallback) {
        return AddrPlan {
            local: IpAddr::V6(ll),
            peer: peer.map(|p| scoped_addr(p.ip(), iface.index, p.port())),
            assign: None,
        };
    }
    AddrPlan {
        local: IpAddr::V4(mine),
        peer: Some(peer.unwrap_or_else(|| SocketAddr::new(IpAddr::V4(theirs), port))),
        assign: (!has_fallback).then(|| assign_order(os, &iface.name, mine)),
    }
}
