//! Motor de teclado y mouse: captura, inyección, mapa de teclas, bordes.
//!
//! - [`event`]: modelo de eventos portable, tablas de teclas y reasignación de modificadores.
//! - [`geometry`]: pantallas visibles de cada máquina y cruce de bordes entre ellas.
//! - [`macos`]: captura e inyección en macOS (traducción portable + CoreGraphics).
//! - [`session`]: sesión de entrada sobre el enlace (servidor y cliente).
//! - [`windows`]: captura e inyección en Windows (lógica portable; las llamadas al sistema, `cfg(windows)`).

pub mod event;
pub mod geometry;
pub mod macos;
pub mod session;
pub mod windows;

pub use event::{Button, HidUsage, InputEvent, KeyState, ModifierKey, Modifiers, Scroll};
pub use geometry::{
    GeometryError, Layout, LayoutOptions, Motion, Point, Rect, Screen, ScreenId, Side,
};
