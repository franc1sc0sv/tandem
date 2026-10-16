//! Backends de macOS.
//!
//! La traducción de eventos es portable y se prueba en Linux; solo las llamadas a
//! CoreGraphics y AppKit van bajo `#[cfg(target_os = "macos")]`.
//!
//! - [`emulate`]: inyección con `CGEventPost` (docs/research/input-engine.md).

pub mod emulate;
