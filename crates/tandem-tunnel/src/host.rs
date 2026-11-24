//! The destination machine: tunnels live here, with their flows and resources, even if the link
//! to the origin drops.
//!
//! What runs inside a tunnel (processes in T-104, ports in T-106, workspaces in
//! T-107) is registered in its [`HostTunnel`]: the output as a flow ([`FlowWriter`]) and what
//! has to be released on close as a [`Resource`]. Processes ([`HostTunnel::exec`])
//! also carry their own table, which the tunnel closes by killing each tree.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tandem_proto::{
    ExecSpec, FlowId, FlowInfo, OutputCursor, ProcessId, ProcessOutput, PtySize, Tunnel,
    TunnelDown, TunnelId, TunnelProcess, TunnelState, TunnelUp, TunnelView,
};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::Changes;
use crate::buffer::{DEFAULT_CAPACITY, OutputBuffer, Read};
use crate::process::output::{self, FlowRead};
use crate::process::system::SystemSpawner;
use crate::process::table::Table;
use crate::process::{ExecError, Launch, Os, Spawner, Terminal, launch};
use crate::wire::{TargetWire, WireError};

/// The most that goes in a data message: far below a frame's maximum, and
/// small enough to alternate between flows.
pub const CHUNK: usize = 64 * 1024;

/// Something of the tunnel that has to be released on close (kill a process tree, close a
/// port). `close` has to return right away: slow work goes in its own task.
pub trait Resource: Send + Sync {
    fn close(&self);
}

#[derive(Debug, Clone, Copy)]
pub struct HostOptions {
    /// What each flow keeps while nobody reads it.
    pub capacity: usize,
    /// How long a closed tunnel is remembered, so an origin that reconnects receives
    /// what was missing and the close.
    pub linger: Duration,
}

impl Default for HostOptions {
    fn default() -> Self {
        HostOptions {
            capacity: DEFAULT_CAPACITY,
            linger: Duration::from_secs(10 * 60),
        }
    }
}

/// The tunnels of which this machine is the destination. Cheap to clone.
#[derive(Clone)]
pub struct Host {
    inner: Arc<HostInner>,
}

