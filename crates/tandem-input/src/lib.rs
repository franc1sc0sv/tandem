//! Keyboard and mouse engine: capture, injection, key map, edges.
//!
//! - [`event`]: portable event model, key tables and modifier remapping.
//! - [`geometry`]: the screen arrangement of each machine and the edge crossing between them.
//! - [`macos`]: capture and injection on macOS (portable translation + CoreGraphics).
//! - [`session`]: input session over the link (server and client).
//! - [`windows`]: capture and injection on Windows (portable logic; system calls, `cfg(windows)`).

pub mod event;
pub mod geometry;
pub mod macos;
pub mod session;
pub mod windows;

pub use event::{Button, HidUsage, InputEvent, KeyState, ModifierKey, Modifiers, Scroll};
pub use geometry::{
    Align, Arrangement, GeometryError, Layout, LayoutOptions, Motion, Point, Rect, Screen,
    ScreenId, Side, shared_span,
};
