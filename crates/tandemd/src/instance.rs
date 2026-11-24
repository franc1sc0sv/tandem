//! One agent per data directory (T-343).
//!
//! The IPC socket's own check ("is someone answering?") is not enough: an agent that is busy
//! or still starting does not answer, and a second one then replaced its socket and took
//! turns with it on the link. The agent takes an exclusive lock on a file of the data
//! directory before touching anything else and keeps it for the life of the process. The OS
//! releases it on any exit, `kill -9` included, so there is no stale lock to clean up.
//!
//! The holder's PID goes in a second file: on Windows the lock covers the whole of the
//! locked file and a reader would be refused.
//!
//! The files are opened by std, which marks them non-inheritable: a tunnel process that
//! outlives the agent does not keep the lock.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use crate::paths::Paths;

/// The lock held by this process; dropping it (or ending the process) releases it.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

#[derive(Debug, thiserror::Error)]
pub enum InstanceError {
    #[error(
        "another tandemd is already running on the data directory {} ({}); this one exits without touching it",
        dir.display(),
        holder(*pid)
    )]
    Held { dir: PathBuf, pid: Option<u32> },
    #[error("could not take the lock {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn holder(pid: Option<u32>) -> String {
    match pid {
        Some(pid) => format!("pid {pid}"),
        None => "pid unknown".to_owned(),
    }
}

/// Takes the data directory's lock, which must already exist.
///
/// If another process holds it, reads its PID and fails without writing anything.
pub fn acquire(paths: &Paths) -> Result<InstanceLock, InstanceError> {
    let path = paths.lock_file();
    let io_err = |source| InstanceError::Io {
        path: path.clone(),
        source,
    };
    // Without truncating: the file carries no data, only the lock.
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(io_err)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(InstanceError::Held {
                dir: paths.data_dir.clone(),
                pid: read_pid(&paths.pid_file()),
            });
        }
        Err(TryLockError::Error(e)) => return Err(io_err(e)),
    }
    // Only informative: if it cannot be written, the next agent says "pid unknown".
    if let Err(e) = fs::write(paths.pid_file(), std::process::id().to_string()) {
        tracing::warn!("could not write {}: {e}", paths.pid_file().display());
    }
    Ok(InstanceLock { _file: file })
}

fn read_pid(path: &Path) -> Option<u32> {
    parse_pid(&fs::read_to_string(path).ok()?)
}

/// The PID in the file's text; anything else (empty, partial, garbage) is "unknown".
fn parse_pid(text: &str) -> Option<u32> {
    text.trim().parse().ok().filter(|pid| *pid != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn paths(dir: &Path) -> Paths {
        Paths {
            config: dir.join("tandem.toml"),
            data_dir: dir.to_owned(),
        }
    }

    #[test]
    fn the_first_one_takes_the_lock_and_writes_its_pid() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(tmp.path());
        let _lock = acquire(&paths).unwrap();
        assert_eq!(read_pid(&paths.pid_file()), Some(std::process::id()));
    }

    #[test]
    fn a_second_one_on_the_same_directory_fails_naming_the_holder() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(tmp.path());
        let _first = acquire(&paths).unwrap();
        let err = acquire(&paths).unwrap_err();
        assert!(
            matches!(&err, InstanceError::Held { pid: Some(pid), .. } if *pid == std::process::id()),
            "{err}"
        );
        assert!(
            err.to_string()
                .contains(&format!("pid {}", std::process::id()))
        );
    }

    #[test]
    fn a_refused_one_leaves_the_holders_pid_file_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(tmp.path());
        let _first = acquire(&paths).unwrap();
        std::fs::write(paths.pid_file(), "4242").unwrap();
        acquire(&paths).unwrap_err();
        assert_eq!(read_pid(&paths.pid_file()), Some(4242));
    }

    #[test]
    fn the_lock_is_released_when_the_holder_goes_away() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(tmp.path());
        drop(acquire(&paths).unwrap());
        let _again = acquire(&paths).unwrap();
    }

    #[test]
    fn each_data_directory_has_its_own_lock() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let _a = acquire(&paths(a.path())).unwrap();
        let _b = acquire(&paths(b.path())).unwrap();
    }

    #[test]
    fn a_missing_pid_file_reads_as_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(tmp.path());
        let _first = acquire(&paths).unwrap();
        std::fs::remove_file(paths.pid_file()).unwrap();
        let err = acquire(&paths).unwrap_err();
        assert!(
            matches!(err, InstanceError::Held { pid: None, .. }),
            "{err}"
        );
        assert!(err.to_string().contains("pid unknown"));
    }

    #[test]
    fn a_lock_file_that_cannot_be_opened_is_an_io_error() {
        let tmp = tempfile::tempdir().unwrap();
        // The directory does not exist.
        let err = acquire(&paths(&tmp.path().join("missing"))).unwrap_err();
        assert!(matches!(err, InstanceError::Io { .. }), "{err}");
    }

    proptest! {
        #[test]
        fn a_pid_survives_the_round_trip_through_the_file(pid in 1..=u32::MAX) {
            prop_assert_eq!(parse_pid(&pid.to_string()), Some(pid));
            prop_assert_eq!(parse_pid(&format!(" {pid}\r\n")), Some(pid));
        }

        #[test]
        fn garbage_never_panics_and_never_gives_pid_zero(text in "\\PC*") {
            prop_assert_ne!(parse_pid(&text), Some(0));
        }
    }
}