struct HostInner {
    opts: HostOptions,
    tunnels: Mutex<BTreeMap<TunnelId, HostTunnel>>,
    changes: Changes,
    /// With which processes are launched: the real one, except in tests.
    spawner: Mutex<Arc<dyn Spawner>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Host {
    pub fn new(opts: HostOptions, changes: Changes) -> Self {
        Host {
            inner: Arc::new(HostInner {
                opts,
                tunnels: Mutex::new(BTreeMap::new()),
                changes,
                spawner: Mutex::new(Arc::new(SystemSpawner)),
            }),
        }
    }

    /// Changes how processes are launched (tests use a double).
    pub fn set_spawner(&self, spawner: Arc<dyn Spawner>) {
        *lock(&self.inner.spawner) = spawner;
    }

    fn tunnel(&self, id: TunnelId) -> Result<HostTunnel, ExecError> {
        self.get(id).ok_or(ExecError::UnknownTunnel(id))
    }

    /// Launches a process in tunnel `id` ([`HostTunnel::exec`]).
    pub fn exec(&self, id: TunnelId, spec: ExecSpec) -> Result<TunnelProcess, ExecError> {
        let spawner = lock(&self.inner.spawner).clone();
        self.tunnel(id)?.exec(spawner.as_ref(), spec)
    }

    /// Kills a process of the tunnel with its whole tree.
    pub fn kill(&self, id: TunnelId, process: ProcessId) -> Result<(), ExecError> {
        self.tunnel(id)?.kill(process)
    }

    /// Input for a terminal process ([`HostTunnel::input`]).
    pub fn input(
        &self,
        id: TunnelId,
        process: ProcessId,
        offset: u64,
        bytes: &[u8],
    ) -> Result<u64, ExecError> {
        self.tunnel(id)?.input(process, offset, bytes)
    }

    pub fn resize(&self, id: TunnelId, process: ProcessId, size: PtySize) -> Result<(), ExecError> {
        self.tunnel(id)?.resize(process, size)
    }

    /// The open tunnels with their processes running, for the UI.
    pub fn views(&self) -> Vec<TunnelView> {
        self.list()
            .into_iter()
            .filter(|t| t.state == TunnelState::Open)
            .map(|tunnel| {
                let running = self.get(tunnel.id).map(|t| t.running()).unwrap_or_default();
                TunnelView { tunnel, running }
            })
            .collect()
    }

    /// Opens the tunnel; if one with that id already exists (a retry) returns that one.
    pub fn open(&self, tunnel: Tunnel) -> Tunnel {
        let mut tunnels = lock(&self.inner.tunnels);
        self.purge(&mut tunnels);
        if let Some(t) = tunnels.get(&tunnel.id) {
            return t.info();
        }
        let mut info = tunnel;
        info.state = TunnelState::Open;
        tracing::info!(id = %info.id, title = %info.title, "tunnel opened from {}", info.origin);
        tunnels.insert(
            info.id,
            HostTunnel::new(info.clone(), self.inner.opts, self.inner.changes.clone()),
        );
        drop(tunnels);
        self.inner.changes.bump();
        info
    }

    /// Closes the tunnel: releases its resources and ends its flows. Closing one already closed
    /// does nothing; an unknown one is an error.
    pub fn close(&self, id: TunnelId) -> Result<(), String> {
        let t = self.get(id).ok_or_else(|| unknown(id))?;
        if t.close() {
            tracing::info!(%id, "tunnel closed");
            self.inner.changes.bump();
        }
        Ok(())
    }

    /// All of them, also the closed ones that are still remembered.
    pub fn list(&self) -> Vec<Tunnel> {
        let mut tunnels = lock(&self.inner.tunnels);
        self.purge(&mut tunnels);
        tunnels.values().map(HostTunnel::info).collect()
    }

    pub fn get(&self, id: TunnelId) -> Option<HostTunnel> {
        lock(&self.inner.tunnels).get(&id).cloned()
    }

    fn purge(&self, tunnels: &mut BTreeMap<TunnelId, HostTunnel>) {
        let linger = self.inner.opts.linger;
        tunnels.retain(|_, t| t.closed_at().is_none_or(|at| at.elapsed() < linger));
    }
}

fn unknown(id: TunnelId) -> String {
    format!("no tunnel {id} on this machine")
}

/// A tunnel on the destination. Cheap to clone.
#[derive(Clone)]
pub struct HostTunnel {
    inner: Arc<TunnelInner>,
}

struct TunnelInner {
    info: Mutex<Tunnel>,
    closed_at: Mutex<Option<Instant>>,
    flows: Mutex<Vec<Arc<Flow>>>,
    resources: Mutex<Vec<Box<dyn Resource>>>,
    /// Working folder of processes that do not ask for another (the git workspace, T-107).
    default_cwd: Mutex<Option<String>>,
    next_flow: AtomicU32,
    opts: HostOptions,
    processes: Table,
    /// The input of each terminal process.
    stdin: Mutex<BTreeMap<ProcessId, Stdin>>,
    /// The close already finished (flows, resources and processes): what is sent afterwards is already the
    /// last.
    settled: AtomicBool,
    /// Rises with each byte written, new flow or close: whoever sends the output follows it.
    changes: watch::Sender<u64>,
    /// Changes of all the tunnels (the UI), which also rise with each process.
    agent_changes: Changes,
}

/// A terminal process's input: what was typed goes to a task that writes it in order.
struct Stdin {
    /// Bytes accepted so far: the position of the next one.
    next: u64,
    tx: mpsc::UnboundedSender<Vec<u8>>,
    terminal: Arc<dyn Terminal>,
}

struct Flow {
    info: FlowInfo,
    state: Mutex<FlowState>,
}

struct FlowState {
    buf: OutputBuffer,
    ended: bool,
    /// The log was already told that the buffer discards (once per flow).
    warned: bool,
}

/// The tunnel is already closed: it admits no new flows or resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("tunnel {0} is closed")]
pub struct TunnelClosed(pub TunnelId);

impl HostTunnel {
    fn new(info: Tunnel, opts: HostOptions, agent_changes: Changes) -> Self {
        HostTunnel {
            inner: Arc::new(TunnelInner {
                info: Mutex::new(info),
                closed_at: Mutex::new(None),
                flows: Mutex::new(Vec::new()),
                resources: Mutex::new(Vec::new()),
                default_cwd: Mutex::new(None),
                next_flow: AtomicU32::new(1),
                opts,
                processes: Table::new(),
                stdin: Mutex::new(BTreeMap::new()),
                settled: AtomicBool::new(false),
                changes: watch::Sender::new(0),
                agent_changes,
            }),
        }
    }

