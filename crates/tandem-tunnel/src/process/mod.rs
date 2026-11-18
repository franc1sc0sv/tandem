//! Processes inside a tunnel (T-104, DESIGN §8).
//!
//! `exec` launches a process on the destination with the agent's `Spawner`; its stdout and stderr
//! are two tunnel flows (with the usual circular buffer and per-byte position, so they
//! survive link drops) and the process is recorded in the tunnel's table with its
//! state, exit code and duration. Closing the tunnel kills all the trees.
//!
//! - [`launch`]: from an `exec` to the concrete command (shells, WSL, Docker). Portable.
//! - [`Spawner`] / [`Child`]: what depends on the system. [`system::SystemSpawner`] is the
//!   real one (process group on Unix, job object on Windows); [`fake::FakeSpawner`],
//!   the test double.
//! - [`output`]: `output { from }` built from the flows.
//!
//! **Privileges (A7):** the process inherits the agent's; nothing here changes them.

pub mod fake;
pub mod launch;
pub mod output;
pub mod secrets;
pub mod system;
pub(crate) mod table;

use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::host::{FlowWriter, HostTunnel};
use crate::wire::BoxFuture;

pub use launch::{Launch, LaunchError, Os};

/// The launched process, to wait for it and kill it.
pub trait Child: Send + Sync {
    /// Resolves when the main process ended, with its code (`None` if a
    /// signal ended it). Called once.
    fn wait(&self) -> BoxFuture<'_, Option<i32>>;
    /// Kills the process and all its descendants. Has to return right away and can be
    /// called several times.
    fn kill_tree(&self);
    /// Nothing is left of the tree (the process ended and its outputs closed): killing it
    /// afterwards does nothing, so as not to touch an identifier the system already reused.
    fn release(&self) {}
}

/// A newly launched process with its outputs. Its stdin is closed: there is no interactive
/// input.
pub struct Spawned {
    /// The system identifier, if it gave one.
    pub pid: Option<u32>,
    pub stdout: Box<dyn AsyncRead + Send + Unpin>,
    pub stderr: Box<dyn AsyncRead + Send + Unpin>,
    pub child: Arc<dyn Child>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SpawnError(pub String);

/// Launches processes on this machine. The real one is [`system::SystemSpawner`].
pub trait Spawner: Send + Sync {
    fn spawn(&self, launch: &Launch) -> Result<Spawned, SpawnError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecError {
    #[error("there is no tunnel {0} on this machine")]
    UnknownTunnel(tandem_proto::TunnelId),
    #[error("tunnel {0} does not have process {1}")]
    UnknownProcess(tandem_proto::TunnelId, tandem_proto::ProcessId),
    #[error("tunnel {0} is closed")]
    Closed(tandem_proto::TunnelId),
    #[error("{0}")]
    Launch(#[from] LaunchError),
    #[error("could not launch: {0}")]
    Spawn(#[from] SpawnError),
}

/// Copies what comes out of `from` to the flow until it closes. On finishing it releases the flow,
/// which ends up finished.
async fn pump(mut from: Box<dyn AsyncRead + Send + Unpin>, to: FlowWriter) {
    let mut buf = vec![0u8; 32 * 1024];
    loop {
        match from.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => to.write(&buf[..n]),
        }
    }
}

/// Watches the process: copies its outputs to the flows and notes when it ends. What
/// stays alive of the tree keeps writing while it has the output open.
pub(crate) fn supervise(
    tunnel: HostTunnel,
    id: tandem_proto::ProcessId,
    spawned: Spawned,
    out: FlowWriter,
    err: FlowWriter,
) {
    let Spawned {
        stdout,
        stderr,
        child,
        ..
    } = spawned;
    let pumps = (
        tokio::spawn(pump(stdout, out)),
        tokio::spawn(pump(stderr, err)),
    );
    tokio::spawn(async move {
        let code = child.wait().await;
        tunnel.process_exited(id, code);
        let _ = pumps.0.await;
        let _ = pumps.1.await;
        child.release();
    });
}
