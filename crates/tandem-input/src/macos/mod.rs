//! macOS backends of the input engine.
//!
//! The logic that can be portable (event translation, cursor balance, permissions)
//! is portable and is tested on Linux; only the CoreGraphics and AppKit calls
//! go under `#[cfg(target_os = "macos")]`.
//!
//! - [`capture`]: CGEventTap on the server (T-022).
//! - [`emulate`]: injection with `CGEventPost` (T-023, docs/research/input-engine.md).

pub mod capture;
pub mod emulate;
