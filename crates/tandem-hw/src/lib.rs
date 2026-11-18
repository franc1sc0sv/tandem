//! Hardware test suite (T-123).
//!
//! Two pieces: the orchestrator (`scripts/hw/run.sh`), which builds, installs and starts test
//! agents on both machines, and this crate, which runs on each machine: it provisions the
//! test agent's identity and config, runs the checks by talking to its IPC, and writes one
//! result per line (JSON) that the orchestrator gathers into the report.
//!
//! None of this runs in CI: anything that touches devices is `#[ignore = "hardware"]` and the
//! binary is only launched by the orchestrator.

pub mod checks;
pub mod conn;
pub mod latency;
pub mod monitor;
pub mod provision;
pub mod report;
