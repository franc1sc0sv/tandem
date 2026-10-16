//! Control de monitores:
//!
//! - trait `MonitorSwitcher` + backends (ddc-windows, ddc-macos, manual, kvm).
//! - trait `DisplayTopology` + backends (windows, macos).
//!
//! Por ahora: DDC/CI portable ([`ddc`]), la topología portable ([`topology`]), sus
//! backends de Windows, la lógica del CLI `tandem-ddc` ([`cli`]) y la topología de
//! macOS ([`quartz`] + `macos`).

pub mod cli;
pub mod ddc;
pub mod quartz;
pub mod topology;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(windows)]
pub mod windows;
