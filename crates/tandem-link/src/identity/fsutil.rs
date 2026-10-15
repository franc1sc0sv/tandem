//! Escritura de archivos sensibles: atómica y, en Unix, con permisos 0600.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::IdentityError;

/// Escribe `bytes` en `path` de forma atómica (archivo temporal + rename) para
/// que un corte a mitad de escritura nunca deje una llave o un trust store a
/// medias.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), IdentityError> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| IdentityError::io(dir, e))?;
    }
    let tmp = temp_path(path);
    let result = write_new(&tmp, bytes)
        .and_then(|()| fs::rename(&tmp, path).map_err(|e| IdentityError::io(path, e)));
    if result.is_err() {
        // Mejor esfuerzo: el error original es el que importa.
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".tmp-{}", std::process::id()));
    path.with_file_name(name)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), IdentityError> {
    let mut file = open_private(path).map_err(|e| IdentityError::io(path, e))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| IdentityError::io(path, e))
}

#[cfg(unix)]
fn open_private(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

// En Windows la protección llega con DPAPI (tarea posterior); el perfil del
// usuario ya restringe el acceso por ACL.
#[cfg(not(unix))]
fn open_private(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
}

/// Rechaza archivos secretos legibles por otros usuarios, como hace ssh.
#[cfg(unix)]
pub(crate) fn check_private(path: &Path, file: &File) -> Result<(), IdentityError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = file
        .metadata()
        .map_err(|e| IdentityError::io(path, e))?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 != 0 {
        return Err(IdentityError::InsecurePermissions {
            path: path.to_path_buf(),
            mode,
        });
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn check_private(_path: &Path, _file: &File) -> Result<(), IdentityError> {
    Ok(())
}
