//! Unix socket en el directorio de datos de la app.
//!
//! Solo para el usuario actual por tres lados: el directorio se crea en 0700, el socket
//! queda en 0600 y cada conexión se compara con nuestro uid (`peer_cred`).

use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use tokio::net::{UnixListener, UnixStream};

use super::{IpcClient, IpcError, Listener};

const SOCKET_FILE: &str = "tandemd.sock";

pub fn default_endpoint(data_dir: &Path) -> io::Result<PathBuf> {
    Ok(data_dir.join(SOCKET_FILE))
}

#[derive(Debug)]
pub struct UnixIpcListener {
    inner: UnixListener,
    path: PathBuf,
    uid: u32,
}

/// Escucha en `path`. Un socket viejo de un agente que ya no corre se reemplaza; si
/// hay otro agente vivo, error.
pub async fn bind(path: &Path) -> Result<UnixIpcListener, IpcError> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if !meta.file_type().is_socket() {
            return Err(IpcError::NotASocket(path.to_owned()));
        }
        if UnixStream::connect(path).await.is_ok() {
            return Err(IpcError::AlreadyRunning(path.to_owned()));
        }
        std::fs::remove_file(path)?;
    }
    if let Some(dir) = path.parent() {
        crate::paths::create_private_dir(dir)?;
    }
    let inner = UnixListener::bind(path)?;
    let mut listener = UnixIpcListener {
        inner,
        path: path.to_owned(),
        uid: 0,
    };
    // A partir de aquí el Drop borra el socket si algo falla.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    // El socket lo acabamos de crear: su dueño somos nosotros.
    listener.uid = std::fs::metadata(path)?.uid();
    Ok(listener)
}

pub async fn connect(path: &Path) -> Result<IpcClient<UnixStream>, IpcError> {
    IpcClient::handshake(UnixStream::connect(path).await?).await
}

impl UnixIpcListener {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Listener for UnixIpcListener {
    type Stream = UnixStream;

    async fn accept(&mut self) -> io::Result<UnixStream> {
        loop {
            let (stream, _) = self.inner.accept().await?;
            match stream.peer_cred() {
                Ok(cred) if cred.uid() == self.uid => return Ok(stream),
                Ok(cred) => {
                    tracing::warn!(uid = cred.uid(), "conexión IPC de otro usuario rechazada")
                }
                Err(e) => tracing::warn!("conexión IPC sin credenciales rechazada: {e}"),
            }
        }
    }
}

impl Drop for UnixIpcListener {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.path)
            && e.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!("no se pudo borrar {}: {e}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El sandbox de desarrollo (WSL) no deja crear sockets Unix. Ahí se omiten estas
    /// pruebas; en CI (`CI` definido) corren siempre, así que no se pierde cobertura.
    fn sockets_unavailable() -> bool {
        if std::env::var_os("CI").is_some() {
            return false;
        }
        let dir = tempfile::tempdir().unwrap();
        match std::os::unix::net::UnixListener::bind(dir.path().join("probe.sock")) {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("omitida: este entorno no permite sockets Unix");
                true
            }
            _ => false,
        }
    }

    #[tokio::test]
    async fn socket_is_owner_only_and_removed_on_drop() {
        if sockets_unavailable() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run/tandemd.sock");
        let l = bind(&path).await.unwrap();
        assert_eq!(l.path(), path);
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        let dir_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        drop(l);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn second_agent_is_refused() {
        if sockets_unavailable() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tandemd.sock");
        let _first = bind(&path).await.unwrap();
        let err = bind(&path).await.unwrap_err();
        assert!(matches!(err, IpcError::AlreadyRunning(_)), "{err}");
        // El primero sigue escuchando.
        assert!(path.exists());
    }

    #[tokio::test]
    async fn stale_socket_is_replaced() {
        if sockets_unavailable() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tandemd.sock");
        // Un socket que quedó de un agente que murió sin limpiar.
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        assert!(path.exists());
        let _l = bind(&path).await.unwrap();
    }

    #[tokio::test]
    async fn regular_file_is_not_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tandemd.sock");
        std::fs::write(&path, b"no tocar").unwrap();
        let err = bind(&path).await.unwrap_err();
        assert!(matches!(err, IpcError::NotASocket(_)), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), b"no tocar");
    }

    #[test]
    fn default_endpoint_is_in_data_dir() {
        assert_eq!(
            default_endpoint(Path::new("/d")).unwrap(),
            Path::new("/d/tandemd.sock")
        );
    }
}
