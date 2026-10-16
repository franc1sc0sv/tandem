//! Binario del agente (servicio launchd / Windows Service).
//!
//! La biblioteca existe para que la lógica del agente se pruebe sin arrancar el servicio.
//!
//! - [`cli`]: argumentos de la línea de órdenes.
//! - [`config`]: `tandem.toml` (el puesto, la red y qué máquina es esta).
//! - [`autoconfig`]: el `tandem.toml` que el agente genera solo si no hay ninguno (T-082).
//! - [`config_edit`]: lo que el agente escribe en `tandem.toml` sin perder los comentarios.
//! - [`paths`]: dónde viven la configuración, el estado y los logs.
//! - [`store`]: el [`tandem_core::State`] persistido en JSON.
//! - [`agent`]: el estado vivo del agente y las respuestas a la UI.
//! - [`orchestrator`]: los cambios de modo como transacciones repartidas entre las dos máquinas.
//! - [`display`]: los pasos de pantalla de esta máquina (entradas, escritorio, pantalla integrada).
//! - [`calibration`]: la sonda de calibración DDC y el `Confirmer` que la muestra en la
//!   máquina que debería verse (T-078).
//! - [`add_monitor`]: «Agregar monitor» (T-080): lista los monitores del bus DDC, los
//!   calibra, guarda el perfil y actualiza la configuración en caliente.
//! - [`power`]: Wake-on-LAN en la Mac y apagado limpio en la PC.
//! - [`pc_watch`]: en la Mac, da la PC por apagada si ningún enlace la ve (corte de corriente).
//! - [`peer`]: el otro agente por el plano de control (pedirle pasos y atender los suyos).
//! - [`network`]: QUIC, descubrimiento del par y gestor de enlaces.
//! - [`input`]: teclado, mouse y portapapeles compartidos, y quién es el servidor.
//! - [`install`]: `--install` / `--uninstall` (config inicial y arranque con la sesión).
//! - [`pairing`]: el emparejamiento con el otro agente, pedido desde la UI.
//! - [`ipc`]: la API local para la UI (Unix socket / named pipe).
//! - [`daemon`]: el agente armado, el bucle principal y el apagado ordenado.
//! - [`logging`]: `tracing` a stdout y a archivo.
//! - `sim` (feature `sim-backends`): pasos simulados para probar el binario sin hardware.

pub mod add_monitor;
pub mod agent;
pub mod autoconfig;
pub mod calibration;
pub mod cli;
pub mod config;
pub mod config_edit;
pub mod daemon;
pub mod display;
pub mod input;
pub mod install;
pub mod ipc;
pub mod logging;
pub mod network;
pub mod orchestrator;
pub mod pairing;
pub mod paths;
pub mod pc_watch;
pub mod peer;
pub mod power;
#[cfg(feature = "sim-backends")]
pub mod sim;
pub mod store;

pub use agent::{Agent, PairingControl};
pub use calibration::{AgentConfirmer, ProbeBoard};
pub use config::{AgentConfig, ConfigError, NetworkConfig, OrchestratorConfig, PowerConfig};
pub use display::DisplaySteps;
pub use orchestrator::{LocalSteps, NoPeer, Orchestrator, PeerLink, StepTimeouts, Unimplemented};
pub use peer::PeerClient;
pub use power::PowerSteps;
pub use store::{StateStore, StoreError};
