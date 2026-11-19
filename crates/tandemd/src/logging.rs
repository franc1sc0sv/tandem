//! `tracing` to stdout and to `<data>/logs/tandemd.log`.
//!
//! The level comes from `TANDEM_LOG` (`EnvFilter` syntax, for example `tandemd=debug`);
//! by default, `info`.

use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

pub const ENV_VAR: &str = "TANDEM_LOG";
const FILE: &str = "tandemd.log";

/// Returns the log file's path.
pub fn init(log_dir: &Path) -> io::Result<PathBuf> {
    crate::paths::create_private_dir(log_dir)?;
    let path = log_dir.join(FILE);
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let filter = EnvFilter::try_from_env(ENV_VAR).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .with(fmt::layer().with_ansi(false).with_writer(Mutex::new(file)))
        .try_init()
        .map_err(io::Error::other)?;
    Ok(path)
}
