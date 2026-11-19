//! File and port errors.

use std::io;
use std::path::Path;

use tandem_proto::TunnelId;
use tandem_tunnel::{TunnelError, WireError};

#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    #[error("invalid path \"{path}\": {why}")]
    Path { path: String, why: &'static str },
    #[error("{path}: {source}")]
    Io { path: String, source: io::Error },
    #[error("chunk {index} of \"{path}\" does not match its hash")]
    BadChunk { path: String, index: u32 },
    #[error("\"{0}\" does not match its hash after being copied")]
    BadFile(String),
    #[error("invalid manifest: {0}")]
    Manifest(String),
    #[error("\"{0}\" has too many chunks to be described in one message")]
    TooLarge(String),
    #[error("the other machine: {0}")]
    Remote(String),
    #[error(transparent)]
    Tunnel(#[from] TunnelError),
    #[error("the link was cut: {0}")]
    Wire(#[from] WireError),
    #[error("no tunnel {0} is open from this machine")]
    NoTunnel(TunnelId),
    #[error("unexpected response from the other machine")]
    Unexpected,
    #[error("the disk task was interrupted")]
    Join,
}

impl FilesError {
    pub fn io(path: &Path, source: io::Error) -> Self {
        FilesError::Io {
            path: path.display().to_string(),
            source,
        }
    }

    pub fn path(path: &str, why: &'static str) -> Self {
        FilesError::Path {
            path: path.to_owned(),
            why,
        }
    }

    /// A cut or a link that is not there: it is retried and resumed.
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            FilesError::Wire(_)
                | FilesError::Tunnel(
                    TunnelError::NoLink | TunnelError::Timeout | TunnelError::Wire(_)
                )
        )
    }
}

pub type Result<T> = std::result::Result<T, FilesError>;
