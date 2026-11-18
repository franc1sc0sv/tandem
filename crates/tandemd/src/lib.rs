//! Agent binary (launchd service / Windows Service).
//!
//! The library exists so the agent's logic can be tested without starting the service.
//!
//! - [`cli`]: command-line arguments.
//! - [`config`]: `tandem.toml` (the shared desk, the network and which machine this is).
//! - [`autoconfig`]: the `tandem.toml` the agent generates on its own if there is none (T-082).
//! - [`config_edit`]: what the agent writes to `tandem.toml` without losing the comments.
//! - [`migrate`]: v0.2 `tandem.toml` (`mac`/`pc` sides) to the A/B sides format.
//! - [`paths`]: where the configuration, state and logs live.
//! - [`store`]: the [`tandem_core::State`] persisted as JSON.
//! - [`agent`]: the agent's live state and the replies to the UI.
//! - [`orchestrator`]: power on and off as transactions split between the two machines.
//! - [`capabilities`]: what each machine has and knows how to do, and what the plan can ask of
//!   each one (T-084).
//! - [`desk_sync`]: the desk's shared configuration between the two machines (T-084).
//! - [`modules`]: each system's modules and their state (active, off, unavailable;
//!   T-098).
//! - [`monitors`]: the desk's monitors (chosen owner, events, changes with a verdict;
//!   T-110).
//! - [`power`]: Wake-on-LAN toward the other machine and clean shutdown of this one.
//! - [`peer_watch`]: considers the other machine off if no link sees it (power
//!   cut or suspended), on both.
//! - [`peer`]: the other agent over the control plane (asking it for steps and serving its own).
//! - [`network`]: QUIC, peer discovery and link manager.
//! - [`input`]: shared keyboard, mouse and clipboard, and where the keyboard is.
//! - [`install`]: `--install` / `--uninstall` (initial config and startup with the session).
//! - [`pairing`]: pairing with the other agent, requested from the UI.
//! - [`ipc`]: the local API for the UI (Unix socket / named pipe).
//! - [`daemon`]: the assembled agent, the main loop and the orderly shutdown.
//! - [`logging`]: `tracing` to stdout and to a file.
//! - `sim` (feature `sim-backends`): simulated steps to test the binary without hardware.
//! - [`test_ports`]: loopback ports for tests with real agents.

pub mod agent;
pub mod autoconfig;
pub mod capabilities;
pub mod cli;
pub mod config;
pub mod config_edit;
pub mod daemon;
pub mod desk_sync;
pub mod drag;
pub mod input;
pub mod install;
pub mod ipc;
pub mod links_cmd;
pub mod logging;
pub mod migrate;
pub mod modules;
pub mod monitors;
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
pub mod test_ports;
pub mod tunnels;

pub use agent::{Agent, PairingControl};
pub use config::{AgentConfig, ConfigError, NetworkConfig, OrchestratorConfig, PowerConfig};
pub use orchestrator::{LocalSteps, NoPeer, Orchestrator, PeerLink, StepTimeouts, Unimplemented};
pub use peer::PeerClient;
pub use power::PowerSteps;
pub use store::{StateStore, StoreError};
pub use tandem_input::session::CursorEvent;
