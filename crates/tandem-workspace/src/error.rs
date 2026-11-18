use tandem_files::FilesError;
use tandem_proto::TunnelId;
use tandem_tunnel::{TunnelError, WireError};

use crate::git::GitError;

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("{0}")]
    Io(String),
    #[error("\"{0}\" is not inside a git repo")]
    NotARepo(String),
    #[error("the repo \"{0}\" has no commits")]
    NoCommits(String),
    #[error("missing {0}")]
    Missing(String),
    #[error("tunnel {0} has no workspace open from this machine")]
    NoWorkspace(TunnelId),
    #[error(transparent)]
    Tunnel(#[from] TunnelError),
    #[error(transparent)]
    Files(#[from] FilesError),
    #[error("the link was cut: {0}")]
    Wire(#[from] WireError),
    #[error("the other machine: {0}")]
    Refused(String),
    #[error("unexpected response from the other machine")]
    Unexpected,
    #[error("the git task was interrupted")]
    Join,
}
