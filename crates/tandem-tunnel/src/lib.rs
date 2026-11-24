//! Tunnels: Claude (or a person) uses the other machine (DESIGN §8, DECISIONS D8, D9, A7).
//!
//! A tunnel is opened by a local client over the IPC on the **origin** machine and lives on the
//! **destination**, with its output flows and its resources. Portable: there is no platform
//! code here except the real process launcher (T-104, [`process::system`], behind the
//! trait [`process::Spawner`]); files and ports (T-106) and git workspaces (T-107)
//! hang off a [`HostTunnel`] as [`FlowWriter`] and [`Resource`].
//!
//! - [`host`]: the destination. [`Host`] registers the tunnels; [`serve`] serves each stream that
//!   the origin opens (a request, or a tunnel's data channel).
//! - [`origin`]: the origin. [`Origin`] opens, closes and lists, and follows each
//!   tunnel's output over the active link, resuming "from byte N" after a cut.
//! - [`process`]: processes inside a tunnel (`exec`, `output`, `kill`).
//! - [`buffer`]: the circular buffer with absolute per-byte position that both use.
//! - [`wire`]: the transport traits (QUIC in `tandemd`, memory in [`mem`]).
//!
//! **Surviving the link:** tunnels do not depend on any stream. If the link drops,
//! what is on the destination keeps running and its output is stored (up to [`DEFAULT_CAPACITY`] per
//! flow; if it fills up, the oldest is discarded and the origin receives the notice
//! [`tandem_proto::TunnelDown::Lost`]). On return, by the same path or another, the
//! origin resumes from the last byte it has.
//!
//! **Privileges (A7):** what runs in a tunnel has those of the destination agent.

pub mod buffer;
pub mod host;
pub mod live;
pub mod mem;
pub mod origin;
pub mod process;
pub mod wire;

use std::sync::Arc;
use std::time::Duration;

use tandem_proto::{
    OutputCursor, ProcessId, ProcessOutput, ProjectTunnels, PtySize, TunnelId, TunnelProcess,
    TunnelState, TunnelView, group_by_project,
};
use tokio::sync::watch;

pub use buffer::{DEFAULT_CAPACITY, OutputBuffer, Read};
pub use host::{
    FlowWriter, Host, HostOptions, HostTunnel, Resource, TunnelClosed, serve, serve_request,
};
pub use live::OutputFollower;
pub use origin::{FlowStatus, Link, OpenSpec, Origin, OriginOptions, TunnelError};
pub use process::{ExecError, Spawner};
pub use wire::{BoxFuture, Dialer, OriginWire, TargetWire, WireError};

/// Notifies that some tunnel changed (open, close). Cheap to clone; all clones are the
/// same notice.
#[derive(Clone)]
pub struct Changes(Arc<watch::Sender<u64>>);

impl Default for Changes {
    fn default() -> Self {
        Changes(Arc::new(watch::Sender::new(0)))
    }
}

impl Changes {
    pub fn bump(&self) {
        self.0.send_modify(|n| *n = n.wrapping_add(1));
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.subscribe()
    }
}

/// An agent's tunnels: those it hosts (destination) and those it opened (origin).
#[derive(Clone)]
pub struct Tunnels {
    pub host: Host,
    pub origin: Origin,
    changes: Changes,
}

impl std::fmt::Debug for Tunnels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tunnels").finish_non_exhaustive()
    }
}

impl Default for Tunnels {
    fn default() -> Self {
        Self::new(HostOptions::default(), OriginOptions::default())
    }
}

impl Tunnels {
    pub fn new(host: HostOptions, origin: OriginOptions) -> Self {
        let changes = Changes::default();
        Tunnels {
            host: Host::new(host, changes.clone()),
            origin: Origin::new(origin, changes.clone()),
            changes,
        }
    }

    /// The open tunnels of both machines as this agent sees them (those it opened
    /// and those it hosts), grouped by project: what the UI shows.
    pub fn view(&self) -> Vec<ProjectTunnels> {
        let mut all: Vec<TunnelView> = self.origin.views();
        for t in self.host.views() {
            if !all.iter().any(|o| o.tunnel.id == t.tunnel.id) {
                all.push(t);
            }
        }
        all.retain(|t| t.tunnel.state == TunnelState::Open);
        group_by_project(all)
    }

    /// The processes of a tunnel of either machine, also those that
    /// ended recently. `None` if the tunnel is not known.
    pub fn processes(&self, tunnel: TunnelId) -> Option<Vec<TunnelProcess>> {
        self.origin
            .processes(tunnel)
            .or_else(|| self.host.get(tunnel).map(|t| t.processes()))
    }

    /// The output of a process from `from`: from the mirror if this machine opened the tunnel, and
    /// otherwise from its own flows.
    pub fn output(
        &self,
        tunnel: TunnelId,
        process: ProcessId,
        from: OutputCursor,
        max: usize,
    ) -> Result<ProcessOutput, TunnelError> {
        match self.origin.output(tunnel, process, from, max) {
            Err(TunnelError::Unknown(_)) => {}
            other => return other,
        }
        let t = self.host.get(tunnel).ok_or(TunnelError::Unknown(tunnel))?;
        t.output(process, from, max)
            .map_err(|_| TunnelError::UnknownProcess(tunnel, process))
    }

    /// Like [`Self::output`], but with nothing new from `from` it waits up to `wait` for output
    /// or for the end of the process. It answers the moment a byte arrives: a terminal's echo
    /// is not held back.
    pub async fn wait_output(
        &self,
        tunnel: TunnelId,
        process: ProcessId,
        from: OutputCursor,
        max: usize,
        wait: Duration,
    ) -> Result<ProcessOutput, TunnelError> {
        let deadline = tokio::time::Instant::now() + wait;
        let mut mirror = self.origin.subscribe_output();
        let mut own = self.host.get(tunnel).map(|t| t.subscribe());
        loop {
            // Before reading: whatever arrives from here wakes the wait below.
            mirror.borrow_and_update();
            if let Some(own) = &mut own {
                own.borrow_and_update();
            }
            let out = self.output(tunnel, process, from, max)?;
            let fresh = !out.stdout.is_empty() || !out.stderr.is_empty();
            if fresh || out.done || tokio::time::Instant::now() >= deadline {
                return Ok(out);
            }
            let own_changed = async {
                match &mut own {
                    Some(rx) => {
                        if rx.changed().await.is_err() {
                            std::future::pending::<()>().await;
                        }
                    }
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                r = mirror.changed() => if r.is_err() { return Ok(out) },
                () = own_changed => {}
                () = tokio::time::sleep_until(deadline) => {}
            }
        }
    }

    /// Types into a terminal process of a tunnel this machine opened ([`Origin::input`]).
    pub fn input(
        &self,
        tunnel: TunnelId,
        process: ProcessId,
        bytes: &[u8],
    ) -> Result<(), TunnelError> {
        self.origin.input(tunnel, process, bytes)
    }

    pub async fn resize(
        &self,
        tunnel: TunnelId,
        process: ProcessId,
        size: PtySize,
    ) -> Result<(), TunnelError> {
        self.origin.resize(tunnel, process, size).await
    }

    /// Rises with each tunnel that opens or closes, here or on the other machine.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }
}
