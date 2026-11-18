//! Los procesos de un túnel en la destino: su estado, su hijo y cuándo se olvidan.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tandem_proto::{ProcessId, ProcessState, TunnelProcess};
use tokio::time::Instant;

use super::Child;

pub(crate) struct Table {
    next: AtomicU32,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Sube con cada cambio; cada entrada guarda el de su último cambio.
    rev: u64,
    entries: BTreeMap<ProcessId, Entry>,
}

struct Entry {
    info: TunnelProcess,
    rev: u64,
    child: Arc<dyn Child>,
    started: Instant,
    ended_at: Option<Instant>,
}

fn lock(m: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Inner {
    /// Pasa a `state` si seguía corriendo. `true` si cambió.
    fn end(&mut self, id: ProcessId, state: ProcessState) -> bool {
        let Some(e) = self.entries.get_mut(&id) else {
            return false;
        };
        if !e.info.is_running() {
            return false;
        }
        self.rev += 1;
        e.rev = self.rev;
        e.info.state = state;
        e.info.duration = Some(u64::try_from(e.started.elapsed().as_millis()).unwrap_or(u64::MAX));
        e.ended_at = Some(Instant::now());
        true
    }
}

impl Table {
    pub fn new() -> Self {
        Table {
            next: AtomicU32::new(1),
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn next_id(&self) -> ProcessId {
        ProcessId(self.next.fetch_add(1, Ordering::Relaxed))
    }

    /// Registra el proceso si `open()` (consultado con el candado de la tabla) sigue
    /// diciendo que el túnel está abierto: quien cierra el túnel marca primero que está
    /// cerrado y después mata lo registrado, así que ningún proceso se escapa en medio.
    pub fn insert(
        &self,
        info: TunnelProcess,
        child: Arc<dyn Child>,
        open: impl FnOnce() -> bool,
    ) -> bool {
        let mut t = lock(&self.inner);
        if !open() {
            return false;
        }
        t.rev += 1;
        let rev = t.rev;
        t.entries.insert(
            info.id,
            Entry {
                info,
                rev,
                child,
                started: Instant::now(),
                ended_at: None,
            },
        );
        true
    }

    /// El proceso terminó solo. `false` si ya estaba terminado (lo mató `kill`).
    pub fn exited(&self, id: ProcessId, code: Option<i32>) -> bool {
        lock(&self.inner).end(id, ProcessState::Exited { code })
    }

    /// Mata el árbol de un proceso. `None` si no existe; `Some(false)` si ya había terminado.
    pub fn kill(&self, id: ProcessId) -> Option<bool> {
        let (child, was_running) = {
            let mut t = lock(&self.inner);
            let child = t.entries.get(&id)?.child.clone();
            (child, t.end(id, ProcessState::Killed))
        };
        // Aun si ya terminó, el árbol puede tener descendientes vivos.
        child.kill_tree();
        Some(was_running)
    }

    /// Mata todos los árboles (cierre del túnel).
    pub fn kill_all(&self) {
        let children: Vec<Arc<dyn Child>> = {
            let mut t = lock(&self.inner);
            let ids: Vec<ProcessId> = t.entries.keys().copied().collect();
            for id in ids {
                t.end(id, ProcessState::Killed);
            }
            t.entries.values().map(|e| e.child.clone()).collect()
        };
        for c in children {
            c.kill_tree();
        }
    }

    pub fn get(&self, id: ProcessId) -> Option<TunnelProcess> {
        lock(&self.inner).entries.get(&id).map(|e| e.info.clone())
    }

    pub fn list(&self) -> Vec<TunnelProcess> {
        lock(&self.inner)
            .entries
            .values()
            .map(|e| e.info.clone())
            .collect()
    }

    pub fn running(&self) -> Vec<TunnelProcess> {
        let mut v: Vec<TunnelProcess> = lock(&self.inner)
            .entries
            .values()
            .filter(|e| e.info.is_running())
            .map(|e| e.info.clone())
            .collect();
        v.sort_by_key(|p| (p.started, p.id));
        v
    }

    /// Los procesos que cambiaron desde lo que ya mandó `sent` (y lo anota).
    pub fn updates(&self, sent: &mut BTreeMap<ProcessId, u64>) -> Vec<TunnelProcess> {
        let t = lock(&self.inner);
        let mut out = Vec::new();
        for (id, e) in &t.entries {
            if sent.get(id).is_none_or(|rev| *rev < e.rev) {
                sent.insert(*id, e.rev);
                out.push(e.info.clone());
            }
        }
        out
    }

    /// Olvida los terminados hace más de `linger` y devuelve cuáles (para soltar sus flujos).
    pub fn purge(&self, linger: Duration) -> Vec<TunnelProcess> {
        let mut t = lock(&self.inner);
        let gone: Vec<ProcessId> = t
            .entries
            .iter()
            .filter(|(_, e)| e.ended_at.is_some_and(|at| at.elapsed() >= linger))
            .map(|(id, _)| *id)
            .collect();
        gone.iter()
            .filter_map(|id| t.entries.remove(id))
            .map(|e| e.info)
            .collect()
    }
}
