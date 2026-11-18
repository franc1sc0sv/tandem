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
    #[error("«{0}» no está dentro de un repo de git")]
    NotARepo(String),
    #[error("el repo «{0}» no tiene ningún commit")]
    NoCommits(String),
    #[error("falta {0}")]
    Missing(String),
    #[error("el túnel {0} no tiene workspace abierto desde esta máquina")]
    NoWorkspace(TunnelId),
    #[error(transparent)]
    Tunnel(#[from] TunnelError),
    #[error(transparent)]
    Files(#[from] FilesError),
    #[error("se cortó el enlace: {0}")]
    Wire(#[from] WireError),
    #[error("la otra máquina: {0}")]
    Refused(String),
    #[error("respuesta inesperada de la otra máquina")]
    Unexpected,
    #[error("la tarea de git se interrumpió")]
    Join,
}
