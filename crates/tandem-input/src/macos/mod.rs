//! macOS backends of the input engine.
//!
//! The logic that can be portable (event translation, cursor balance, permissions)
//! is portable and is tested on Linux; only the CoreGraphics and AppKit calls
//! go under `#[cfg(target_os = "macos")]`.
//!
//! - [`capture`]: CGEventTap on the server (T-022).
//! - [`emulate`]: injection with `CGEventPost` (T-023, docs/research/input-engine.md).
//! - `display_power`: turning a display off and on (T-341).
//! - `hotplug`: display changes (T-349).
//! - `iokit`: the lid and the EDID of every external display (T-353).

pub mod capture;
#[cfg(target_os = "macos")]
pub mod display_power;
pub mod emulate;
#[cfg(target_os = "macos")]
pub mod hotplug;
#[cfg(target_os = "macos")]
pub mod iokit;

/// Tag that Tandem's injection writes into `kCGEventSourceUserData` of every event it
/// posts ("TNDM"). The tap lets tagged events through without forwarding them, so what
/// Tandem injects on this Mac never comes back as local input (T-241). Windows gets the
/// same guard from `LLMHF_INJECTED` and Raw Input without a device.
pub const INJECTED_MARKER: i64 = 0x544E_444D;

#[cfg(test)]
mod tests;
