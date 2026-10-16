//! UI de Tandem (Tauri 2).
//!
//! - [`session`]: la conexión viva con `tandemd` que alimenta la escena principal.
//! - [`status`]: lo que la ventana muestra del agente, pedido a `tandemd` por la IPC local.
//! - `app` (feature `app`): la app de Tauri y sus comandos.

pub mod session;
pub mod status;

#[cfg(feature = "app")]
pub mod app;