    /// Sets the tunnel's default working folder for processes.
    pub fn set_default_cwd(&self, cwd: impl Into<String>) {
        *lock(&self.inner.default_cwd) = Some(cwd.into());
    }

    pub fn default_cwd(&self) -> Option<String> {
        lock(&self.inner.default_cwd).clone()
    }

    pub fn info(&self) -> Tunnel {
        lock(&self.inner.info).clone()
    }

    pub fn id(&self) -> TunnelId {
        lock(&self.inner.info).id
    }

    pub fn is_open(&self) -> bool {
        self.closed_at().is_none()
    }

    fn closed_at(&self) -> Option<Instant> {
        *lock(&self.inner.closed_at)
    }

    fn bump(&self) {
        self.inner.changes.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// A new output flow (for example, a process's), with the tunnel's circular
    /// buffer.
    pub fn add_flow(&self, label: impl Into<String>) -> Result<FlowWriter, TunnelClosed> {
        let mut flows = lock(&self.inner.flows);
        if !self.is_open() {
            return Err(TunnelClosed(self.id()));
        }
        let id = FlowId(self.inner.next_flow.fetch_add(1, Ordering::Relaxed));
        let flow = Arc::new(Flow {
            info: FlowInfo {
                id,
                label: label.into(),
            },
            state: Mutex::new(FlowState {
                buf: OutputBuffer::new(self.inner.opts.capacity),
                ended: false,
                warned: false,
            }),
        });
        flows.push(flow.clone());
        drop(flows);
        self.bump();
        Ok(FlowWriter {
            tunnel: self.clone(),
            flow,
        })
    }

    /// Registers something to release when the tunnel closes. If it is already closed it is released right away.
    pub fn add_resource(&self, resource: Box<dyn Resource>) -> Result<(), TunnelClosed> {
        let mut resources = lock(&self.inner.resources);
        if !self.is_open() {
            drop(resources);
            resource.close();
            return Err(TunnelClosed(self.id()));
        }
        resources.push(resource);
        Ok(())
    }

    /// `true` if this call closed it.
    fn close(&self) -> bool {
        // With the flows lock (the same order as `add_flow`): whoever sends the output
        // and sees the tunnel closed also sees all its flows ended.
        {
            let flows = lock(&self.inner.flows);
            {
                let mut at = lock(&self.inner.closed_at);
                if at.is_some() {
                    return false;
                }
                *at = Some(Instant::now());
            }
            for flow in flows.iter() {
                lock(&flow.state).ended = true;
            }
        }
        lock(&self.inner.info).state = TunnelState::Closed;
        let resources = std::mem::take(&mut *lock(&self.inner.resources));
        for r in &resources {
            r.close();
        }
        // With the tunnel already closed: no new process enters (see `Table::insert`).
        self.inner.processes.kill_all();
        lock(&self.inner.stdin).clear();
        self.inner.settled.store(true, Ordering::SeqCst);
        self.bump();
        true
    }

    fn is_settled(&self) -> bool {
        self.inner.settled.load(Ordering::SeqCst)
    }

    /// Launches a process with `spawner` (T-104). Its stdout and stderr are two new flows
    /// of the tunnel; the process stays in the table until `linger` has passed since its end.
    pub fn exec(
        &self,
        spawner: &dyn Spawner,
        mut spec: ExecSpec,
    ) -> Result<TunnelProcess, ExecError> {
        let tunnel = self.id();
        // A folder of this machine is not valid inside WSL or a container.
        let local_shell = !spec
            .shell
            .as_deref()
            .is_some_and(|s| s.starts_with("wsl:") || s.starts_with("docker:"));
        if spec.cwd.is_none() && local_shell {
            spec.cwd = self.default_cwd();
        }
        if !self.is_open() {
            return Err(ExecError::Closed(tunnel));
        }
        self.purge_processes();
        let launch: Launch = launch::plan(&spec, Os::current())?;
        let mut spawned = spawner.spawn(&launch)?;
        let terminal = match (spawned.stdin.take(), spawned.terminal.take()) {
            (Some(stdin), Some(terminal)) => Some((stdin, terminal)),
            _ => None,
        };
        let id = self.inner.processes.next_id();
        let command = launch::display(&spec);
        let flows = self
            .add_flow(format!("{id} {command} · stdout"))
            .and_then(|out| Ok((out, self.add_flow(format!("{id} {command} · stderr"))?)));
        let Ok((out, err)) = flows else {
            spawned.child.kill_tree();
            return Err(ExecError::Closed(tunnel));
        };
        let info = TunnelProcess {
            id,
            tunnel,
            command,
            shell: spec.shell,
            cwd: spec.cwd,
            pid: spawned.pid,
            stdout: out.id(),
            stderr: err.id(),
            started: now_millis(),
            duration: None,
            state: tandem_proto::ProcessState::Running,
            pty: terminal.is_some(),
        };
        if !self
            .inner
            .processes
            .insert(info.clone(), spawned.child.clone(), || self.is_open())
        {
            spawned.child.kill_tree();
            return Err(ExecError::Closed(tunnel));
        }
        if let Some((mut stdin, terminal)) = terminal {
            let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
            tokio::spawn(async move {
                while let Some(bytes) = rx.recv().await {
                    if stdin.write_all(&bytes).await.is_err() || stdin.flush().await.is_err() {
                        break;
                    }
                }
            });
            lock(&self.inner.stdin).insert(
                id,
                Stdin {
                    next: 0,
                    tx,
                    terminal,
                },
            );
        }
        tracing::info!(%tunnel, process = %id, command = %info.command, pty = info.pty, "launched process");
        crate::process::supervise(self.clone(), id, spawned, out, err);
        self.process_changed();
        Ok(info)
    }

    /// Kills a process with its whole tree. Killing one that already ended does nothing.
    pub fn kill(&self, process: ProcessId) -> Result<(), ExecError> {
        match self.inner.processes.kill(process) {
            None => Err(ExecError::UnknownProcess(self.id(), process)),
            Some(was_running) => {
                if was_running {
                    tracing::info!(tunnel = %self.id(), %process, "killed process");
                    self.process_changed();
                }
                Ok(())
            }
        }
    }

    /// `bytes` go at position `offset` of the process's input; only what it did not have yet
    /// is typed, so a resend never types twice. Returns the position of the next byte.
    pub fn input(&self, process: ProcessId, offset: u64, bytes: &[u8]) -> Result<u64, ExecError> {
        let mut all = lock(&self.inner.stdin);
        let Some(s) = all.get_mut(&process) else {
            return Err(self.no_terminal(process));
        };
        if offset > s.next {
            return Err(ExecError::InputGap(self.id(), process, s.next));
        }
        let have = usize::try_from(s.next - offset).unwrap_or(usize::MAX);
        let new = bytes.get(have..).unwrap_or_default();
        if !new.is_empty() {
            // If the process already ended its input is gone: what arrives is accepted and
            // dropped, as a terminal would.
            let _ = s.tx.send(new.to_vec());
            s.next += new.len() as u64;
        }
        Ok(s.next)
    }

    pub fn resize(&self, process: ProcessId, size: PtySize) -> Result<(), ExecError> {
        let terminal = lock(&self.inner.stdin)
            .get(&process)
            .map(|s| s.terminal.clone())
            .ok_or_else(|| self.no_terminal(process))?;
        terminal.resize(size)?;
        Ok(())
    }

    fn no_terminal(&self, process: ProcessId) -> ExecError {
        if self.inner.processes.get(process).is_some() {
            ExecError::NoTerminal(self.id(), process)
        } else {
            ExecError::UnknownProcess(self.id(), process)
        }
    }

    /// Rises with each byte written, new flow, process change or close.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.inner.changes.subscribe()
    }

