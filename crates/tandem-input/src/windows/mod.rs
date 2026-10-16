//! Windows: captura e inyección de entrada.
//!
//! La lógica (traducción, filtros, watchdog) es portable y se prueba en Linux; solo
//! las llamadas al sistema (`SendInput`, hooks, métricas del escritorio) van bajo
//! `cfg(windows)`.
//!
//! - [`capture`]: captura de la entrada local (hooks de bajo nivel y Raw Input).
//! - [`emulate`]: inyección con `SendInput` (scancodes, mouse absoluto, rueda).

pub mod capture;
pub mod emulate;

/// Marca en `dwExtraInfo` de todo lo que inyecta Tandem ("TNDM"), para que la captura
/// pueda descartar sus propios eventos sin depender solo de `LL*HF_INJECTED`.
pub const TANDEM_EXTRA_INFO: usize = 0x544E_444D;
