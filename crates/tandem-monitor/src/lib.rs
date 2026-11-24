//! Monitors in Tandem (DESIGN §10, DECISIONS D11–D13, T-110).
//!
//! All DDC goes through `monitor-ddc` (repo `monitor-firmware`, A5): a single standard, the
//! own firmware's; anything else is "not compatible" and never receives a Set.
//!
//! - [`election`]: who owns each monitor, the same on both machines.
//! - [`directory`]: what is known about each monitor (channels of both, owner, what the
//!   owner read) and the views for the UI.
//! - [`local`]: the monitors this machine sees: probing, events and state of the ones it has
//!   to read, and the input switch with a verdict.
//! - [`ring`]: the monitor's clock (`0xEF`), which wraps around, and the time of each event.
//! - [`guard`]: automatic input rollback for hardware tests, in the process and
//!   with a separate watchdog.
//!
//! Everything is synchronous (DDC blocks): the agent calls it outside the tokio runtime.

pub mod directory;
pub mod election;
pub mod guard;
pub mod local;
pub mod ring;

pub use directory::Directory;
pub use election::elect;
pub use local::{
    LocalConfig, LocalMonitors, NO_VERDICT, NOT_OWNER, edid_input, edid_size_mm, set_edid_input,
    set_edid_size_mm,
};

use monitor_ddc::Input;
use tandem_core::MonitorInput;

/// The `monitor-ddc` input as Tandem names it.
pub fn to_core(input: Input) -> MonitorInput {
    match input {
        Input::DisplayPort => MonitorInput::DisplayPort,
        Input::Hdmi => MonitorInput::Hdmi,
    }
}

/// The inverse of [`to_core`].
pub fn to_ddc(input: MonitorInput) -> Input {
    match input {
        MonitorInput::DisplayPort => Input::DisplayPort,
        MonitorInput::Hdmi => Input::Hdmi,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_convert_both_ways() {
        for i in MonitorInput::ALL {
            assert_eq!(to_core(to_ddc(i)), i);
        }
    }
}
