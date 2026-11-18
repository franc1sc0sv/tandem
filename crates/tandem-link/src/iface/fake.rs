use tokio::sync::watch;

use super::{Interfaces, NetInterface};

/// Interfaces de mentira: se enchufan y desenchufan a mano.
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

    /// Agrega la interfaz o, si su índice ya existe, la reemplaza.
    pub fn plug(&self, iface: NetInterface) {
        self.tx.send_modify(|l| {
            l.retain(|i| i.index != iface.index);
            l.push(iface);
        });
    }

    /// Quita la interfaz (cable desconectado o adaptador retirado).
    pub fn unplug(&self, index: u32) {
        self.tx.send_modify(|l| l.retain(|i| i.index != index));
    }

    /// El cable sigue pero el enlace baja (o sube).
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
