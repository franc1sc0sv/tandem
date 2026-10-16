//! Control de monitores:
//!
//! - trait `MonitorSwitcher` + backends (ddc-windows, ddc-macos, manual, kvm).
//! - trait `DisplayTopology` + backends (windows, macos).
//!
//! Estructura:
//!
//! - [`ddc`]: DDC/CI portable (EDID, capacidades, cambio de entrada) y [`cli`], la lógica
//!   del CLI `tandem-ddc`.
//! - [`switcher`]: el cambio de entrada en un cambio de modo (`MonitorSwitcher`: DDC,
//!   manual o por monitor).
//! - [`topology`]: el trait único [`topology::DisplayTopology`] (qué monitores forman
//!   parte del escritorio, con espejo opcional), su `Display` y su `TopologyError` comunes,
//!   la búsqueda del monitor de la configuración y las operaciones de un cambio de modo
//!   (`attach`, `detach`, `set_builtin`). Incluye el planificador de rutas de Windows y su
//!   doble `FakeTopology`.
//! - [`quartz`]: `MacTopology`, el backend de macOS en código portable sobre las primitivas
//!   `QuartzApi`, con su doble `FakeQuartz`.
//! - [`desktop`]: `DesktopControl`, lo que usa el ejecutor de pasos, implementado una vez
//!   sobre el trait; `desktop::native` elige el backend real de cada sistema.
//! - `windows` y `macos`: solo FFI (`dxva2`/`SetDisplayConfig`, CoreGraphics), cada uno
//!   detrás de su `cfg`.
//! - [`gather`]: "traer ventanas" de la pantalla integrada de la Mac (con `macos::ax`).

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
