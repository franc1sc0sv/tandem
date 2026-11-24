use std::collections::BTreeMap;
use std::net::SocketAddr;

use tokio::task::JoinHandle;

use super::{InterfaceKind, Interfaces, NetInterface};
use crate::manager::{LinkCandidate, LinkControl, LinkKind};

/// What has to be done to the manager so that it matches the interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WiredChange {
    Add(LinkCandidate),
    Remove(LinkKind),
}

/// Wired links according to the interfaces of now. `current` is what was already applied
/// (type → interface index and address): it is updated and only the difference is returned.
///
/// Each cable type uses one interface, the lowest-index one that is up and for which
/// `resolve` knows where the other agent is. Wi-Fi does not go through here: it is the LAN, which
/// its own mDNS discovers.
///
/// A link is only removed when its interface is gone or down, or another interface takes its
/// type. While its interface stays, a `resolve` that loses the other agent for a moment (mDNS
/// answers that come and go) changes nothing, and a new address comes out as an addition alone:
/// the manager keeps a healthy link's connection and only dials the new address once that one
/// drops.
pub fn wired_changes(
    current: &mut BTreeMap<LinkKind, (u32, SocketAddr)>,
    interfaces: &[NetInterface],
    resolve: &dyn Fn(&NetInterface) -> Option<SocketAddr>,
) -> Vec<WiredChange> {
    let mut wanted: BTreeMap<LinkKind, (u32, SocketAddr)> = BTreeMap::new();
    let mut sorted: Vec<&NetInterface> = interfaces
        .iter()
        .filter(|i| i.up && matches!(i.kind, InterfaceKind::Usb4 | InterfaceKind::Ethernet))
        .collect();
    sorted.sort_by_key(|i| i.index);
    for i in sorted {
        let kind = i.kind.link_kind();
        if wanted.contains_key(&kind) {
            continue;
        }
        let kept = current.get(&kind).filter(|(index, _)| *index == i.index);
        if let Some(addr) = resolve(i) {
            wanted.insert(kind, (i.index, addr));
        } else if let Some(have) = kept {
            wanted.insert(kind, *have);
        }
    }
    let mut changes = Vec::new();
    for (kind, (index, _)) in current.iter() {
        if wanted.get(kind).is_none_or(|(i, _)| i != index) {
            changes.push(WiredChange::Remove(*kind));
        }
    }
    for (kind, want) in &wanted {
        if current.get(kind) != Some(want) {
            changes.push(WiredChange::Add(LinkCandidate {
                kind: *kind,
                addr: want.1,
            }));
        }
    }
    *current = wanted;
    changes
}

/// Keeps the link manager up to date with the interfaces: when a cable appears, its
/// link enters; when it goes down, it leaves at once, without waiting for the health check. It ends
/// when the [`JoinHandle`] is dropped (it is aborted) or when the manager closes.
pub fn follow_wired<I, F>(interfaces: &I, control: LinkControl, resolve: F) -> JoinHandle<()>
where
    I: Interfaces + ?Sized,
    F: Fn(&NetInterface) -> Option<SocketAddr> + Send + 'static,
{
    let mut rx = interfaces.watch();
    tokio::spawn(async move {
        let mut current = BTreeMap::new();
        loop {
            let list = rx.borrow_and_update().clone();
            for change in wired_changes(&mut current, &list, &resolve) {
                let done = match change {
                    WiredChange::Add(c) => {
                        tracing::info!(link = %c.kind, addr = %c.addr, "cable interface detected");
                        control.add(c)
                    }
                    WiredChange::Remove(kind) => {
                        tracing::info!(link = %kind, "cable interface down");
                        control.remove(kind)
                    }
                };
                if done.is_err() {
                    return;
                }
            }
            if rx.changed().await.is_err() {
                return;
            }
        }
    })
}
