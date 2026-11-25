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
#[cfg(windows)]
pub mod hotplug;

/// Mark in `dwExtraInfo` of everything Tandem injects ("TNDM"), so that the capture
/// can discard its own events without depending only on `LL*HF_INJECTED`.
pub const TANDEM_EXTRA_INFO: usize = 0x544E_444D;

/// Raises the calling thread above ordinary work (`THREAD_PRIORITY_HIGHEST`): the
/// injection thread (T-358). Not time-critical: it must never starve the hooks' thread.
#[cfg(windows)]
pub fn raise_thread_priority() {
    use ::windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
    };
    // SAFETY: pseudo-handle of the current thread; no pointers.
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) };
}
