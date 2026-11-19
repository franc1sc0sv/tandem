use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, PoisonError};

use tokio::sync::watch;

use super::NetInterface;

/// Where the other agent is through a specific interface. With a link-local IPv6 on the
/// cable there is no way to know the other's without asking through that interface: the answer
/// comes from mDNS, tied to its `scope_id`.
pub trait InterfaceDiscovery: Send + Sync + 'static {
    /// The address of the other agent learned through `iface`, if it already answered, with the port
    /// it announces (not the configured one: the other may listen on another).
    fn peer_addr(&self, iface: &NetInterface) -> Option<SocketAddr>;
}

/// Real discovery: what the agent's mDNS announces (which already listens on all the
/// interfaces, cable ones too) filtered by the interface's `scope_id`. A link-local
/// address from another interface is of no use for this one.
pub struct ScopedDiscovery {
    announced: watch::Receiver<Vec<SocketAddr>>,
}

impl ScopedDiscovery {
    pub fn new(announced: watch::Receiver<Vec<SocketAddr>>) -> Self {
        Self { announced }
    }
}

impl InterfaceDiscovery for ScopedDiscovery {
    fn peer_addr(&self, iface: &NetInterface) -> Option<SocketAddr> {
        scoped_peer(&self.announced.borrow(), iface.index)
    }
}

/// The link-local announced with the given `scope_id`.
pub fn scoped_peer(announced: &[SocketAddr], scope_id: u32) -> Option<SocketAddr> {
    announced.iter().find_map(|a| match a {
        SocketAddr::V6(v6) if v6.scope_id() == scope_id && v6.scope_id() != 0 => {
            Some(SocketAddr::new(IpAddr::V6(*v6.ip()), v6.port()))
        }
        _ => None,
    })
}

/// Fake discovery: the other "appears" and "disappears" by hand per interface.
#[derive(Default)]
pub struct FakeDiscovery {
    peers: Mutex<BTreeMap<u32, SocketAddr>>,
}

impl FakeDiscovery {
    pub fn announce(&self, scope_id: u32, addr: SocketAddr) {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(scope_id, addr);
    }

    pub fn withdraw(&self, scope_id: u32) {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&scope_id);
    }
}

impl InterfaceDiscovery for FakeDiscovery {
    fn peer_addr(&self, iface: &NetInterface) -> Option<SocketAddr> {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&iface.index)
            .copied()
    }
}
