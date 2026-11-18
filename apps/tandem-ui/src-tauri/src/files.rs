//! Settings → Advanced: where the configuration file and the logs are, and how to show them
//! in each system's file explorer. The command is built here (pure) to test it on Linux.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Paths shown by Settings → Advanced (text, as the user sees them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FilesInfo {
    pub config: String,
    pub logs: String,
}

/// What to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Config,
    Logs,
}

/// The configuration file and the logs folder of this machine.
pub fn locate() -> Result<(PathBuf, PathBuf), String> {
    let p = tandemd::paths::Paths::resolve(None, None).map_err(|e| e.to_string())?;
    let logs = p.log_dir();
    Ok((p.config, logs))
}

/// Command that shows `path` in the explorer: the file selected or the folder open.
pub fn reveal_command(os: &str, path: &Path, is_file: bool) -> (String, Vec<String>) {
    let p = path.display().to_string();
    match os {
        "macos" if is_file => ("open".into(), vec!["-R".into(), p]),
        "macos" => ("open".into(), vec![p]),
        "windows" if is_file => ("explorer".into(), vec![format!("/select,{p}")]),
        "windows" => ("explorer".into(), vec![p]),
        _ => {
            let dir = if is_file {
                path.parent().unwrap_or(path).display().to_string()
            } else {
                p
            };
            ("xdg-open".into(), vec![dir])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_os_reveals_a_file_or_opens_a_folder() {
        let f = Path::new("/a/b/tandem.toml");
        assert_eq!(
            reveal_command("macos", f, true),
            ("open".into(), vec!["-R".into(), "/a/b/tandem.toml".into()])
        );
        assert_eq!(
            reveal_command("macos", Path::new("/a/logs"), false),
            ("open".into(), vec!["/a/logs".into()])
        );
        assert_eq!(
            reveal_command("windows", f, true).1,
            vec!["/select,/a/b/tandem.toml".to_string()]
        );
        assert_eq!(
            reveal_command("linux", f, true),
            ("xdg-open".into(), vec!["/a/b".into()])
        );
    }
}
