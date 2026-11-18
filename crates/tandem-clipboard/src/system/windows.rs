use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;

use super::ClipboardError;

/// Clipboard sequence number of the window station: rises with
/// every change, from any app. It is queried instead of registering an
/// `AddClipboardFormatListener`, which would need a window and its message
/// loop just to find out the same thing.
pub(super) struct Counter;

impl Counter {
    pub(super) fn new() -> Self {
        Self
    }

    pub(super) fn get(&mut self, _: &mut arboard::Clipboard) -> Result<u64, ClipboardError> {
        // SAFETY: takes nothing and touches none of our memory; without access to the
        // window station it returns 0, which the driver takes as "no changes".
        Ok(u64::from(unsafe { GetClipboardSequenceNumber() }))
    }
}
