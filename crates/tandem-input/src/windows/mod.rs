//! Windows: input capture and injection.
//!
//! The logic (translation, filters, watchdog) is portable and is tested on Linux; only
//! the system calls (`SendInput`, hooks, desktop metrics) go under
//! `cfg(windows)`.
//!
//! - [`capture`]: capture of local input (low-level hooks and Raw Input).
//! - [`emulate`]: injection with `SendInput` (scancodes, absolute mouse, wheel).

pub mod capture;
pub mod emulate;

/// Mark in `dwExtraInfo` of everything Tandem injects ("TNDM"), so that the capture
/// can discard its own events without depending only on `LL*HF_INJECTED`.
pub const TANDEM_EXTRA_INFO: usize = 0x544E_444D;
