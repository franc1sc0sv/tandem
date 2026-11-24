use tokio::sync::watch;

use super::{Interfaces, NetInterface};

/// Fake interfaces: plugged and unplugged by hand.
#[derive(Debug, Clone)]
pub struct FakeInterfaces {
    tx: watch::Sender<Vec<NetInterface>>,
}

impl FakeInterfaces {
    pub fn new(initial: Vec<NetInterface>) -> Self {
        Self {
            tx: watch::Sender::new(initial),
        }
    }

    /// Adds the interface or, if its index already exists, replaces it.
    pub fn plug(&self, iface: NetInterface) {
        self.tx.send_modify(|l| {
            l.retain(|i| i.index != iface.index);
            l.push(iface);
        });
    }

    /// Removes the interface (cable disconnected or adapter removed).
    pub fn unplug(&self, index: u32) {
        self.tx.send_modify(|l| l.retain(|i| i.index != index));
    }

    /// The cable stays but the link goes down (or up).
    pub fn set_up(&self, index: u32, up: bool) {
        self.tx.send_modify(|l| {
            for i in l.iter_mut().filter(|i| i.index == index) {
                i.up = up;
            }
        });
    }
}

impl Interfaces for FakeInterfaces {
    fn list(&self) -> Vec<NetInterface> {
        self.tx.borrow().clone()
    }

    fn watch(&self) -> watch::Receiver<Vec<NetInterface>> {
        self.tx.subscribe()
    }
}
