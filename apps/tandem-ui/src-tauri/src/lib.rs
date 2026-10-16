//! UI de Tandem (Tauri 2).
//!
//! - [`status`]: lo que la ventana muestra del agente, pedido a `tandemd` por la IPC local.
//! - `app` (feature `app`): la app de Tauri y sus comandos.

pub mod status;

#[cfg(feature = "app")]
pub mod app;
