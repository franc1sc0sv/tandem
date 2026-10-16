use objc2_app_kit::NSPasteboard;

use super::ClipboardError;

/// `changeCount` del pasteboard general: sube con cada cambio, de cualquier app.
pub(super) struct Counter;

impl Counter {
    pub(super) fn new() -> Self {
        Self
    }

    pub(super) fn get(&mut self, _: &mut arboard::Clipboard) -> Result<u64, ClipboardError> {
        // Negativo no pasa: el contador empieza en 0 y solo sube.
        Ok(NSPasteboard::generalPasteboard().changeCount() as u64)
    }
}
