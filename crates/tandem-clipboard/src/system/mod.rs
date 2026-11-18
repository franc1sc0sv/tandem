//! Real clipboard: reading and writing with `arboard`, and each system's
//! change counter.

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
    #[error("clipboard: {0}")]
    Arboard(#[from] arboard::Error),
    #[error("image of {width}x{height} with {len} bytes")]
    BadImage {
        width: usize,
        height: usize,
        len: usize,
    },
}

/// This machine's clipboard.
///
/// On Windows the clipboard belongs to the user's session desktop: the
/// SYSTEM service does not see it, so this runs in the desktop helper
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

/// A missing format is not an error: the clipboard may have only
/// some. Being busy with another app is (it is retried on the next change).
fn optional<T>(r: Result<T, arboard::Error>) -> Result<Option<T>, ClipboardError> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e @ arboard::Error::ClipboardOccupied) => Err(e.into()),
        Err(e) => {
            tracing::trace!("format not available: {e}");
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

/// `arboard` replaces the whole clipboard on each write, so text and
/// image cannot be set together: text (with its HTML) takes priority,
/// because a copy with both is usually from a document.
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

    /// Touches the machine's real clipboard: by hand only.
    #[test]
    #[ignore = "hardware"]
    fn round_trip_with_the_real_clipboard() {
        let mut cb = SystemClipboard::new().unwrap();
        let before = cb.change_count().unwrap();
        let text = ClipContent {
            text: Some("tandem \u{fc}".into()),
            html: Some("<b>tandem \u{fc}</b>".into()),
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
    fn arboard_image_with_mismatched_bytes_is_an_error() {
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
