//! USB-C hub detection and automatic roles (DESIGN §7).
//!
//! The keyboard and mouse server is the machine where the hub is connected. Each
//! agent looks for the configured keyboard and mouse by VID/PID ([`HubConfig`]),
//! with debounce so it does not react to brief disconnections, and [`RoleArbiter`]
//! decides which machine is the server by combining what is here, what the other
//! agent reports and the UI's manual selector, which wins over detection.
//!
//! - [`DeviceScanner`]: what each platform implements (IOKit on macOS,
//!   SetupAPI on Windows); [`fake::FakeScanner`] is the test double.
//! - [`Debouncer`] and [`HubTracker`]: pure logic, without I/O or its own clock.
//! - [`HubWatcher`]: polls the scanner and publishes the already stable presence.

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
pub use roles::{PeerReport, Role, RoleArbiter, Selector};
pub use watcher::{
    DEFAULT_DEBOUNCE, DEFAULT_POLL, DeviceScanner, HubConfig, HubTracker, HubWatcher, Presence,
    WatchConfig,
};

#[cfg(target_os = "macos")]
pub use macos::{IoKitScanner as SystemScanner, ScanError};
#[cfg(windows)]
pub use windows::{ScanError, SetupApiScanner as SystemScanner};
