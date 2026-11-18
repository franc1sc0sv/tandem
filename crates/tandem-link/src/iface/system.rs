use std::collections::BTreeMap;
use std::time::Duration;

use if_addrs::IfOperStatus;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::{Interfaces, NetInterface, classify};

/// How often the system's list is read again.
pub const POLL: Duration = Duration::from_millis(500);

/// The real interfaces (`getifaddrs` on macOS and Linux, `GetAdaptersAddresses` on
/// Windows, through `if-addrs`). It is read every [`POLL`] and only what changed is published.
///
/// Pending hardware: native notifications (`SCDynamicStore`, `NotifyIpInterfaceChange`)
/// would lower the detection of a removal from 0.5 s to almost nothing, and the MAC, the negotiated speed and
/// the type via `IfType` / `SCNetworkInterface` are not given by this reading.
pub struct SystemInterfaces {
    state: watch::Receiver<Vec<NetInterface>>,
    task: JoinHandle<()>,
}

impl SystemInterfaces {
    /// Starts reading. It has to be called inside a tokio runtime.
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

/// The system's interfaces, without loopback, one per name and with all their addresses.
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
