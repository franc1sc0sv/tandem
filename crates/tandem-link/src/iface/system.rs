use std::collections::BTreeMap;
use std::time::Duration;

use if_addrs::IfOperStatus;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::{Interfaces, NetInterface, classify};

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

/// Las interfaces del sistema, sin loopback, una por nombre y con todas sus direcciones.
fn read() -> Vec<NetInterface> {
    let Ok(all) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let mut by_name: BTreeMap<String, NetInterface> = BTreeMap::new();
    for i in all.into_iter().filter(|i| !i.is_loopback()) {
        let entry = by_name
            .entry(i.name.clone())
            .or_insert_with(|| NetInterface {
                index: i.index.unwrap_or(0),
                kind: classify(&i.name),
                name: i.name.clone(),
                addrs: Vec::new(),
                mac: None,
                speed_mbps: None,
                up: false,
            });
        entry.addrs.push(i.ip());
        entry.up |= matches!(i.oper_status, IfOperStatus::Up | IfOperStatus::Unknown);
    }
    by_name.into_values().collect()
}
