//! Binario del agente (servicio launchd / Windows Service).
//!
//! La biblioteca existe para que la lógica del agente se pruebe sin arrancar el servicio.
//!
//! - [`cli`]: argumentos de la línea de órdenes.
//! - [`config`]: `tandem.toml` (el puesto, la red y qué máquina es esta).
//! - [`paths`]: dónde viven la configuración, el estado y los logs.
//! - [`store`]: el [`tandem_core::State`] persistido en JSON.
//! - [`agent`]: el estado vivo del agente y las respuestas a la UI.
//! - [`orchestrator`]: los cambios de modo como transacciones repartidas entre las dos máquinas.
//! - [`display`]: los pasos de pantalla de esta máquina (entradas, escritorio, pantalla integrada).
//! - [`power`]: Wake-on-LAN en la Mac y apagado limpio en la PC.
//! - [`pc_watch`]: en la Mac, da la PC por apagada si ningún enlace la ve (corte de corriente).
//! - [`peer`]: el otro agente por el plano de control (pedirle pasos y atender los suyos).
//! - [`ipc`]: la API local para la UI (Unix socket / named pipe).
//! - [`daemon`]: el bucle principal y el apagado ordenado.
//! - [`logging`]: `tracing` a stdout y a archivo.

pub mod agent;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod display;
pub mod ipc;
pub mod logging;
pub mod orchestrator;
pub mod paths;
pub mod pc_watch;
pub mod peer;
pub mod power;
pub mod store;

pub use agent::Agent;
pub use config::{AgentConfig, ConfigError, NetworkConfig, OrchestratorConfig, PowerConfig};
pub use display::DisplaySteps;
pub use orchestrator::{LocalSteps, NoPeer, Orchestrator, PeerLink, StepTimeouts, Unimplemented};
pub use peer::PeerClient;
pub use power::PowerSteps;
pub use store::{StateStore, StoreError};
