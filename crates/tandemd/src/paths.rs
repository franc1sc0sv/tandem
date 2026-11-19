//! Where the configuration, state and logs live.
//!
//! By default, in each system's app directory: `~/Library/Application Support/Tandem`
//! on macOS, `%APPDATA%\Tandem` on Windows and `~/.local/share/Tandem` on Linux (tests only).

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
#[error("the system does not report a {0} directory; pass it with the corresponding option")]
pub struct NoDefaultDir(&'static str);

/// Absolute path without requiring it to exist; if the cwd cannot be read, it is left as is.
pub fn absolute(path: PathBuf) -> PathBuf {
    std::path::absolute(&path).unwrap_or(path)
}

impl Paths {
    /// What does not come on the command line comes from the standard directories.
    pub fn resolve(
        config: Option<PathBuf>,
        data_dir: Option<PathBuf>,
    ) -> Result<Self, NoDefaultDir> {
        let data_dir = match data_dir {
            Some(d) => d,
            None => dirs::data_dir()
                .map(|d| d.join(APP_DIR))
                .ok_or(NoDefaultDir("data"))?,
        };
        let config = match config {
            Some(c) => c,
            None => dirs::config_dir()
                .map(|d| d.join(APP_DIR).join(CONFIG_FILE))
                .ok_or(NoDefaultDir("configuration"))?,
        };
        // Made absolute once: git and other children run in another cwd (T-197).
        Ok(Paths {
            config: absolute(config),
            data_dir: absolute(data_dir),
        })
    }

    pub fn state_file(&self) -> PathBuf {
        self.data_dir.join(STATE_FILE)
    }

    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join(LOG_DIR)
    }

    /// The Ed25519 key of this machine (DESIGN §9).
    pub fn identity_file(&self) -> PathBuf {
        self.data_dir.join(IDENTITY_FILE)
    }

    /// The keys of the paired peers.
    pub fn trust_file(&self) -> PathBuf {
        self.data_dir.join(TRUST_FILE)
    }
}

/// Creates the directory (and its parents) accessible only to the current user.
///
/// In Unix, a newly created one is left as 0700: the IPC socket lives there. One that
/// already existed is not touched; the IPC still checks who connects.
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
    fn relative_paths_become_absolute() {
        let p = Paths::resolve(Some("c.toml".into()), Some("agent/data".into())).unwrap();
        assert!(p.data_dir.is_absolute() && p.data_dir.ends_with("agent/data"));
        assert!(p.config.is_absolute());
    }

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
        // In CI there is always HOME / APPDATA.
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
        // Creating it again is not an error.
        create_private_dir(&dir).unwrap();
    }
}
