use objc2_app_kit::NSPasteboard;

use super::ClipboardError;

/// `changeCount` of the general pasteboard: rises with every change, from any app.
pub(super) struct Counter;

impl Counter {
    pub(super) fn new() -> Self {
        Self
    }

    pub(super) fn get(&mut self, _: &mut arboard::Clipboard) -> Result<u64, ClipboardError> {
        // Negative does not happen: the counter starts at 0 and only rises.
        Ok(NSPasteboard::generalPasteboard().changeCount() as u64)
    }
}
