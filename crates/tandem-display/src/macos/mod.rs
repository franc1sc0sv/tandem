//! Backends de macOS.

pub mod quartz;

pub use quartz::CgApi;

/// Topología de pantallas de la Mac sobre CoreGraphics.
pub type MacDisplays = crate::quartz::MacTopology<CgApi>;
