//! Windows: captura e inyección de entrada.
//!
//! La traducción de eventos es portable y se prueba en Linux; solo el envío al sistema
//! (`SendInput`, métricas del escritorio) va bajo `#[cfg(windows)]`.
//!
//! - [`emulate`]: inyección con `SendInput` (scancodes, mouse absoluto, rueda).

pub mod emulate;

/// Marca en `dwExtraInfo` de todo lo que inyecta Tandem ("TNDM"), para que la captura
/// pueda descartar sus propios eventos sin depender solo de `LL*HF_INJECTED`.
pub const TANDEM_EXTRA_INFO: usize = 0x544E_444D;