    /// The process ended on its own (called by whoever watches it).
    pub(crate) fn process_exited(&self, process: ProcessId, code: Option<i32>) {
        if self.inner.processes.exited(process, code) {
            tracing::info!(tunnel = %self.id(), %process, ?code, "ended process");
            self.process_changed();
        }
    }

    fn process_changed(&self) {
        self.bump();
        self.inner.agent_changes.bump();
    }

    /// The tunnel's processes, also those that ended recently.
    pub fn processes(&self) -> Vec<TunnelProcess> {
        self.purge_processes();
        self.inner.processes.list()
    }

    /// Those that are running, from oldest to newest.
    pub fn running(&self) -> Vec<TunnelProcess> {
        self.inner.processes.running()
    }

    /// The output of a process from `from`, with up to `max` bytes of each flow.
    pub fn output(
        &self,
        process: ProcessId,
        from: OutputCursor,
        max: usize,
    ) -> Result<ProcessOutput, ExecError> {
        let p = self
            .inner
            .processes
            .get(process)
            .ok_or(ExecError::UnknownProcess(self.id(), process))?;
        let flows = self.flows();
        Ok(output::collect(p, from, max, |flow, at, max| {
            let f = flows.iter().find(|f| f.info.id == flow)?;
            let s = lock(&f.state);
            Some(FlowRead {
                read: s.buf.read(at, max),
                end: s.buf.end(),
                ended: s.ended,
            })
        }))
    }

