//! Control de monitores:
//!
//! - trait `MonitorSwitcher` + backends (ddc-windows, ddc-macos, manual, kvm).
//! - trait `DisplayTopology` + backends (windows, macos).
//!
//! Por ahora: DDC/CI portable ([`ddc`]), su backend de Windows y la lógica del CLI
//! `tandem-ddc` ([`cli`]).

pub mod cli;
pub mod ddc;

#[cfg(windows)]
pub mod windows;
