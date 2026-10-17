//! Sincronización del portapapeles entre los dos agentes (DESIGN §7,
//! docs/research/input-engine.md).
//!
//! - [`ClipContent`]: texto, HTML e imagen, con su [`ContentHash`].
//! - [`ClipSync`]: máquina de estados sin E/S, igual en los dos lados. Anuncia
//!   las copias locales, suprime el eco por hash y pide lo del otro lado cuando
//!   esta máquina tiene el foco (transferencia perezosa).
//! - [`Clipboard`]: lo que cada plataforma implementa (contador de cambios,
//!   lectura y escritura). [`SystemClipboard`] usa `arboard` y el contador del
//!   sistema; [`fake::FakeClipboard`] es el doble de prueba.
//! - [`ClipDriver`]: los une con un stream confiable de `tandem-link`, propio
//!   del portapapeles para que una imagen grande no frene las teclas.

mod clipboard;
pub mod content;
mod driver;
pub mod fake;
pub mod msg;
pub mod sync;
mod system;

pub use clipboard::Clipboard;
pub use content::{ClipContent, ContentHash, Image};
pub use driver::{ClipConfig, ClipDriver, ReadBackoff, SyncEnd};
pub use msg::ClipMsg;
pub use sync::{Action, ClipSync, SyncConfig, SyncStats};
pub use system::{ClipboardError, SystemClipboard};