    /// Forgets the processes ended more than `linger` ago with their flows.
    fn purge_processes(&self) {
        let gone = self.inner.processes.purge(self.inner.opts.linger);
        if gone.is_empty() {
            return;
        }
        lock(&self.inner.stdin).retain(|id, _| !gone.iter().any(|p| p.id == *id));
        lock(&self.inner.flows).retain(|f| {
            !gone
                .iter()
                .any(|p| p.stdout == f.info.id || p.stderr == f.info.id)
        });
    }

    fn flows(&self) -> Vec<Arc<Flow>> {
        lock(&self.inner.flows).clone()
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Where whoever produces a flow's output writes. Dropping it ends the flow.
pub struct FlowWriter {
    tunnel: HostTunnel,
    flow: Arc<Flow>,
}

impl FlowWriter {
    pub fn id(&self) -> FlowId {
        self.flow.info.id
    }

    /// Appends to the flow; if the buffer is full the oldest is discarded (and the origin will
    /// learn through [`TunnelDown::Lost`]). After ending, it does nothing.
    pub fn write(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        {
            let mut s = lock(&self.flow.state);
            if s.ended {
                return;
            }
            s.buf.push(bytes);
            if s.buf.dropped() > 0 && !s.warned {
                s.warned = true;
                tracing::warn!(
                    tunnel = %self.tunnel.id(),
                    flow = %self.flow.info.label,
                    "the output buffer filled up without anyone reading it: the oldest is discarded"
                );
            }
        }
        self.tunnel.bump();
    }

    /// There is no more output.
    pub fn finish(&self) {
        let changed = {
            let mut s = lock(&self.flow.state);
            !std::mem::replace(&mut s.ended, true)
        };
        if changed {
            self.tunnel.bump();
        }
    }
}

impl Drop for FlowWriter {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Serves a tunnel stream opened by the origin: a request and its response, or the data
/// channel of a tunnel until it closes or is cut.
pub async fn serve<W: TargetWire>(host: &Host, mut wire: W) -> Result<(), WireError> {
    let Some(req) = wire.recv().await? else {
        return Ok(());
    };
    serve_request(host, wire, req).await
}

/// Like [`serve`] with the first request already read: for whoever dispatches the requests of other
/// modules (files and ports) and leaves the rest to the tunnels.
pub async fn serve_request<W: TargetWire>(
    host: &Host,
    mut wire: W,
    req: TunnelUp,
) -> Result<(), WireError> {
    let reply = match req {
        TunnelUp::Files(_) | TunnelUp::Port(_) | TunnelUp::Workspace(_) => {
            return wire
                .send(&TunnelDown::Refused(
                    "this machine does not serve files, ports or workspaces".into(),
                ))
                .await;
        }
        TunnelUp::Open(t) => TunnelDown::Opened(host.open(t)),
        TunnelUp::Close(id) => match host.close(id) {
            Ok(()) => TunnelDown::Closed(id),
            Err(e) => TunnelDown::Refused(e),
        },
        TunnelUp::List => TunnelDown::List(host.list()),
        TunnelUp::Exec { tunnel, spec } => match host.exec(tunnel, spec) {
            Ok(p) => TunnelDown::Started(p),
            Err(e) => TunnelDown::Refused(e.to_string()),
        },
        TunnelUp::Kill { tunnel, process } => match host.kill(tunnel, process) {
            Ok(()) => TunnelDown::Killed(process),
            Err(e) => TunnelDown::Refused(e.to_string()),
        },
        TunnelUp::Input {
            tunnel,
            process,
            offset,
            bytes,
        } => match host.input(tunnel, process, offset, &bytes) {
            Ok(next) => TunnelDown::InputAck { process, next },
            Err(e) => TunnelDown::Refused(e.to_string()),
        },
        TunnelUp::Resize {
            tunnel,
            process,
            size,
        } => match host.resize(tunnel, process, size) {
            Ok(()) => TunnelDown::Resized(process),
            Err(e) => TunnelDown::Refused(e.to_string()),
        },
        TunnelUp::Attach { tunnel, from } => {
            let Some(t) = host.get(tunnel) else {
                return wire.send(&TunnelDown::Refused(unknown(tunnel))).await;
            };
            wire.start_output()?;
            return stream(&t, from.into_iter().collect(), &mut wire).await;
        }
    };
    wire.send(&reply).await
}

/// Sends the output of `t` from the positions `from`, one piece per flow per turn, until
/// the tunnel closes and nothing is left.
async fn stream<W: TargetWire>(
    t: &HostTunnel,
    mut pos: BTreeMap<FlowId, u64>,
    wire: &mut W,
) -> Result<(), WireError> {
    let mut known: BTreeSet<FlowId> = BTreeSet::new();
    let mut ended: BTreeSet<FlowId> = BTreeSet::new();
    let mut procs: BTreeMap<ProcessId, u64> = BTreeMap::new();
    let mut changes = t.inner.changes.subscribe();
    loop {
        // Before looking: whatever is written from here wakes the wait below.
        changes.borrow_and_update();
        let closed = t.is_settled();
        let mut sent = false;
        // Processes first: this way the origin knows what the flows that follow belong to.
        for p in t.inner.processes.updates(&mut procs) {
            wire.send(&TunnelDown::Process(p)).await?;
            sent = true;
        }
        for flow in t.flows() {
            let id = flow.info.id;
            if known.insert(id) {
                wire.send(&TunnelDown::Flow(flow.info.clone())).await?;
            }
            if ended.contains(&id) {
                continue;
            }
            let at = pos.entry(id).or_insert(0);
            let (read, done, total) = {
                let s = lock(&flow.state);
                (s.buf.read(*at, CHUNK), s.ended, s.buf.end())
            };
            let msg = match read {
                Read::Data { offset, bytes } => {
                    *at = offset + bytes.len() as u64;
                    TunnelDown::Data {
                        flow: id,
                        offset,
                        bytes,
                    }
                }
                Read::Lost { from, to } => {
                    *at = to;
                    TunnelDown::Lost { flow: id, from, to }
                }
                Read::Pending if done => {
                    ended.insert(id);
                    TunnelDown::End { flow: id, total }
                }
                Read::Pending => continue,
            };
            wire.send(&msg).await?;
            sent = true;
        }
        if sent {
            continue;
        }
        if closed {
            return wire.send(&TunnelDown::TunnelClosed).await;
        }
        if changes.changed().await.is_err() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use tandem_core::Machine;
    use tandem_proto::{Client, ClientKind};

    use super::*;

    pub(crate) fn tunnel(id: u64) -> Tunnel {
        Tunnel {
            id: TunnelId(id),
            title: "tests".into(),
            project: None,
            origin: Machine::A,
            target: Machine::B,
            opened_by: Client {
                kind: ClientKind::Cli,
                session: "s".into(),
            },
            state: TunnelState::Open,
            created: 0,
        }
    }

    struct Flag(Arc<AtomicBool>);

    impl Resource for Flag {
        fn close(&self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn opening_twice_returns_the_same_and_closing_releases_the_resources() {
        let host = Host::new(HostOptions::default(), Changes::default());
        host.open(tunnel(1));
        let mut again = tunnel(1);
        again.title = "other".into();
        assert_eq!(host.open(again).title, "tests");
        let t = host.get(TunnelId(1)).unwrap();
        let closed = Arc::new(AtomicBool::new(false));
        t.add_resource(Box::new(Flag(closed.clone()))).unwrap();
        let w = t.add_flow("out").unwrap();
        host.close(TunnelId(1)).unwrap();
        assert!(closed.load(Ordering::SeqCst));
        assert_eq!(host.list()[0].state, TunnelState::Closed);
        // Whatever arrives afterwards is released right away and not accepted.
        let late = Arc::new(AtomicBool::new(false));
        assert!(t.add_resource(Box::new(Flag(late.clone()))).is_err());
        assert!(late.load(Ordering::SeqCst));
        assert!(t.add_flow("x").is_err());
        w.write(b"late");
        assert!(host.close(TunnelId(9)).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn closed_ones_are_forgotten_after_a_while() {
        let opts = HostOptions {
            linger: Duration::from_secs(5),
            ..HostOptions::default()
        };
        let host = Host::new(opts, Changes::default());
        host.open(tunnel(1));
        host.close(TunnelId(1)).unwrap();
        assert_eq!(host.list().len(), 1);
        tokio::time::advance(Duration::from_secs(6)).await;
        assert!(host.list().is_empty());
    }
}
