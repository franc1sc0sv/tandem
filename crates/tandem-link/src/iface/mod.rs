//! Network interfaces and cable links detected on their own (T-094, DESIGN §6.1).
//!
//! The operating system is looked at behind [`Interfaces`]: the real implementation is
//! [`SystemInterfaces`] and tests use [`FakeInterfaces`], which plugs and unplugs
//! cables by hand. The rest is pure logic: classifying the interface ([`classify`]), building the
//! link addresses ([`plan`]), labeling with the worse of the two ends
//! ([`worst`]) and keeping the link manager up to date ([`follow_wired`]).

mod addr;
mod assign;
mod classify;
mod discover;
mod fake;
mod follow;
mod resolve;
mod system;

use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

pub use addr::{
    AddrPlan, AssignOrder, FALLBACK_PREFIX, assign_order, fallback_ip, link_local_of, plan,
    scoped_addr,
};
pub use assign::{
    AddrAssigner, AssignError, FakeAssigner, SystemAssigner, assign_command, is_permission_failure,
};
pub use classify::{classify, kind_for_addr, relabel, worst};
pub use discover::{FakeDiscovery, InterfaceDiscovery, ScopedDiscovery, scoped_peer};
pub use fake::FakeInterfaces;
pub use follow::{WiredChange, follow_wired, wired_changes};
pub use resolve::{AssignNotice, AssignNotices, WiredResolver};
pub use system::SystemInterfaces;

use crate::manager::LinkKind;

/// What the system says an interface is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InterfaceKind {
    /// Host-to-host network over USB4 / Thunderbolt (Thunderbolt Bridge on macOS, "USB4 P2P
    /// Network Adapter" on Windows).
    Usb4,
    Ethernet,
    Wifi,
    /// Could not be identified (or it is loopback, VPN, virtual...).
    Other,
}

impl InterfaceKind {
    /// For the link, what is not known counts as Wi-Fi: a Wi-Fi taken for a cable
    /// would carry data without a cap and add latency to input; a cable taken for Wi-Fi
    /// is just slower.
    pub fn link_kind(self) -> LinkKind {
        match self {
            InterfaceKind::Usb4 => LinkKind::Usb4,
            InterfaceKind::Ethernet => LinkKind::Ethernet,
            InterfaceKind::Wifi | InterfaceKind::Other => LinkKind::Wifi,
        }
    }
}

/// A network interface.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NetInterface {
    /// System index; it is the `scope_id` of its link-local addresses.
    pub index: u32,
    pub name: String,
    pub kind: InterfaceKind,
    pub addrs: Vec<IpAddr>,
    pub mac: Option<[u8; 6]>,
    /// Negotiated speed, if the system gives it.
    pub speed_mbps: Option<u64>,
    /// With a cable or active link.
    pub up: bool,
}

/// Addition or removal of an interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterfaceEvent {
    Added(NetInterface),
    Removed(NetInterface),
}

/// The system seen as a list of interfaces with change notification.
pub trait Interfaces: Send + Sync + 'static {
    /// The interfaces of now.
    fn list(&self) -> Vec<NetInterface>;

    /// Follows the list: every change (addition, removal, cable going up or down) publishes it whole.
    fn watch(&self) -> watch::Receiver<Vec<NetInterface>>;
}

/// Additions and removals between two lists. An interface that changes address, type or
/// state comes out as a removal and an addition; one that goes down (`up == false`) counts as a removal.
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
