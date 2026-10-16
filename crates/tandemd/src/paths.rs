//! Dónde viven la configuración, el estado y los logs.
//!
//! Por omisión, en el directorio de la app de cada sistema: `~/Library/Application Support/Tandem`
//! en macOS, `%APPDATA%\Tandem` en Windows y `~/.local/share/Tandem` en Linux (solo pruebas).

use std::io;
use std::path::{Path, PathBuf};

const APP_DIR: &str = "Tandem";
const CONFIG_FILE: &str = "tandem.toml";
const STATE_FILE: &str = "state.json";
const LOG_DIR: &str = "logs";
const IDENTITY_FILE: &str = "identity.key";
const TRUST_FILE: &str = "trust.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config: PathBuf,
    pub data_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
#[error("el sistema no indica un directorio de {0}; pásalo con la opción correspondiente")]
pub struct NoDefaultDir(&'static str);

impl Paths {
    /// Lo que no venga en la línea de órdenes sale de los directorios estándar.
    pub fn resolve(
        config: Option<PathBuf>,
        data_dir: Option<PathBuf>,
    ) -> Result<Self, NoDefaultDir> {
        let data_dir = match data_dir {
            Some(d) => d,
            None => dirs::data_dir()
                .map(|d| d.join(APP_DIR))
                .ok_or(NoDefaultDir("datos"))?,
        };
        let config = match config {
            Some(c) => c,
            None => dirs::config_dir()
                .map(|d| d.join(APP_DIR).join(CONFIG_FILE))
                .ok_or(NoDefaultDir("configuración"))?,
        };
        Ok(Paths { config, data_dir })
    }

    pub fn state_file(&self) -> PathBuf {
        self.data_dir.join(STATE_FILE)
    }

    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join(LOG_DIR)
    }

    /// La llave Ed25519 de esta máquina (DESIGN §9).
    pub fn identity_file(&self) -> PathBuf {
        self.data_dir.join(IDENTITY_FILE)
    }

    /// Las llaves de los pares emparejados.
    pub fn trust_file(&self) -> PathBuf {
        self.data_dir.join(TRUST_FILE)
    }
}

/// Crea el directorio (y sus padres) accesible solo para el usuario actual.
///
/// En Unix, el que se crea nuevo queda en 0700: ahí vive el socket de la IPC. Uno que ya
/// existía no se toca; la IPC igual comprueba quién se conecta.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_paths_win() {
        let p = Paths::resolve(Some("/x/t.toml".into()), Some("/d".into())).unwrap();
        assert_eq!(p.config, PathBuf::from("/x/t.toml"));
        assert_eq!(p.state_file(), Path::new("/d").join("state.json"));
        assert_eq!(p.log_dir(), Path::new("/d").join("logs"));
        assert_eq!(p.identity_file(), Path::new("/d").join("identity.key"));
        assert_eq!(p.trust_file(), Path::new("/d").join("trust.toml"));
    }

    #[test]
    fn defaults_live_in_app_dir() {
        // En CI siempre hay HOME / APPDATA.
        let p = Paths::resolve(None, None).unwrap();
        assert!(p.data_dir.ends_with(APP_DIR));
        assert!(p.config.ends_with(Path::new(APP_DIR).join(CONFIG_FILE)));
    }

    #[cfg(unix)]
    #[test]
    fn private_dir_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a/b");
        create_private_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        // Volver a crearlo no es un error.
        create_private_dir(&dir).unwrap();
    }
}
