use std::collections::BTreeMap;
use std::time::Duration;

use if_addrs::IfOperStatus;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::table::{AddrRow, interfaces_from_rows};
use super::{Interfaces, NetInterface};

/// Cada cuánto se vuelve a leer la lista del sistema.
pub const POLL: Duration = Duration::from_millis(500);

/// Las interfaces reales (`getifaddrs` en macOS y Linux, `GetAdaptersAddresses` en
/// Windows, por `if-addrs`). Se lee cada [`POLL`] y se publica solo lo que cambió.
///
/// Pendiente de hardware: los avisos nativos (`SCDynamicStore`, `NotifyIpInterfaceChange`)
/// bajarían la detección de una baja de 0,5 s a casi nada, y la MAC, la velocidad negociada y
/// el tipo por `IfType` / `SCNetworkInterface` no los da esta lectura.
pub struct SystemInterfaces {
    state: watch::Receiver<Vec<NetInterface>>,
    task: JoinHandle<()>,
}

impl SystemInterfaces {
    /// Empieza a leer. Tiene que llamarse dentro de un runtime de tokio.
    pub fn spawn() -> Self {
        let (tx, state) = watch::channel(read());
        let task = tokio::spawn(async move {
            let mut tick = tokio::time::interval(POLL);
            loop {
                tick.tick().await;
                let now = read();
                tx.send_if_modified(|old| {
                    let changed = *old != now;
                    if changed {
                        *old = now;
                    }
                    changed
                });
            }
        });
        Self { state, task }
    }
}

impl Drop for SystemInterfaces {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Interfaces for SystemInterfaces {
    fn list(&self) -> Vec<NetInterface> {
        self.state.borrow().clone()
    }

    fn watch(&self) -> watch::Receiver<Vec<NetInterface>> {
        self.state.clone()
    }
}

/// The OS interfaces, without loopback, one per name with all their addresses.
fn read() -> Vec<NetInterface> {
    let Ok(all) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let descriptions = descriptions();
    interfaces_from_rows(all.into_iter().filter(|i| !i.is_loopback()).map(|i| {
        let index = i.index.unwrap_or(0);
        AddrRow {
            description: descriptions.get(&index).cloned(),
            ip: i.ip(),
            up: matches!(i.oper_status, IfOperStatus::Up | IfOperStatus::Unknown),
            name: i.name,
            index,
        }
    }))
}

/// Hardware description of each adapter by interface index. Only Windows has one worth
/// reading: there the friendly name of the USB4 cable is a plain "Ethernet N".
#[cfg(windows)]
fn descriptions() -> BTreeMap<u32, String> {
    super::adapters_windows::descriptions()
}

#[cfg(not(windows))]
fn descriptions() -> BTreeMap<u32, String> {
    BTreeMap::new()
}
