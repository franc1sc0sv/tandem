use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, PoisonError};

use tokio::sync::watch;

use super::NetInterface;

/// Dónde está el otro agente por una interfaz concreta. Con una IPv6 link-local en el
/// cable no hay forma de saber la del otro sin preguntar por esa interfaz: la respuesta
/// sale de mDNS, atado a su `scope_id`.
pub trait InterfaceDiscovery: Send + Sync + 'static {
    /// La dirección del otro agente aprendida por `iface`, si ya contestó.
    fn peer_ip(&self, iface: &NetInterface) -> Option<IpAddr>;
}

/// Descubrimiento real: lo que anuncia el mDNS del agente (que ya escucha en todas las
/// interfaces, también en las de cable) filtrado por el `scope_id` de la interfaz. Una
/// dirección link-local de otra interfaz no sirve para esta.
pub struct ScopedDiscovery {
    announced: watch::Receiver<Vec<SocketAddr>>,
}

impl ScopedDiscovery {
    pub fn new(announced: watch::Receiver<Vec<SocketAddr>>) -> Self {
        Self { announced }
    }
}

impl InterfaceDiscovery for ScopedDiscovery {
    fn peer_ip(&self, iface: &NetInterface) -> Option<IpAddr> {
        scoped_peer(&self.announced.borrow(), iface.index)
    }
}

/// La link-local anunciada con el `scope_id` dado.
pub fn scoped_peer(announced: &[SocketAddr], scope_id: u32) -> Option<IpAddr> {
    announced.iter().find_map(|a| match a {
        SocketAddr::V6(v6) if v6.scope_id() == scope_id && v6.scope_id() != 0 => {
            Some(IpAddr::V6(*v6.ip()))
        }
        _ => None,
    })
}

/// Descubrimiento de mentira: el otro «aparece» y «desaparece» a mano por interfaz.
#[derive(Default)]
pub struct FakeDiscovery {
    peers: Mutex<BTreeMap<u32, IpAddr>>,
}

impl FakeDiscovery {
    pub fn announce(&self, scope_id: u32, ip: IpAddr) {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(scope_id, ip);
    }

    pub fn withdraw(&self, scope_id: u32) {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&scope_id);
    }
}

impl InterfaceDiscovery for FakeDiscovery {
    fn peer_ip(&self, iface: &NetInterface) -> Option<IpAddr> {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&iface.index)
            .copied()
    }
}
