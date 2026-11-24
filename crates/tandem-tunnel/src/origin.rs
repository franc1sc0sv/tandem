//! The origin machine: opens and closes tunnels on the other and follows their output.
//!
//! For each open tunnel a follower runs that keeps the data channel
//! ([`TunnelUp::Attach`]) over the active link. If the stream is cut, or the active link
//! moves to another path, it asks again for the output "from byte N" of each flow, with N what
//! it already has in its mirror: nothing is lost or repeated (what arrives repeated is
//! discarded in [`OutputBuffer::accept`]).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tandem_core::Machine;
use tandem_proto::{
    Client, ExecSpec, FlowId, FlowInfo, OutputCursor, ProcessId, ProcessOutput, Project, PtySize,
    Tunnel, TunnelDown, TunnelId, TunnelProcess, TunnelState, TunnelUp, TunnelView,
};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::Changes;
use crate::buffer::{DEFAULT_CAPACITY, OutputBuffer, Read};
use crate::process::output::{self, FlowRead};
use crate::wire::{Dialer, WireError};

/// The link used to talk to the other machine; `None` if none sees it.
pub type Link = Option<Arc<dyn Dialer>>;

#[derive(Debug, Clone, Copy)]
pub struct OriginOptions {
    /// What each flow's mirror keeps.
    pub capacity: usize,
    /// Cap for open, close or list.
    pub request_timeout: Duration,
    /// Wait before retrying the data channel over the same link.
    pub retry: Duration,
    /// How long a closed tunnel is remembered (and its output, to read it). It is also how long
    /// closing one whose `Open` went unanswered is insisted on.
    pub linger: Duration,
}

