//! UI de Tandem (Tauri 2).
//!
//! - [`session`]: la conexión viva con `tandemd` que alimenta la escena principal.
//! - [`status`]: lo que la ventana muestra del agente, pedido a `tandemd` por la IPC local.
//! - [`shortcuts`]: los atajos globales, su forma canónica y `ui.toml`.
//! - [`tray`]: el menú del ícono de la barra de menú / bandeja y la mini escena.
//! - [`launch_agent`]: el LaunchAgent de macOS que arranca `tandemd` al iniciar sesión.
//! - [`setup`]: el primer arranque en macOS (guía de permisos).
//! - [`update`]: actualizar las dos máquinas (versiones, plan y «Actualizar solo»).
//! - `app` (feature `app`): la app de Tauri y sus comandos.

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
