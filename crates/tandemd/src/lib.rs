//! Binario del agente (servicio launchd / Windows Service).
//!
//! La biblioteca existe para que la lógica del agente se pruebe sin arrancar el servicio.
//!
//! - [`cli`]: argumentos de la línea de órdenes.
//! - [`config`]: `tandem.toml` (el puesto compartido, la red y qué máquina es esta).
//! - [`autoconfig`]: el `tandem.toml` que el agente genera solo si no hay ninguno (T-082).
//! - [`config_edit`]: lo que el agente escribe en `tandem.toml` sin perder los comentarios.
//! - [`migrate`]: `tandem.toml` de v0.2 (lados `mac`/`pc`) al formato de lados A/B.
//! - [`paths`]: dónde viven la configuración, el estado y los logs.
//! - [`store`]: el [`tandem_core::State`] persistido en JSON.
//! - [`agent`]: el estado vivo del agente y las respuestas a la UI.
//! - [`orchestrator`]: encender y apagar como transacciones repartidas entre las dos máquinas.
//! - [`capabilities`]: lo que cada máquina tiene y sabe hacer, y lo que el plan puede pedirle
//!   a cada una (T-084).
//! - [`desk_sync`]: la configuración compartida del puesto entre las dos máquinas (T-084).
//! - [`modules`]: los módulos de cada sistema y su estado (activo, apagado, no disponible;
//!   T-098).
//! - [`power`]: Wake-on-LAN hacia la otra máquina y apagado limpio de esta.
//! - [`peer_watch`]: da a la otra máquina por apagada si ningún enlace la ve (corte de
//!   corriente o suspendida), en las dos.
//! - [`peer`]: el otro agente por el plano de control (pedirle pasos y atender los suyos).
//! - [`network`]: QUIC, descubrimiento del par y gestor de enlaces.
//! - [`input`]: teclado, mouse y portapapeles compartidos, y dónde está el teclado.
//! - [`install`]: `--install` / `--uninstall` (config inicial y arranque con la sesión).
//! - [`pairing`]: el emparejamiento con el otro agente, pedido desde la UI.
//! - [`ipc`]: la API local para la UI (Unix socket / named pipe).
//! - [`daemon`]: el agente armado, el bucle principal y el apagado ordenado.
//! - [`logging`]: `tracing` a stdout y a archivo.
//! - `sim` (feature `sim-backends`): pasos simulados para probar el binario sin hardware.

pub mod agent;
pub mod autoconfig;
pub mod capabilities;
pub mod cli;
pub mod config;
pub mod config_edit;
pub mod daemon;
pub mod desk_sync;
pub mod input;
pub mod install;
pub mod ipc;
pub mod logging;
pub mod migrate;
pub mod modules;
pub mod network;
pub mod orchestrator;
pub mod pairing;
pub mod paths;
pub mod peer;
pub mod peer_watch;
pub mod power;
#[cfg(feature = "sim-backends")]
pub mod sim;
pub mod store;

pub use agent::{Agent, PairingControl};
pub use config::{AgentConfig, ConfigError, NetworkConfig, OrchestratorConfig, PowerConfig};
pub use orchestrator::{LocalSteps, NoPeer, Orchestrator, PeerLink, StepTimeouts, Unimplemented};
pub use peer::PeerClient;
pub use power::PowerSteps;
pub use store::{StateStore, StoreError};
