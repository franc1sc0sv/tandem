//! Detección del hub USB-C y roles automáticos (DESIGN §7).
//!
//! El servidor de teclado y mouse es la máquina donde está conectado el hub. Cada
//! agente busca el teclado y el mouse configurados por VID/PID ([`HubConfig`]),
//! con antirrebote para no reaccionar a desconexiones breves, y [`RoleArbiter`]
//! decide qué máquina es el servidor juntando lo de aquí, lo que reporta el otro
//! agente y el selector manual de la UI, que gana sobre la detección.
//!
//! - [`DeviceScanner`]: lo que cada plataforma implementa (IOKit en macOS,
//!   SetupAPI en Windows); [`fake::FakeScanner`] es el doble de prueba.
//! - [`Debouncer`] y [`HubTracker`]: lógica pura, sin E/S ni reloj propio.
//! - [`HubWatcher`]: sondea el escáner y publica la presencia ya estable.

mod debounce;
pub mod fake;
mod id;
mod roles;
mod watcher;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub use debounce::Debouncer;
pub use id::{ParseUsbIdError, UsbId};
pub use roles::{Role, RoleArbiter, Selector};
pub use watcher::{
    DEFAULT_DEBOUNCE, DEFAULT_POLL, DeviceScanner, HubConfig, HubTracker, HubWatcher, Presence,
    WatchConfig,
};

#[cfg(target_os = "macos")]
pub use macos::{IoKitScanner as SystemScanner, ScanError};
#[cfg(windows)]
pub use windows::{ScanError, SetupApiScanner as SystemScanner};
