//! Desk simulation, for tests only (docs/TESTING.md).
//!
//! Two real `tandemd` in the same process, each with its orchestrator, its link
//! manager and the real control protocol, joined by an in-memory network with one
//! link per cable (USB4, Ethernet, Wi-Fi) that can be cut. Platform backends are
//! replaced by a simulated physical desk:
//!
//! - [`world`]: each machine's power and the USB hub.
//! - [`steps`]: each machine's steps over that desk.
//! - [`net`]: the links and the `PeerLink` that follows the active link.
//! - [`fault`]: [`FaultPlan`] to fail, hang or delay a step and cut links.
//! - [`desk`]: [`Desk`], which builds everything and checks the desk against the state.
//! - [`monitors`]: the `monitor-ddc` doubles connected to both machines (T-110).
//! - [`wifi`]: [`SimWifi`], a Wi-Fi with bandwidth and a queue, for input priority
//!   over data (T-101).
//!
//! - [`input`]: [`InputPair`], two agents with the input engine over real QUIC on
//!   loopback, for the input latency bench (T-093). There the clock is the real one.
//!
//! Except for [`input`], everything runs with the tokio clock paused: times (PC startup,
//! pings, timeouts) are exact and there are no real waits. The geometry and sessions
//! of the input engine are also tested in `tandemd/tests/input.rs`.

pub mod desk;
pub mod fault;
pub mod input;
pub mod monitors;
pub mod net;
pub mod scenes;
pub mod steps;
pub mod wifi;
pub mod world;

pub use desk::{Desk, DeskBuilder, Outcome, SimError, TxnRun};
pub use fault::{FaultAction, FaultPlan};
pub use input::{InputPair, SimAgent};
pub use monitors::{MachineBus, MonitorSpec, SimMonitors};
pub use scenes::SceneScreens;
pub use steps::HardwareTimes;
pub use wifi::{SimWifi, WifiReader, WifiWriter};
pub use world::{Physical, Power, World};
