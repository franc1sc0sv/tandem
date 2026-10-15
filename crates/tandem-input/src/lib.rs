//! Motor de teclado y mouse: captura, inyección, mapa de teclas, bordes.
//!
//! - [`event`]: modelo de eventos portable, tablas de teclas y reasignación de modificadores.

pub mod event;

pub use event::{Button, HidUsage, InputEvent, KeyState, ModifierKey, Modifiers, Scroll};