impl Default for OriginOptions {
    fn default() -> Self {
        OriginOptions {
            capacity: DEFAULT_CAPACITY,
            request_timeout: Duration::from_secs(10),
            retry: Duration::from_millis(500),
            linger: Duration::from_secs(10 * 60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TunnelError {
    #[error("the other machine is not connected")]
    NoLink,
    #[error("the other machine did not answer in time")]
    Timeout,
    #[error("the link to the other machine was cut: {0}")]
    Wire(#[from] WireError),
    #[error("the other machine did not accept it: {0}")]
    Refused(String),
    #[error("unexpected response from the other machine")]
    Unexpected,
    #[error("a tunnel's destination has to be the other machine ({0})")]
    SameMachine(Machine),
    #[error("no tunnel {0} is open from this machine")]
    Unknown(TunnelId),
    #[error("tunnel {0} does not have process {1}")]
    UnknownProcess(TunnelId, ProcessId),
    #[error("could not pick a random id: {0}")]
    Entropy(String),
    #[error("process {1} of tunnel {0} does not run in a terminal: it takes no input")]
    NoTerminal(TunnelId, ProcessId),
}

/// The most input that goes in one message: far below a frame's maximum.
const INPUT_CHUNK: usize = 64 * 1024;

/// What the client asks for when opening.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSpec {
    pub title: String,
    pub project: Option<Project>,
    /// This machine.
    pub origin: Machine,
    pub target: Machine,
    pub opened_by: Client,
}

/// What is known about a flow at the origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowStatus {
    pub info: FlowInfo,
    /// Bytes received (the position of the next one).
    pub received: u64,
    /// Bytes that were lost because the destination's buffer filled up without being read.
    pub lost: u64,
    /// The total, if the flow already ended and everything arrived.
    pub ended: Option<u64>,
}

/// The tunnels opened from this machine. Cheap to clone; when the last is dropped, its
/// followers stop (the tunnels stay alive on the destination).
#[derive(Clone)]
pub struct Origin {
    inner: Arc<OriginInner>,
}

struct OriginInner {
    opts: OriginOptions,
    link: watch::Sender<Link>,
    tunnels: Mutex<BTreeMap<TunnelId, Entry>>,
    /// Tunnel changes (open, close), for the UI.
    changes: Changes,
    /// Rises with each byte received, for whoever reads the live output.
    output: watch::Sender<u64>,
}

struct Entry {
    info: Tunnel,
    flows: BTreeMap<FlowId, Mirror>,
    /// The processes the destination reported, also those ended less than `linger` ago.
    processes: BTreeMap<ProcessId, Proc>,
    follower: Option<JoinHandle<()>>,
    /// Since when it has been closed; after `linger` it is forgotten.
    closed_at: Option<Instant>,
    /// What is typed into each terminal process, on its way to the destination.
    stdin: BTreeMap<ProcessId, Arc<InputQueue>>,
}

/// A terminal process's input on the origin: bytes queue up here and one task sends them in
/// order, each with its position, until the destination acknowledges them. A cut or a link change
/// only delays them: the resend carries the same position and the destination drops what it
/// already had.
struct InputQueue {
    state: Mutex<InputState>,
    wake: Notify,
}

#[derive(Default)]
struct InputState {
    /// Position of `queued[0]` in the process's input.
    base: u64,
    queued: Vec<u8>,
    /// No more will be sent (tunnel closed, process gone).
    dead: bool,
}

struct Proc {
    info: TunnelProcess,
    /// Since when it is known to have ended; after `linger` it is forgotten with its flows.
    ended_at: Option<Instant>,
}

struct Mirror {
    info: FlowInfo,
    buf: OutputBuffer,
    /// What the destination reported as lost (does not include what the mirror dropped for lack of room).
    lost: u64,
    ended: Option<u64>,
}

impl Drop for OriginInner {
    fn drop(&mut self) {
        for e in lock(&self.tunnels).values() {
            if let Some(f) = &e.follower {
                f.abort();
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

impl Origin {
    pub fn new(opts: OriginOptions, changes: Changes) -> Self {
        Origin {
            inner: Arc::new(OriginInner {
                opts,
                link: watch::Sender::new(None),
                tunnels: Mutex::new(BTreeMap::new()),
                changes,
                output: watch::Sender::new(0),
            }),
        }
    }

    /// The active link changed. The same `Arc` does not count as a change.
    pub fn set_link(&self, link: Link) {
        self.inner.link.send_if_modified(|old| {
            let same = match (&*old, &link) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            if !same {
                *old = link;
            }
            !same
        });
    }

    /// Opens a tunnel at `spec.target` and starts following its output.
    pub async fn open(&self, spec: OpenSpec) -> Result<Tunnel, TunnelError> {
        if spec.target != spec.origin.other() {
            return Err(TunnelError::SameMachine(spec.target));
        }
        let id = getrandom::u64().map_err(|e| TunnelError::Entropy(e.to_string()))?;
        let tunnel = Tunnel {
            id: TunnelId(id),
            title: spec.title,
            project: spec.project,
            origin: spec.origin,
            target: spec.target,
            opened_by: spec.opened_by,
            state: TunnelState::Open,
            created: now_millis(),
        };
        let id = tunnel.id;
        let opened = match self.inner.request(TunnelUp::Open(tunnel)).await {
            Ok(TunnelDown::Opened(t)) => t,
            Ok(_) => return Err(TunnelError::Unexpected),
            // The request may have arrived and the response been lost: the destination would have a tunnel that
            // nobody follows or closes. It is closed in the background when there is a link (closing
            // one that does not exist does nothing).
            Err(e @ (TunnelError::Timeout | TunnelError::Wire(_))) => {
                tokio::spawn(discard(Arc::downgrade(&self.inner), id));
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        // Registry first: the follower starts by reading what is already held.
        self.inner.tunnels().insert(
            opened.id,
            Entry {
                info: opened.clone(),
                flows: BTreeMap::new(),
                processes: BTreeMap::new(),
                follower: None,
                closed_at: None,
                stdin: BTreeMap::new(),
            },
        );
        let follower = tokio::spawn(follow(Arc::downgrade(&self.inner), opened.id));
        if let Some(e) = lock(&self.inner.tunnels).get_mut(&opened.id) {
            e.follower = Some(follower);
        }
        self.inner.changes.bump();
        Ok(opened)
    }

    /// Closes the tunnel on the destination (with its resources). The output that was missing keeps
    /// arriving until the close.
    pub async fn close(&self, id: TunnelId) -> Result<(), TunnelError> {
        if !self.inner.tunnels().contains_key(&id) {
            return Err(TunnelError::Unknown(id));
        }
        match self.inner.request(TunnelUp::Close(id)).await? {
            TunnelDown::Closed(_) => {
                // The follower's close notice arrives over another flow and may come after
                // this response: it is marked here so `tunnels()` already sees it closed.
                self.inner.mark_closed(id);
                Ok(())
            }
            _ => Err(TunnelError::Unexpected),
        }
    }

    /// Opens a tunnel stream over the active link, for operations that carry their own
    /// dialog (files and ports, T-106).
    pub async fn dial(&self) -> Result<Box<dyn crate::OriginWire>, TunnelError> {
        let dialer = self
            .inner
            .link
            .borrow()
            .clone()
            .ok_or(TunnelError::NoLink)?;
        let wire = tokio::time::timeout(self.inner.opts.request_timeout, dialer.dial())
            .await
            .map_err(|_| TunnelError::Timeout)??;
        Ok(wire)
    }

    /// `true` if the tunnel is open and this machine opened it.
    pub fn is_open(&self, id: TunnelId) -> bool {
        self.inner
            .tunnels()
            .get(&id)
            .is_some_and(|e| e.info.state == TunnelState::Open)
    }

    /// The destination's tunnels, according to it.
    pub async fn remote_list(&self) -> Result<Vec<Tunnel>, TunnelError> {
        match self.inner.request(TunnelUp::List).await? {
            TunnelDown::List(list) => Ok(list),
            _ => Err(TunnelError::Unexpected),
        }
    }

    /// Those opened from here (also those already closed).
    pub fn tunnels(&self) -> Vec<Tunnel> {
        self.inner
            .tunnels()
            .values()
            .map(|e| e.info.clone())
            .collect()
    }

    /// Launches a process in tunnel `id` of the other machine (T-104). Processes belong to
    /// that tunnel: closing it kills them.
    pub async fn exec(&self, id: TunnelId, spec: ExecSpec) -> Result<TunnelProcess, TunnelError> {
        if !self.inner.tunnels().contains_key(&id) {
            return Err(TunnelError::Unknown(id));
        }
        match self
            .inner
            .request(TunnelUp::Exec { tunnel: id, spec })
            .await?
        {
            TunnelDown::Started(p) => {
                self.inner.note_process(id, &p);
                Ok(p)
            }
            _ => Err(TunnelError::Unexpected),
        }
    }

    /// Kills a process of the tunnel with its whole tree. Its new state arrives over the data
    /// channel.
    pub async fn kill(&self, id: TunnelId, process: ProcessId) -> Result<(), TunnelError> {
        if !self.inner.tunnels().contains_key(&id) {
            return Err(TunnelError::Unknown(id));
        }
        match self
            .inner
            .request(TunnelUp::Kill {
                tunnel: id,
                process,
            })
            .await?
        {
            TunnelDown::Killed(_) => Ok(()),
            _ => Err(TunnelError::Unexpected),
        }
    }

    /// Types `bytes` into a terminal process of the other machine. Returns once queued: the
    /// bytes arrive in order and exactly once, over whichever link is active, also after cuts.
    pub fn input(&self, id: TunnelId, process: ProcessId, bytes: &[u8]) -> Result<(), TunnelError> {
        let queue = {
            let mut tunnels = self.inner.tunnels();
            let e = tunnels.get_mut(&id).ok_or(TunnelError::Unknown(id))?;
            let p = e
                .processes
                .get(&process)
                .ok_or(TunnelError::UnknownProcess(id, process))?;
            if !p.info.pty {
                return Err(TunnelError::NoTerminal(id, process));
            }
            if !p.info.is_running() {
                // Like a terminal whose program ended: what is typed goes nowhere.
                return Ok(());
            }
            match e.stdin.get(&process) {
                Some(q) => q.clone(),
                None => {
                    let q = Arc::new(InputQueue {
                        state: Mutex::new(InputState::default()),
                        wake: Notify::new(),
                    });
                    e.stdin.insert(process, q.clone());
                    tokio::spawn(send_input(
                        Arc::downgrade(&self.inner),
                        id,
                        process,
                        q.clone(),
                    ));
                    q
                }
            }
        };
        lock(&queue.state).queued.extend_from_slice(bytes);
        queue.wake.notify_one();
        Ok(())
    }

    /// Bytes typed into a terminal process that the other machine has not acknowledged yet.
    pub fn input_pending(&self, id: TunnelId, process: ProcessId) -> usize {
        self.inner
            .tunnels()
            .get(&id)
            .and_then(|e| e.stdin.get(&process))
            .map_or(0, |q| lock(&q.state).queued.len())
    }

    /// Changes the size of a terminal process's terminal.
    pub async fn resize(
        &self,
        id: TunnelId,
        process: ProcessId,
        size: PtySize,
    ) -> Result<(), TunnelError> {
        if !self.inner.tunnels().contains_key(&id) {
            return Err(TunnelError::Unknown(id));
        }
        match self
            .inner
            .request(TunnelUp::Resize {
                tunnel: id,
                process,
                size,
            })
            .await?
        {
            TunnelDown::Resized(_) => Ok(()),
            _ => Err(TunnelError::Unexpected),
        }
    }

    /// The tunnel's processes known here (running and ended recently), by
    /// id. `None` if the tunnel was not opened from this machine.
    pub fn processes(&self, id: TunnelId) -> Option<Vec<TunnelProcess>> {
        self.inner
            .tunnels()
            .get(&id)
            .map(|e| e.processes.values().map(|p| p.info.clone()).collect())
    }

    /// The output of a process from `from`, with up to `max` bytes of each flow (what
    /// T-104 exposes as `output`). Without waiting: what already arrived.
    pub fn output(
        &self,
        id: TunnelId,
        process: ProcessId,
        from: OutputCursor,
        max: usize,
    ) -> Result<ProcessOutput, TunnelError> {
        let tunnels = self.inner.tunnels();
        let e = tunnels.get(&id).ok_or(TunnelError::Unknown(id))?;
        let p = e
            .processes
            .get(&process)
            .ok_or(TunnelError::UnknownProcess(id, process))?;
        Ok(output::collect(
            p.info.clone(),
            from,
            max,
            |flow, at, max| {
                let m = e.flows.get(&flow)?;
                Some(FlowRead {
                    read: m.buf.read(at, max),
                    end: m.buf.end(),
                    ended: m.ended.is_some(),
                })
            },
        ))
    }

    /// The tunnels opened from here, with their processes running, for the UI.
    pub fn views(&self) -> Vec<TunnelView> {
        self.inner
            .tunnels()
            .values()
            .map(|e| {
                let mut running: Vec<TunnelProcess> = e
                    .processes
                    .values()
                    .filter(|p| p.info.is_running())
                    .map(|p| p.info.clone())
                    .collect();
                running.sort_by_key(|p| (p.started, p.id));
                TunnelView {
                    tunnel: e.info.clone(),
                    running,
                }
            })
            .collect()
    }

    pub fn flows(&self, id: TunnelId) -> Vec<FlowStatus> {
        self.inner
            .tunnels()
            .get(&id)
            .map(|e| {
                e.flows
                    .values()
                    .map(|m| FlowStatus {
                        info: m.info.clone(),
                        received: m.buf.end(),
                        lost: m.lost,
                        ended: m.ended,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The output received from a flow from `from` (what T-104 exposes as `output`).
    pub fn read(&self, id: TunnelId, flow: FlowId, from: u64, max: usize) -> Option<Read> {
        self.inner
            .tunnels()
            .get(&id)?
            .flows
            .get(&flow)
            .map(|m| m.buf.read(from, max))
    }

    /// Rises with each byte received from any tunnel.
    pub fn subscribe_output(&self) -> watch::Receiver<u64> {
        self.inner.output.subscribe()
    }
}

impl OriginInner {
    /// The tunnels, without those closed more than `linger` ago.
    /// Marks the tunnel as closed locally (idempotent).
    fn mark_closed(&self, id: TunnelId) {
        let mut tunnels = lock(&self.tunnels);
        let Some(e) = tunnels.get_mut(&id) else {
            return;
        };
        if e.info.state == TunnelState::Closed {
            return;
        }
        e.info.state = TunnelState::Closed;
        e.closed_at = Some(Instant::now());
        stop_input(e);
        drop(tunnels);
        self.changes.bump();
        self.output.send_modify(|n| *n = n.wrapping_add(1));
    }

    fn tunnels(&self) -> MutexGuard<'_, BTreeMap<TunnelId, Entry>> {
        let mut tunnels = lock(&self.tunnels);
        let linger = self.opts.linger;
        tunnels.retain(|_, e| e.closed_at.is_none_or(|at| at.elapsed() < linger));
        for e in tunnels.values_mut() {
            forget_old_processes(e, linger);
        }
        tunnels
    }

    /// Records a process the destination reported (the `exec` response or the data channel).
    fn note_process(&self, id: TunnelId, p: &TunnelProcess) {
        let changed = lock(&self.tunnels)
            .get_mut(&id)
            .is_some_and(|e| upsert_process(e, p));
        if changed {
            self.changes.bump();
        }
    }

    async fn request(&self, up: TunnelUp) -> Result<TunnelDown, TunnelError> {
        let dialer = self.link.borrow().clone().ok_or(TunnelError::NoLink)?;
        let ask = async {
            let mut wire = dialer.dial().await?;
            wire.send(up).await?;
            wire.recv().await?.ok_or_else(|| {
                WireError::new("the other machine closed the stream without answering")
            })
        };
        match tokio::time::timeout(self.opts.request_timeout, ask).await {
            Err(_) => Err(TunnelError::Timeout),
            Ok(Err(e)) => Err(TunnelError::Wire(e)),
            Ok(Ok(TunnelDown::Refused(why))) => Err(TunnelError::Refused(why)),
            Ok(Ok(reply)) => Ok(reply),
        }
    }

    /// What is already held of each flow; `None` if the tunnel is no longer followed.
    fn cursors(&self, id: TunnelId) -> Option<Vec<(FlowId, u64)>> {
        let tunnels = lock(&self.tunnels);
        let e = tunnels.get(&id)?;
        (e.info.state == TunnelState::Open)
            .then(|| e.flows.iter().map(|(f, m)| (*f, m.buf.end())).collect())
    }

    /// Applies a data channel message. `true` if the tunnel ended.
    fn apply(&self, id: TunnelId, msg: TunnelDown) -> bool {
        let mut tunnels = lock(&self.tunnels);
        let Some(e) = tunnels.get_mut(&id) else {
            return true;
        };
        let capacity = self.opts.capacity;
        match msg {
            TunnelDown::Flow(info) => {
                let id = info.id;
                mirror(e, id, capacity).info = info;
            }
            TunnelDown::Data {
                flow,
                offset,
                bytes,
            } => {
                let m = mirror(e, flow, capacity);
                if let Err(gap) = m.buf.accept(offset, &bytes) {
                    // Should not happen: it is always requested from what is held.
                    tracing::warn!(tunnel = %id, "{gap}");
                    m.lost += gap.offset - gap.at;
                    m.buf.skip_to(gap.offset);
                    let _ = m.buf.accept(offset, &bytes);
                }
                drop(tunnels);
                self.output.send_modify(|n| *n = n.wrapping_add(1));
                return false;
            }
            TunnelDown::Lost { flow, from, to } => {
                let m = mirror(e, flow, capacity);
                let missing = to.saturating_sub(m.buf.end().max(from));
                if missing > 0 {
                    tracing::warn!(
                        tunnel = %id,
                        flow = %m.info.label,
                        "{missing} output bytes were lost: the other machine's buffer filled up"
                    );
                    m.lost += missing;
                    m.buf.skip_to(to);
                }
            }
            TunnelDown::End { flow, total } => mirror(e, flow, capacity).ended = Some(total),
            TunnelDown::Process(p) => {
                if !p.is_running()
                    && let Some(q) = e.stdin.remove(&p.id)
                {
                    q.stop();
                }
                let changed = upsert_process(e, &p);
                drop(tunnels);
                if changed {
                    self.changes.bump();
                }
                self.output.send_modify(|n| *n = n.wrapping_add(1));
                return false;
            }
            TunnelDown::TunnelClosed | TunnelDown::Refused(_) => {
                if let TunnelDown::Refused(why) = &msg {
                    tracing::warn!(tunnel = %id, "the other machine no longer has the tunnel: {why}");
                }
                e.info.state = TunnelState::Closed;
                e.follower = None;
                e.closed_at = Some(Instant::now());
                stop_input(e);
                drop(tunnels);
                self.changes.bump();
                self.output.send_modify(|n| *n = n.wrapping_add(1));
                return true;
            }
            TunnelDown::Opened(_)
            | TunnelDown::Closed(_)
            | TunnelDown::List(_)
            | TunnelDown::Files(_)
            | TunnelDown::Port(_)
            | TunnelDown::Workspace(_)
            | TunnelDown::Started(_)
            | TunnelDown::Killed(_)
            | TunnelDown::InputAck { .. }
            | TunnelDown::Resized(_) => {}
        }
        drop(tunnels);
        self.output.send_modify(|n| *n = n.wrapping_add(1));
        false
    }
}

/// Stores what the destination reported about a process. An old notice (running) does not override a
/// new one (ended): the `exec` response and the data channel can arrive in any
/// order. `true` if something changed.
fn upsert_process(e: &mut Entry, p: &TunnelProcess) -> bool {
    if let Some(old) = e.processes.get_mut(&p.id) {
        if old.info == *p || (!old.info.is_running() && p.is_running()) {
            return false;
        }
        old.info = p.clone();
        if !p.is_running() && old.ended_at.is_none() {
            old.ended_at = Some(Instant::now());
        }
        return true;
    }
    e.processes.insert(
        p.id,
        Proc {
            info: p.clone(),
            ended_at: (!p.is_running()).then(Instant::now),
        },
    );
    true
}

/// Forgets processes ended more than `linger` ago, with their flows.
fn forget_old_processes(e: &mut Entry, linger: Duration) {
    let gone: Vec<TunnelProcess> = e
        .processes
        .values()
        .filter(|p| p.ended_at.is_some_and(|at| at.elapsed() >= linger))
        .map(|p| p.info.clone())
        .collect();
    for p in gone {
        if let Some(q) = e.stdin.remove(&p.id) {
            q.stop();
        }
        e.processes.remove(&p.id);
        e.flows.remove(&p.stdout);
        e.flows.remove(&p.stderr);
    }
}

impl InputQueue {
    fn stop(&self) {
        let mut s = lock(&self.state);
        s.dead = true;
        s.queued.clear();
        drop(s);
        self.wake.notify_one();
    }
}

fn stop_input(e: &mut Entry) {
    for q in std::mem::take(&mut e.stdin).into_values() {
        q.stop();
    }
}

/// Sends `queue` to the destination in order until it is stopped or nobody is left.
async fn send_input(
    origin: Weak<OriginInner>,
    id: TunnelId,
    process: ProcessId,
    queue: Arc<InputQueue>,
) {
    let Some((mut links, retry)) = origin.upgrade().map(|o| (o.link.subscribe(), o.opts.retry))
    else {
        return;
    };
    loop {
        let next = {
            let s = lock(&queue.state);
            if s.dead {
                return;
            }
            (!s.queued.is_empty()).then(|| {
                let n = s.queued.len().min(INPUT_CHUNK);
                (s.base, s.queued[..n].to_vec())
            })
        };
        let Some((offset, bytes)) = next else {
            queue.wake.notified().await;
            continue;
        };
        let Some(o) = origin.upgrade() else {
            return;
        };
        let reply = o
            .request(TunnelUp::Input {
                tunnel: id,
                process,
                offset,
                bytes,
            })
            .await;
        drop(o);
        match reply {
            Ok(TunnelDown::InputAck { next, .. }) => {
                let mut s = lock(&queue.state);
                let done = usize::try_from(next.saturating_sub(s.base))
                    .unwrap_or(usize::MAX)
                    .min(s.queued.len());
                s.queued.drain(..done);
                s.base += done as u64;
            }
            Ok(_) | Err(TunnelError::Refused(_) | TunnelError::Unexpected) => {
                // The process (or its terminal) is gone on the other machine: nothing will take it.
                if let Err(e) = &reply {
                    tracing::debug!(tunnel = %id, %process, "input not delivered: {e}");
                }
                queue.stop();
                return;
            }
            Err(e) => {
                tracing::debug!(tunnel = %id, %process, "input will be resent: {e}");
                links.borrow_and_update();
                tokio::select! {
                    () = tokio::time::sleep(retry) => {}
                    r = links.changed() => if r.is_err() { return },
                }
            }
        }
    }
}

fn mirror(e: &mut Entry, flow: FlowId, capacity: usize) -> &mut Mirror {
    e.flows.entry(flow).or_insert_with(|| Mirror {
        info: FlowInfo {
            id: flow,
            label: String::new(),
        },
        buf: OutputBuffer::new(capacity),
        lost: 0,
        ended: None,
    })
}

enum Attach {
    /// The tunnel ended (or there is no one left to notify).
    Done,
    /// The active link changed: it is requested again over the new one, without waiting.
    LinkChanged,
    Failed(WireError),
}

/// Keeps `id`'s data channel over the active link until the tunnel closes.
async fn follow(origin: Weak<OriginInner>, id: TunnelId) {
    let (mut links, retry) = match origin.upgrade() {
        Some(o) => (o.link.subscribe(), o.opts.retry),
        None => return,
    };
    loop {
        let link = links.borrow_and_update().clone();
        let Some(dialer) = link else {
            if links.changed().await.is_err() {
                return;
            }
            continue;
        };
        match attach(&origin, id, dialer.as_ref(), &mut links).await {
            Attach::Done => return,
            Attach::LinkChanged => continue,
            Attach::Failed(e) => {
                tracing::debug!(tunnel = %id, "tunnel data channel cut: {e}");
                tokio::select! {
                    () = tokio::time::sleep(retry) => {}
                    r = links.changed() => if r.is_err() { return },
                }
            }
        }
    }
}

async fn attach(
    origin: &Weak<OriginInner>,
    id: TunnelId,
    dialer: &dyn Dialer,
    links: &mut watch::Receiver<Link>,
) -> Attach {
    let Some(from) = origin.upgrade().and_then(|o| o.cursors(id)) else {
        return Attach::Done;
    };
    let session = async {
        let mut wire = dialer.dial().await?;
        wire.send(TunnelUp::Attach { tunnel: id, from }).await?;
        loop {
            let Some(msg) = wire.recv().await? else {
                return Err(WireError::new("the other machine closed the data channel"));
            };
            let Some(o) = origin.upgrade() else {
                return Ok(());
            };
            if o.apply(id, msg) {
                return Ok(());
            }
        }
    };
    tokio::select! {
        r = session => match r {
            Ok(()) => Attach::Done,
            Err(e) => Attach::Failed(e),
        },
        r = links.changed() => match r {
            Ok(()) => Attach::LinkChanged,
            Err(_) => Attach::Done,
        },
    }
}

/// Closes on the destination a tunnel whose `Open` went unanswered: tries on each
/// link that appears until the destination answers, or until `linger`.
async fn discard(origin: Weak<OriginInner>, id: TunnelId) {
    let Some((mut links, retry, linger)) = origin
        .upgrade()
        .map(|o| (o.link.subscribe(), o.opts.retry, o.opts.linger))
    else {
        return;
    };
    let give_up = Instant::now() + linger;
    while Instant::now() < give_up {
        links.borrow_and_update();
        let Some(o) = origin.upgrade() else {
            return;
        };
        match o.request(TunnelUp::Close(id)).await {
            // Closed, or it never got opened.
            Ok(_) | Err(TunnelError::Refused(_)) => {
                tracing::debug!(tunnel = %id, "tunnel without a response on open: closed on the other machine");
                return;
            }
            Err(e) => {
                tracing::debug!(tunnel = %id, "could not close the unanswered tunnel: {e}")
            }
        }
        drop(o);
        tokio::select! {
            () = tokio::time::sleep(retry) => {}
            r = links.changed() => if r.is_err() { return },
        }
    }
    tracing::warn!(tunnel = %id, "could not close on the other machine a tunnel left unanswered on open");
}
