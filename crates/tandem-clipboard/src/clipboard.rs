use std::fmt::Display;

use crate::content::ClipContent;

/// The system clipboard, as seen by the driver.
///
/// Calls are blocking but short, except reading or writing a large
/// image.
pub trait Clipboard: Send {
    type Error: Display;

    /// A number that changes every time the clipboard changes, whoever
    /// changes it (`changeCount` of NSPasteboard, `GetClipboardSequenceNumber` on
    /// Windows). Reading it is cheap: the driver checks it often and only reads the
    /// content when it changes.
    fn change_count(&mut self) -> Result<u64, Self::Error>;

    /// What it holds now, in the formats that are synchronized; `None` if it holds
    /// none.
    fn read(&mut self) -> Result<Option<ClipContent>, Self::Error>;

    /// Replaces the content.
    fn write(&mut self, content: &ClipContent) -> Result<(), Self::Error>;
}
