use std::collections::BTreeMap;
use std::net::SocketAddr;

use tokio::task::JoinHandle;

use super::{InterfaceKind, Interfaces, NetInterface};
use crate::manager::{LinkCandidate, LinkControl, LinkKind};

/// Lo que hay que hacerle al gestor para que coincida con las interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WiredChange {
    Add(LinkCandidate),
    Remove(LinkKind),
}

/// Enlaces por cable según las interfaces de ahora. `current` es lo que ya se aplicó
/// (tipo → índice de la interfaz y dirección): se actualiza y devuelve solo la diferencia.
///
/// Cada tipo de cable usa una interfaz, la de menor índice que esté arriba y de la que
/// `resolve` sepa dónde está el otro agente. El Wi-Fi no pasa por aquí: es la LAN, que
/// descubre su propio mDNS. Si la dirección o la interfaz de un tipo cambia, sale una baja
/// y una alta.
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
        if let Some(addr) = resolve(i) {
            wanted.insert(kind, (i.index, addr));
        }
    }
    let mut changes = Vec::new();
    for (kind, have) in current.iter() {
        if wanted.get(kind) != Some(have) {
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

/// Mantiene el gestor de enlaces al día con las interfaces: al aparecer un cable, su
/// enlace entra; al bajar, sale en el acto, sin esperar a la revisión de salud. Termina
/// al soltar el [`JoinHandle`] (se aborta) o al cerrarse el gestor.
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
                        tracing::info!(link = %c.kind, addr = %c.addr, "interfaz de cable detectada");
                        control.add(c)
                    }
                    WiredChange::Remove(kind) => {
                        tracing::info!(link = %kind, "interfaz de cable caída");
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
