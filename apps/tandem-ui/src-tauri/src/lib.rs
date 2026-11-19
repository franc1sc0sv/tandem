//! Tandem UI (Tauri 2).
//!
//! - [`session`]: the live connection to `tandemd` that feeds the main scene.
//! - [`status`]: what the window shows of the agent, requested from `tandemd` over the local IPC.
//! - [`shortcuts`]: the global shortcuts, their canonical form and `ui.toml`.
//! - [`tray`]: the menu bar / tray icon menu and the mini scene.
//! - [`launch_agent`]: the macOS LaunchAgent that starts `tandemd` at login.
//! - [`setup`]: the first launch on macOS (permissions guide).
//! - [`update`]: updating both machines (versions, plan and "Update automatically").
//! - `app` (feature `app`): the Tauri app and its commands.

pub mod admin_service;
pub mod files;
pub mod launch_agent;
pub mod session;
pub mod setup;
pub mod shortcuts;
pub mod status;
pub mod tray;
pub mod tunnels;
pub mod update;

#[cfg(feature = "app")]
pub mod app;
#[cfg(feature = "app")]
pub mod updater;
