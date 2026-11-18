//! The processes of a tunnel on the destination: their state, their child and when they are forgotten.

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
    /// Rises with each change; each entry keeps that of its last change.
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
    /// Moves to `state` if it was still running. `true` if it changed.
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

    /// Registers the process if `open()` (queried with the table's lock) still
    /// says the tunnel is open: whoever closes the tunnel first marks it as
    /// closed and then kills what is registered, so no process escapes in between.
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

    /// The process ended on its own. `false` if it was already ended (killed by `kill`).
    pub fn exited(&self, id: ProcessId, code: Option<i32>) -> bool {
        lock(&self.inner).end(id, ProcessState::Exited { code })
    }

    /// Kills a process's tree. `None` if it does not exist; `Some(false)` if it had already ended.
    pub fn kill(&self, id: ProcessId) -> Option<bool> {
        let (child, was_running) = {
            let mut t = lock(&self.inner);
            let child = t.entries.get(&id)?.child.clone();
            (child, t.end(id, ProcessState::Killed))
        };
        // Even if it already ended, the tree may have live descendants.
        child.kill_tree();
        Some(was_running)
    }

    /// Kills all the trees (tunnel close).
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

    /// The processes that changed since what `sent` already sent (and notes it).
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

    /// Forgets those ended more than `linger` ago and returns which (to release their flows).
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
