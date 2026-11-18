use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tandem_core::{Machine, Os};

use super::{AddrAssigner, AssignError, InterfaceDiscovery, NetInterface, fallback_ip, plan};

/// Un aviso para la pantalla Red: el cable no tiene la IP de respaldo y el agente no pudo
/// ponérsela; `command` es la orden exacta para el usuario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignNotice {
    pub interface: String,
    pub message: String,
    pub command: String,
}

type Sink = Arc<dyn Fn(Vec<AssignNotice>) + Send + Sync>;

/// Los avisos de asignación vigentes, uno por interfaz. Se clona barato. Cada cambio se
/// entrega a un receptor opcional (el estado que lee la pantalla Red).
#[derive(Clone, Default)]
pub struct AssignNotices {
    inner: Arc<Mutex<BTreeMap<u32, AssignNotice>>>,
    sink: Option<Sink>,
}

impl std::fmt::Debug for AssignNotices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.list()).finish()
    }
}

impl AssignNotices {
    pub fn with_sink(sink: impl Fn(Vec<AssignNotice>) + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::default(),
            sink: Some(Arc::new(sink)),
        }
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<u32, AssignNotice>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn list(&self) -> Vec<AssignNotice> {
        self.lock().values().cloned().collect()
    }

    fn publish(&self) {
        if let Some(sink) = &self.sink {
            sink(self.list());
        }
    }

    fn set(&self, index: u32, notice: AssignNotice) {
        let old = self.lock().insert(index, notice.clone());
        if old.as_ref() != Some(&notice) {
            self.publish();
        }
    }

    fn clear(&self, index: u32) {
        let old = self.lock().remove(&index);
        if old.is_some() {
            self.publish();
        }
    }
}

/// Todo lo que hace falta para decidir dónde está el otro agente por un cable.
pub struct WiredResolver {
    pub side: Machine,
    pub os: Os,
    pub port: u16,
    pub discovery: Arc<dyn InterfaceDiscovery>,
    pub assigner: Arc<dyn AddrAssigner>,
    pub notices: AssignNotices,
}

impl WiredResolver {
    /// Dónde está el otro agente por `iface`. Con link-local, la dirección sale del
    /// descubrimiento de esa interfaz (hasta que conteste, el cable no entra). Sin ella,
    /// el respaldo: se le asigna la IP al cable y solo si el sistema no deja queda el aviso
    /// con la orden exacta.
    pub fn resolve(&self, iface: &NetInterface) -> Option<SocketAddr> {
        let p = plan(
            iface,
            self.side,
            self.os,
            self.discovery.peer_ip(iface),
            self.port,
        );
        let Some(order) = p.assign else {
            self.notices.clear(iface.index);
            return p.peer;
        };
        match self.assigner.assign(iface, fallback_ip(self.side)) {
            Ok(()) => {
                tracing::info!(interfaz = %iface.name, "IP de respaldo asignada al cable");
                self.notices.clear(iface.index);
            }
            Err(e) => {
                let message = match e {
                    AssignError::PermissionDenied => "Falta la IP de respaldo del cable y el \
                        agente no tiene permisos para asignarla; ejecuta la orden como \
                        administrador"
                        .to_owned(),
                    AssignError::Failed(why) => {
                        format!("No se pudo asignar la IP de respaldo del cable ({why})")
                    }
                };
                tracing::warn!(interfaz = %iface.name, "{message}: {}", order.command);
                self.notices.set(
                    iface.index,
                    AssignNotice {
                        interface: iface.name.clone(),
                        message,
                        command: order.command,
                    },
                );
            }
        }
        p.peer
    }
}
