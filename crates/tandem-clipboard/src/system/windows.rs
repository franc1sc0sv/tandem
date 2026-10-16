use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;

use super::ClipboardError;

/// Número de secuencia del portapapeles de la estación de ventanas: sube con
/// cada cambio, de cualquier app. Se consulta en vez de registrar un
/// `AddClipboardFormatListener`, que necesitaría una ventana y su bucle de
/// mensajes solo para enterarse de lo mismo.
pub(super) struct Counter;

impl Counter {
    pub(super) fn new() -> Self {
        Self
    }

    pub(super) fn get(&mut self, _: &mut arboard::Clipboard) -> Result<u64, ClipboardError> {
        // SAFETY: no recibe nada ni toca memoria nuestra; sin acceso a la
        // estación de ventanas devuelve 0, que el driver toma como "sin cambios".
        Ok(u64::from(unsafe { GetClipboardSequenceNumber() }))
    }
}
