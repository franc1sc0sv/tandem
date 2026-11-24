use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tandem_core::{Machine, Os};

use super::{AddrAssigner, AssignError, InterfaceDiscovery, NetInterface, fallback_ip, plan};

/// A notice for the Network screen: the cable does not have the fallback IP and the agent could not
/// set it; `command` is the exact command for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignNotice {
    pub interface: String,
    pub message: String,
    pub command: String,
}

type Sink = Arc<dyn Fn(Vec<AssignNotice>) + Send + Sync>;

/// The current assignment notices, one per interface. Cheap to clone. Every change is
/// delivered to an optional receiver (the state the Network screen reads).
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

/// Everything needed to decide where the other agent is through a cable.
pub struct WiredResolver {
    pub side: Machine,
    pub os: Os,
    pub port: u16,
    pub discovery: Arc<dyn InterfaceDiscovery>,
    pub assigner: Arc<dyn AddrAssigner>,
    pub notices: AssignNotices,
}

impl WiredResolver {
    /// Where the other agent is through `iface`. With link-local, the address comes from the
    /// discovery of that interface (until it answers, the cable does not come in). Without it,
    /// the fallback: the IP is assigned to the cable and only if the system does not allow it the notice
    /// with the exact command remains.
    pub fn resolve(&self, iface: &NetInterface) -> Option<SocketAddr> {
        let p = plan(
            iface,
            self.side,
            self.os,
            self.discovery.peer_addr(iface),
            self.port,
        );
        let Some(order) = p.assign else {
            self.notices.clear(iface.index);
            return p.peer;
        };
        match self.assigner.assign(iface, fallback_ip(self.side)) {
            Ok(()) => {
                tracing::info!(interface = %iface.name, "fallback IP assigned to the cable");
                self.notices.clear(iface.index);
            }
            Err(e) => {
                let message = match e {
                    AssignError::PermissionDenied => "The cable's fallback IP is missing and the \
                        agent has no permission to assign it; run the command as \
                        administrator"
                        .to_owned(),
                    AssignError::Failed(why) => {
                        format!("Could not assign the cable's fallback IP ({why})")
                    }
                };
                tracing::warn!(interface = %iface.name, "{message}: {}", order.command);
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
