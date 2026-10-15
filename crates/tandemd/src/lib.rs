//! Binario del agente (servicio launchd / Windows Service).
//!
//! La biblioteca existe para que la lógica del agente se pruebe sin arrancar el servicio.
//!
//! - [`cli`]: argumentos de la línea de órdenes.
//! - [`config`]: `tandem.toml` (el puesto, la red y qué máquina es esta).
//! - [`paths`]: dónde viven la configuración, el estado y los logs.
//! - [`store`]: el [`tandem_core::State`] persistido en JSON.
//! - [`agent`]: el estado vivo del agente y las respuestas a la UI.
//! - [`ipc`]: la API local para la UI (Unix socket / named pipe).
//! - [`daemon`]: el bucle principal y el apagado ordenado.
//! - [`logging`]: `tracing` a stdout y a archivo.

pub mod agent;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod ipc;
pub mod logging;
pub mod paths;
pub mod store;

pub use agent::Agent;
pub use config::{AgentConfig, ConfigError, NetworkConfig};
pub use store::{StateStore, StoreError};
