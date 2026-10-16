//! Backends de macOS.

pub mod ax;
pub mod quartz;

pub use ax::{AxApi, gather_builtin_windows};
pub use quartz::CgApi;

/// Topología de pantallas de la Mac sobre CoreGraphics.
pub type MacDisplays = crate::quartz::MacTopology<CgApi>;
