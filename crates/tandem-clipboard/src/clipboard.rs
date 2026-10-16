use std::fmt::Display;

use crate::content::ClipContent;

/// El portapapeles del sistema, visto por el driver.
///
/// Las llamadas son bloqueantes pero cortas, salvo leer o escribir una imagen
/// grande.
pub trait Clipboard: Send {
    type Error: Display;

    /// Un número que cambia cada vez que cambia el portapapeles, lo cambie quien
    /// lo cambie (`changeCount` de NSPasteboard, `GetClipboardSequenceNumber` en
    /// Windows). Leerlo es barato: el driver lo consulta seguido y solo lee el
    /// contenido cuando cambia.
    fn change_count(&mut self) -> Result<u64, Self::Error>;

    /// Lo que tiene ahora, en los formatos que se sincronizan; `None` si no tiene
    /// ninguno.
    fn read(&mut self) -> Result<Option<ClipContent>, Self::Error>;

    /// Reemplaza el contenido.
    fn write(&mut self, content: &ClipContent) -> Result<(), Self::Error>;
}
