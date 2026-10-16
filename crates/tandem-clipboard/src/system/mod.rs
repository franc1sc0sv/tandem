//! Portapapeles real: lectura y escritura con `arboard`, y el contador de cambios
//! de cada sistema.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(windows, target_os = "macos")))]
mod other;
#[cfg(windows)]
mod windows;

use arboard::ImageData;

use crate::clipboard::Clipboard;
use crate::content::{ClipContent, Image};

#[cfg(target_os = "macos")]
use macos::Counter;
#[cfg(not(any(windows, target_os = "macos")))]
use other::Counter;
#[cfg(windows)]
use windows::Counter;

#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    #[error("portapapeles: {0}")]
    Arboard(#[from] arboard::Error),
    #[error("imagen de {width}x{height} con {len} bytes")]
    BadImage {
        width: usize,
        height: usize,
        len: usize,
    },
}

/// El portapapeles de esta máquina.
///
/// En Windows el portapapeles es del escritorio de la sesión del usuario: el
/// servicio SYSTEM no lo ve, así que esto corre en el helper del escritorio
/// (DESIGN §7).
pub struct SystemClipboard {
    inner: arboard::Clipboard,
    counter: Counter,
}

impl SystemClipboard {
    pub fn new() -> Result<Self, ClipboardError> {
        Ok(Self {
            inner: arboard::Clipboard::new()?,
            counter: Counter::new(),
        })
    }
}

/// Un formato que falta no es un error: el portapapeles puede tener solo
/// algunos. Que esté ocupado por otra app sí (se reintenta en el próximo cambio).
fn optional<T>(r: Result<T, arboard::Error>) -> Result<Option<T>, ClipboardError> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e @ arboard::Error::ClipboardOccupied) => Err(e.into()),
        Err(e) => {
            tracing::trace!("formato no disponible: {e}");
            Ok(None)
        }
    }
}

pub(crate) fn read_arboard(
    cb: &mut arboard::Clipboard,
) -> Result<Option<ClipContent>, ClipboardError> {
    let text = optional(cb.get().text())?;
    let html = optional(cb.get().html())?;
    let image = match optional(cb.get().image())? {
        Some(img) => Some(to_image(img)?),
        None => None,
    };
    Ok(ClipContent { text, html, image }.normalized())
}

fn to_image(img: ImageData<'_>) -> Result<Image, ClipboardError> {
    let bad = || ClipboardError::BadImage {
        width: img.width,
        height: img.height,
        len: img.bytes.len(),
    };
    let width = u32::try_from(img.width).map_err(|_| bad())?;
    let height = u32::try_from(img.height).map_err(|_| bad())?;
    let image = Image {
        width,
        height,
        rgba: img.bytes.to_vec(),
    };
    if image.is_valid() {
        Ok(image)
    } else {
        Err(bad())
    }
}

/// `arboard` reemplaza todo el portapapeles en cada escritura, así que no se
/// pueden poner texto e imagen juntos: el texto (con su HTML) tiene prioridad,
/// porque una copia con los dos suele ser de un documento.
pub(crate) fn write_arboard(
    cb: &mut arboard::Clipboard,
    content: &ClipContent,
) -> Result<(), ClipboardError> {
    match content {
        ClipContent {
            text: Some(text),
            html: Some(html),
            ..
        } => cb.set_html(html.as_str(), Some(text.as_str()))?,
        ClipContent {
            text: Some(text), ..
        } => cb.set_text(text.as_str())?,
        ClipContent {
            image: Some(img), ..
        } => cb.set_image(ImageData {
            width: img.width as usize,
            height: img.height as usize,
            bytes: img.rgba.as_slice().into(),
        })?,
        ClipContent {
            html: Some(html), ..
        } => cb.set_html(html.as_str(), None::<&str>)?,
        ClipContent { .. } => {}
    }
    Ok(())
}

impl Clipboard for SystemClipboard {
    type Error = ClipboardError;

    fn change_count(&mut self) -> Result<u64, ClipboardError> {
        self.counter.get(&mut self.inner)
    }

    fn read(&mut self) -> Result<Option<ClipContent>, ClipboardError> {
        read_arboard(&mut self.inner)
    }

    fn write(&mut self, content: &ClipContent) -> Result<(), ClipboardError> {
        write_arboard(&mut self.inner, content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Toca el portapapeles real de la máquina: solo a mano.
    #[test]
    #[ignore = "hardware"]
    fn ida_y_vuelta_con_el_portapapeles_real() {
        let mut cb = SystemClipboard::new().unwrap();
        let before = cb.change_count().unwrap();
        let text = ClipContent {
            text: Some("tandem ñ".into()),
            html: Some("<b>tandem ñ</b>".into()),
            image: None,
        };
        cb.write(&text).unwrap();
        assert_ne!(cb.change_count().unwrap(), before);
        let back = cb.read().unwrap().unwrap();
        assert_eq!(back.text, text.text);
        assert!(back.html.unwrap().contains("tandem"));

        let img = ClipContent {
            image: Some(Image {
                width: 2,
                height: 1,
                rgba: vec![255, 0, 0, 255, 0, 0, 255, 255],
            }),
            ..ClipContent::default()
        };
        cb.write(&img).unwrap();
        assert_eq!(cb.read().unwrap().unwrap().image, img.image);
    }

    #[test]
    fn imagen_de_arboard_con_bytes_que_no_cuadran_es_error() {
        let img = ImageData {
            width: 2,
            height: 2,
            bytes: vec![0u8; 15].into(),
        };
        assert!(matches!(
            to_image(img),
            Err(ClipboardError::BadImage { len: 15, .. })
        ));
        let ok = ImageData {
            width: 1,
            height: 2,
            bytes: vec![9u8; 8].into(),
        };
        assert_eq!(to_image(ok).unwrap().rgba, vec![9u8; 8]);
    }
}
