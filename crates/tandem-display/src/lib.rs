//! Control de monitores:
//!
//! - trait `MonitorSwitcher` + backends (ddc-windows, ddc-macos, manual, kvm).
//! - trait `DisplayTopology` + backends (windows, macos).
//!
//! Por ahora: DDC/CI portable ([`ddc`]), la topología portable ([`topology`]), sus
//! backends de Windows, la lógica del CLI `tandem-ddc` ([`cli`]), la topología de
//! macOS ([`quartz`] + `macos`), el cambio de entrada en un cambio de modo
//! ([`switcher`]), el trait del escritorio de cada máquina que usa el ejecutor de
//! pasos ([`desktop`]) y "traer ventanas" de la pantalla integrada de la Mac
//! ([`gather`] + `macos::ax`).

pub mod cli;
pub mod ddc;
pub mod desktop;
pub mod gather;
pub mod quartz;
pub mod switcher;
pub mod topology;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(windows)]
pub mod windows;
