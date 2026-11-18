//! Errores de archivos y puertos.

use std::io;
use std::path::Path;

use tandem_proto::TunnelId;
use tandem_tunnel::{TunnelError, WireError};

#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    #[error("ruta no válida «{path}»: {why}")]
    Path { path: String, why: &'static str },
    #[error("{path}: {source}")]
    Io { path: String, source: io::Error },
    #[error("el fragmento {index} de «{path}» no coincide con su hash")]
    BadChunk { path: String, index: u32 },
    #[error("«{0}» no coincide con su hash al terminar de copiarse")]
    BadFile(String),
    #[error("manifiesto no válido: {0}")]
    Manifest(String),
    #[error("la otra máquina: {0}")]
    Remote(String),
    #[error(transparent)]
    Tunnel(#[from] TunnelError),
    #[error("se cortó el enlace: {0}")]
    Wire(#[from] WireError),
    #[error("no hay ningún túnel {0} abierto desde esta máquina")]
    NoTunnel(TunnelId),
    #[error("respuesta inesperada de la otra máquina")]
    Unexpected,
    #[error("la tarea de disco se interrumpió")]
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

    /// Un corte o un enlace que no está: se vuelve a intentar y se reanuda.
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
