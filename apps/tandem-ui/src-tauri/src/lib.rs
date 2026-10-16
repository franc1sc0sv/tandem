//! UI de Tandem (Tauri 2).
//!
//! - [`session`]: la conexión viva con `tandemd` que alimenta la escena principal.
//! - [`status`]: lo que la ventana muestra del agente, pedido a `tandemd` por la IPC local.
//! - [`shortcuts`]: los atajos globales, su forma canónica y `ui.toml`.
//! - [`tray`]: el menú del ícono de la barra de menú / bandeja y la mini escena.
//! - `app` (feature `app`): la app de Tauri y sus comandos.

pub mod session;
pub mod shortcuts;
pub mod status;
pub mod tray;

#[cfg(feature = "app")]
pub mod app;
