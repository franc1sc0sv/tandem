//! Files and ports per tunnel (DESIGN §8, T-106), and base of the drags (§9, T-121).
//!
//! - [`path`]: normalized paths between Windows and Unix.
//! - [`source`] / [`sink`]: read a tree with BLAKE3 hashes per chunk (1 MiB) and of the total;
//!   write it verifying, with resumption from disk, permissions and dates preserved.
//! - [`transfer`]: `put` and `get` over a tunnel stream, with progress and retry.
//! - [`ports`]: TCP port forwarding in both directions.
//! - [`Files`]: what an agent hangs on (the destination side and the origin's forwards).
//! - [`drag`]: drag files when crossing the edge (D14), with the system behind traits.
//! - [`app_quit`]: what the agent's AppKit host answers to a Quit Apple event (T-333).
//! - [`mem`]: in-memory paths with cuts, to test with two agents in one process.
//!
//! Chunks and requests go over `Channel::Tunnel` (T-103), so they inherit the
//! low priority and the bandwidth cap of data (T-101).

pub mod app_quit;
pub mod drag;
#[cfg(target_os = "macos")]
pub mod drag_macos;
pub mod drag_os;
#[cfg(windows)]
pub mod drag_target_windows;
#[cfg(windows)]
pub mod drag_windows;
pub mod error;
pub mod mem;
pub mod path;
pub mod ports;
pub mod secrets;
pub mod sink;
pub mod source;
pub mod transfer;

use std::path::Path;
use std::sync::Arc;

use tandem_proto::{FilesReport, ForwardInfo, TransferProgress, TunnelId, TunnelUp};
use tandem_tunnel::{Host, Origin, TargetWire, WireError};
use tokio::sync::broadcast;

pub use error::FilesError;
pub use ports::{ForwardSpec, Forwards, PortHost};
pub use secrets::SecretFilter;
pub use sink::Sink;
pub use source::Source;
pub use transfer::Options;

/// Files and ports of an agent: serves the other machine's requests (destination) and
/// carries the transfers and forwards that local clients opened (origin).
#[derive(Clone)]
pub struct Files {
    ports: PortHost,
    forwards: Forwards,
    progress: broadcast::Sender<TransferProgress>,
    opts: Arc<Options>,
}

impl Default for Files {
    fn default() -> Self {
        Self::new(Options::default())
    }
}

impl std::fmt::Debug for Files {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Files").finish_non_exhaustive()
    }
}

impl Files {
    pub fn new(opts: Options) -> Self {
        Files {
            ports: PortHost::default(),
            forwards: Forwards::default(),
            progress: broadcast::channel(64).0,
            opts: Arc::new(opts),
        }
    }

    /// The progress of the transfers this machine started.
    pub fn subscribe_progress(&self) -> broadcast::Receiver<TransferProgress> {
        self.progress.subscribe()
    }

    pub fn forwards(&self) -> &Forwards {
        &self.forwards
    }

    /// Serves a tunnel stream opened by the other machine: files and ports here, the
    /// rest (open, close, data channel) is resolved by `tandem-tunnel`.
    pub async fn serve<W: TargetWire>(&self, host: &Host, mut wire: W) -> Result<(), WireError> {
        let Some(req) = wire.recv().await? else {
            return Ok(());
        };
        self.serve_request(host, wire, req).await
    }

    /// Like [`Files::serve`] with the first request already read: for whoever dispatches requests of
    /// other modules (workspaces) and leaves those of files and ports here.
    pub async fn serve_request<W: TargetWire>(
        &self,
        host: &Host,
        mut wire: W,
        req: TunnelUp,
    ) -> Result<(), WireError> {
        match req {
            TunnelUp::Files(f) => transfer::serve(host, &mut wire, f, self.opts.chunk_size).await,
            TunnelUp::Port(p) => ports::serve(host, &self.ports, &mut wire, p).await,
            other => tandem_tunnel::serve_request(host, wire, other).await,
        }
    }

    /// Copies `local` to `remote` on the tunnel's destination. What looks like a credential does not
    /// travel (T-130); what was skipped goes in the report.
    pub async fn put(
        &self,
        origin: &Origin,
        tunnel: TunnelId,
        local: &Path,
        remote: &str,
    ) -> Result<FilesReport, FilesError> {
        self.put_with(origin, tunnel, local, remote, &[]).await
    }

    /// Like [`Files::put`], also sending the credentials that `include` names (paths
    /// relative to `local`).
    pub async fn put_with(
        &self,
        origin: &Origin,
        tunnel: TunnelId,
        local: &Path,
        remote: &str,
        include: &[String],
    ) -> Result<FilesReport, FilesError> {
        let sink = |p| {
            let _ = self.progress.send(p);
        };
        transfer::put(origin, tunnel, local, remote, include, &self.opts, &sink).await
    }

    /// Fetches `remote` from the tunnel's destination to `local`.
    pub async fn get(
        &self,
        origin: &Origin,
        tunnel: TunnelId,
        remote: &str,
        local: &Path,
    ) -> Result<FilesReport, FilesError> {
        let sink = |p| {
            let _ = self.progress.send(p);
        };
        transfer::get(origin, tunnel, remote, local, &self.opts, &sink).await
    }

    /// Forwards a port through the tunnel.
    pub async fn forward(
        &self,
        origin: &Origin,
        spec: ForwardSpec,
    ) -> Result<ForwardInfo, FilesError> {
        self.forwards.start(origin, spec).await
    }
}
