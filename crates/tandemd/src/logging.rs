//! `tracing` a stdout y a `<datos>/logs/tandemd.log`.
//!
//! El nivel sale de `TANDEM_LOG` (sintaxis de `EnvFilter`, por ejemplo `tandemd=debug`);
//! por omisión, `info`.

use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

pub const ENV_VAR: &str = "TANDEM_LOG";
const FILE: &str = "tandemd.log";

/// Devuelve la ruta del archivo de log.
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
