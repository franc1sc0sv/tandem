//! Backends de macOS del motor de entrada.
//!
//! La lógica que se puede (traducción de eventos, balance del cursor, permisos)
//! es portable y se prueba en Linux; solo las llamadas a CoreGraphics y AppKit
//! van bajo `#[cfg(target_os = "macos")]`.
//!
//! - [`capture`]: CGEventTap en el servidor (T-022).
//! - [`emulate`]: inyección con `CGEventPost` (T-023, docs/research/input-engine.md).

pub mod capture;
pub mod emulate;
