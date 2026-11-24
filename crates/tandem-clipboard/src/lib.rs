//! Clipboard synchronization between the two agents (DESIGN §7,
//! docs/research/input-engine.md).
//!
//! - [`ClipContent`]: text, HTML and image, with its [`ContentHash`].
//! - [`ClipSync`]: state machine without I/O, the same on both sides. Announces
//!   local copies, suppresses the echo by hash and asks for the other side's content when
//!   this machine has focus (lazy transfer).
//! - [`Clipboard`]: what each platform implements (change counter,
//!   read and write). [`SystemClipboard`] uses `arboard` and the system
//!   counter; [`fake::FakeClipboard`] is the test double.
//! - [`ClipDriver`]: joins them with a reliable `tandem-link` stream, dedicated
//!   to the clipboard so a large image does not stall the keys.

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
