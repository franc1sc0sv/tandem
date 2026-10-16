//! Lo que se sincroniza: el contenido del portapapeles en los formatos de v1
//! (DESIGN §7: texto, texto enriquecido e imagen) y su hash.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Contenido del portapapeles. Una copia puede traer varios formatos a la vez
/// (texto y HTML desde un navegador, por ejemplo); se guardan todos.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipContent {
    pub text: Option<String>,
    /// HTML del texto enriquecido.
    pub html: Option<String>,
    pub image: Option<Image>,
}

/// Imagen en RGBA de 8 bits, fila por fila, como la da y la toma `arboard`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Image({}x{}, {} bytes)",
            self.width,
            self.height,
            self.rgba.len()
        )
    }
}

impl Image {
    /// Si los bytes alcanzan exactamente para `width × height` píxeles RGBA.
    pub fn is_valid(&self) -> bool {
        (self.width as u64)
            .checked_mul(self.height as u64)
            .and_then(|px| px.checked_mul(4))
            .is_some_and(|len| len == self.rgba.len() as u64)
    }
}

/// SHA-256 de un [`ClipContent`]: identifica el contenido para no devolver como
/// cambio nuevo lo que llegó del otro lado (supresión de eco) y para comprobar
/// una transferencia.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContentHash(pub [u8; 32]);

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash(")?;
        for b in &self.0[..6] {
            write!(f, "{b:02x}")?;
        }
        write!(f, "…)")
    }
}

impl ClipContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Self::default()
        }
    }

    /// Quita los formatos vacíos; `None` si no queda nada que sincronizar.
    ///
    /// Una imagen con medidas que no cuadran con sus bytes se descarta: escribirla
    /// en el otro sistema fallaría o daría basura.
    pub fn normalized(mut self) -> Option<Self> {
        self.text = self.text.filter(|t| !t.is_empty());
        self.html = self.html.filter(|h| !h.is_empty());
        self.image = self
            .image
            .filter(|i| i.width > 0 && i.height > 0 && i.is_valid());
        (!self.is_empty()).then_some(self)
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.html.is_none() && self.image.is_none()
    }

    /// Hash del contenido. Cada formato entra con su etiqueta y su largo, así dos
    /// contenidos distintos nunca se codifican igual (texto "ab" + HTML "c" no
    /// choca con texto "a" + HTML "bc").
    pub fn hash(&self) -> ContentHash {
        let mut h = Sha256::new();
        let mut field = |tag: u8, bytes: Option<&[u8]>| {
            if let Some(bytes) = bytes {
                h.update([tag]);
                h.update((bytes.len() as u64).to_le_bytes());
                h.update(bytes);
            }
        };
        field(1, self.text.as_deref().map(str::as_bytes));
        field(2, self.html.as_deref().map(str::as_bytes));
        if let Some(img) = &self.image {
            let mut dims = [0u8; 8];
            dims[..4].copy_from_slice(&img.width.to_le_bytes());
            dims[4..].copy_from_slice(&img.height.to_le_bytes());
            field(3, Some(&dims));
            field(4, Some(&img.rgba));
        }
        ContentHash(h.finalize().into())
    }

    /// Lo que mide aproximadamente en el cable, sin codificarlo.
    pub fn approx_len(&self) -> usize {
        self.text.as_ref().map_or(0, String::len)
            + self.html.as_ref().map_or(0, String::len)
            + self.image.as_ref().map_or(0, |i| i.rgba.len())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use proptest::prelude::*;

    use super::*;

    fn image(w: u32, h: u32) -> Image {
        Image {
            width: w,
            height: h,
            rgba: vec![7; (w * h * 4) as usize],
        }
    }

    #[test]
    fn normalizar_quita_lo_vacio() {
        assert_eq!(ClipContent::default().normalized(), None);
        assert_eq!(ClipContent::text("").normalized(), None);
        let c = ClipContent {
            text: Some("hola".into()),
            html: Some(String::new()),
            image: Some(Image {
                width: 2,
                height: 2,
                rgba: vec![0; 3],
            }),
        };
        assert_eq!(c.normalized(), Some(ClipContent::text("hola")));
        let img = ClipContent {
            image: Some(image(2, 3)),
            ..ClipContent::default()
        };
        assert_eq!(img.clone().normalized(), Some(img));
    }

    #[test]
    fn imagen_valida_solo_si_los_bytes_cuadran() {
        assert!(image(3, 5).is_valid());
        let mut i = image(3, 5);
        i.rgba.pop();
        assert!(!i.is_valid());
        let huge = Image {
            width: u32::MAX,
            height: u32::MAX,
            rgba: Vec::new(),
        };
        assert!(!huge.is_valid());
    }

    #[test]
    fn el_hash_separa_los_formatos() {
        let a = ClipContent {
            text: Some("ab".into()),
            html: Some("c".into()),
            image: None,
        };
        let b = ClipContent {
            text: Some("a".into()),
            html: Some("bc".into()),
            image: None,
        };
        let c = ClipContent {
            text: None,
            html: Some("abc".into()),
            image: None,
        };
        assert_ne!(a.hash(), b.hash());
        assert_ne!(a.hash(), c.hash());
        assert_ne!(ClipContent::text("abc").hash(), c.hash());
        // Mismas bytes de imagen con otras medidas: otra imagen.
        let mut w = image(2, 3);
        let h = image(3, 2);
        assert_ne!(
            ClipContent {
                image: Some(w.clone()),
                ..Default::default()
            }
            .hash(),
            ClipContent {
                image: Some(h),
                ..Default::default()
            }
            .hash()
        );
        w.rgba[0] = 8;
        assert_ne!(
            ClipContent {
                image: Some(w),
                ..Default::default()
            }
            .hash(),
            ClipContent {
                image: Some(image(2, 3)),
                ..Default::default()
            }
            .hash()
        );
    }

    pub(crate) fn content() -> impl Strategy<Value = ClipContent> {
        let img = (1u32..6, 1u32..6, any::<u8>()).prop_map(|(w, h, b)| Image {
            width: w,
            height: h,
            rgba: vec![b; (w * h * 4) as usize],
        });
        (
            proptest::option::of(".{0,40}"),
            proptest::option::of(".{0,40}"),
            proptest::option::of(img),
        )
            .prop_map(|(text, html, image)| ClipContent { text, html, image })
    }

    proptest! {
        #[test]
        fn hash_igual_sii_contenido_igual(a in content(), b in content()) {
            prop_assert_eq!(a.hash() == b.hash(), a == b);
        }

        #[test]
        fn ida_y_vuelta_por_el_cable(c in content()) {
            let bytes = tandem_proto::encode(&c).unwrap();
            let back: ClipContent = tandem_proto::decode(&bytes).unwrap();
            prop_assert_eq!(back.hash(), c.hash());
            prop_assert_eq!(back, c);
        }
    }
}
