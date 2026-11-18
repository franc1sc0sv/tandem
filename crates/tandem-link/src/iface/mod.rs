//! Interfaces de red y enlaces por cable detectados solos (T-094, DESIGN §6.1).
//!
//! El sistema operativo se mira detrás de [`Interfaces`]: la implementación real es
//! [`SystemInterfaces`] y las pruebas usan [`FakeInterfaces`], que enchufa y desenchufa
//! cables a mano. Lo demás es lógica pura: clasificar la interfaz ([`classify`]), armar las
//! direcciones del enlace ([`plan`]), etiquetar con la peor de las dos puntas
//! ([`worst`]) y mantener el gestor de enlaces al día ([`follow_wired`]).

mod addr;
mod classify;
mod fake;
mod follow;
mod system;

use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

pub use addr::{
    AddrPlan, AssignOrder, FALLBACK_PREFIX, assign_order, fallback_ip, link_local_of, plan,
    scoped_addr,
};
pub use classify::{classify, kind_for_addr, relabel, worst};
pub use fake::FakeInterfaces;
pub use follow::{WiredChange, follow_wired, wired_changes};
pub use system::SystemInterfaces;

use crate::manager::LinkKind;

/// Lo que el sistema dice que es una interfaz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InterfaceKind {
    /// Red entre hosts por USB4 / Thunderbolt (Thunderbolt Bridge en macOS, «USB4 P2P
    /// Network Adapter» en Windows).
    Usb4,
    Ethernet,
    Wifi,
    /// No se pudo identificar (o es loopback, VPN, virtual...).
    Other,
}

impl InterfaceKind {
    /// Para el enlace, lo que no se sabe cuenta como Wi-Fi: un Wi-Fi tomado por cable
    /// llevaría datos sin tope y le metería latencia a la entrada; un cable tomado por Wi-Fi
    /// solo va más lento.
    pub fn link_kind(self) -> LinkKind {
        match self {
            InterfaceKind::Usb4 => LinkKind::Usb4,
            InterfaceKind::Ethernet => LinkKind::Ethernet,
            InterfaceKind::Wifi | InterfaceKind::Other => LinkKind::Wifi,
        }
    }
}

/// Una interfaz de red.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NetInterface {
    /// Índice del sistema; es el `scope_id` de sus direcciones link-local.
    pub index: u32,
    pub name: String,
    pub kind: InterfaceKind,
    pub addrs: Vec<IpAddr>,
    pub mac: Option<[u8; 6]>,
    /// Velocidad negociada, si el sistema la da.
    pub speed_mbps: Option<u64>,
    /// Con cable o enlace activo.
    pub up: bool,
}

/// Alta o baja de una interfaz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterfaceEvent {
    Added(NetInterface),
    Removed(NetInterface),
}

/// El sistema visto como lista de interfaces con aviso de cambios.
pub trait Interfaces: Send + Sync + 'static {
    /// Las interfaces de ahora.
    fn list(&self) -> Vec<NetInterface>;

    /// Sigue la lista: cada cambio (alta, baja, cable que sube o baja) la publica entera.
    fn watch(&self) -> watch::Receiver<Vec<NetInterface>>;
}

/// Altas y bajas entre dos listas. Una interfaz que cambia de dirección, de tipo o de
/// estado sale como baja y alta; una que baja (`up == false`) cuenta como baja.
pub fn diff(old: &[NetInterface], new: &[NetInterface]) -> Vec<InterfaceEvent> {
    let live = |l: &[NetInterface]| l.iter().filter(|i| i.up).cloned().collect::<Vec<_>>();
    let (old, new) = (live(old), live(new));
    let mut events: Vec<InterfaceEvent> = old
        .iter()
        .filter(|i| !new.contains(i))
        .cloned()
        .map(InterfaceEvent::Removed)
        .collect();
    events.extend(
        new.iter()
            .filter(|i| !old.contains(i))
            .cloned()
            .map(InterfaceEvent::Added),
    );
    events
}

#[cfg(test)]
mod tests;
