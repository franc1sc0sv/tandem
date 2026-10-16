//! Modelo de eventos de entrada, independiente de la plataforma.
//!
//! Las teclas viajan por su código físico USB HID: el sistema destino aplica su
//! propia distribución, así ES/LatAm, teclas muertas y AltGr/Option funcionan solos
//! (docs/research/input-engine.md). Este módulo no depende de ninguna plataforma;
//! la captura y la inyección convierten desde y hacia estos tipos con [`keymap`].
//!
//! - [`keymap`]: tablas HID ↔ keycode de Mac ↔ scancode de Windows.
//! - [`remap`]: reasignación de modificadores por dirección (Cmd→Ctrl, etc.).
//! - [`pressed`]: conjunto de teclas y botones presionados, con `release_all()`.

pub mod keymap;
pub mod pressed;
pub mod remap;

use serde::{Deserialize, Serialize};

pub use keymap::{MacKeycode, WinScancode};
pub use pressed::{Change, Pressed};
pub use remap::{Direction, ModifierRemap, RemapError, RemapPolicy};

/// Página HID de teclado (Keyboard/Keypad).
pub const PAGE_KEYBOARD: u16 = 0x07;
/// Página HID de consumo: teclas multimedia.
pub const PAGE_CONSUMER: u16 = 0x0C;
/// Página HID Generic Desktop: dormir, despertar.
pub const PAGE_GENERIC_DESKTOP: u16 = 0x01;

/// Código físico de una tecla: página y uso USB HID (equivale a `KeyboardEvent.code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct HidUsage {
    pub page: u16,
    pub id: u16,
}

impl HidUsage {
    pub const fn new(page: u16, id: u16) -> Self {
        Self { page, id }
    }

    /// Uso de la página de teclado (0x07).
    pub const fn key(id: u16) -> Self {
        Self::new(PAGE_KEYBOARD, id)
    }

    /// Uso de la página de consumo (0x0C).
    pub const fn consumer(id: u16) -> Self {
        Self::new(PAGE_CONSUMER, id)
    }

    /// El modificador que representa esta tecla, si lo es.
    pub fn modifier(self) -> Option<ModifierKey> {
        ModifierKey::from_usage(self)
    }
}

/// Las ocho teclas modificadoras de HID (0xE0–0xE7), con lado.
///
/// `Meta` es Cmd en Mac y la tecla Windows en PC; `Alt` es Option en Mac.
/// `AltRight` en Windows es AltGr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModifierKey {
    ControlLeft,
    ShiftLeft,
    AltLeft,
    MetaLeft,
    ControlRight,
    ShiftRight,
    AltRight,
    MetaRight,
}

impl ModifierKey {
    /// En el orden de los bits del reporte HID.
    pub const ALL: [ModifierKey; 8] = [
        ModifierKey::ControlLeft,
        ModifierKey::ShiftLeft,
        ModifierKey::AltLeft,
        ModifierKey::MetaLeft,
        ModifierKey::ControlRight,
        ModifierKey::ShiftRight,
        ModifierKey::AltRight,
        ModifierKey::MetaRight,
    ];

    /// Posición en el byte de modificadores del reporte HID.
    pub const fn bit(self) -> u8 {
        self as u8
    }

    pub const fn usage(self) -> HidUsage {
        HidUsage::key(0xE0 + self as u16)
    }

    pub fn from_usage(usage: HidUsage) -> Option<Self> {
        if usage.page != PAGE_KEYBOARD || !(0xE0..=0xE7).contains(&usage.id) {
            return None;
        }
        Self::ALL.get(usize::from(usage.id - 0xE0)).copied()
    }
}

/// Conjunto de modificadores presionados, con el formato del byte HID.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const EMPTY: Modifiers = Modifiers(0);

    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, key: ModifierKey) -> bool {
        self.0 & (1 << key.bit()) != 0
    }

    pub fn insert(&mut self, key: ModifierKey) {
        self.0 |= 1 << key.bit();
    }

    pub fn remove(&mut self, key: ModifierKey) {
        self.0 &= !(1 << key.bit());
    }

    pub fn iter(self) -> impl Iterator<Item = ModifierKey> {
        ModifierKey::ALL
            .into_iter()
            .filter(move |k| self.contains(*k))
    }
}

impl FromIterator<ModifierKey> for Modifiers {
    fn from_iter<I: IntoIterator<Item = ModifierKey>>(iter: I) -> Self {
        let mut m = Modifiers::EMPTY;
        for k in iter {
            m.insert(k);
        }
        m
    }
}

/// Estado de una tecla o un botón.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    Pressed,
    Released,
}

/// Botón del mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Right,
    Middle,
    /// Botón lateral "atrás" (X1 en Windows, botón 3 en Mac).
    Back,
    /// Botón lateral "adelante" (X2 en Windows, botón 4 en Mac).
    Forward,
    /// Cualquier otro, por número de botón empezando en 0 (izquierdo).
    Other(u8),
}

/// Desplazamiento. Convención: `y` positivo = rueda hacia adelante (alejándose del
/// usuario, como `WHEEL_DELTA` en Windows); `x` positivo = hacia la derecha. Cada
/// captura deshace el "scroll natural" de su sistema y cada inyección lo vuelve a
/// aplicar según el destino, para que el sentido físico se conserve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Scroll {
    /// Rueda con muescas, en unidades v120 (120 = una muesca; admite fracciones).
    Wheel { dx_v120: i32, dy_v120: i32 },
    /// Trackpad o rueda continua, en píxeles lógicos.
    Pixels { dx: f64, dy: f64 },
}

/// Un evento de entrada, tal como viaja entre las dos máquinas.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    /// Tecla por código físico HID.
    Key {
        usage: HidUsage,
        state: KeyState,
    },
    Button {
        button: Button,
        state: KeyState,
    },
    /// Movimiento relativo, sin aceleración del origen. La captura lo da en
    /// unidades nativas de su máquina; por el enlace viaja en puntos lógicos
    /// (`session`).
    MotionRelative {
        dx: f64,
        dy: f64,
    },
    /// Posición absoluta en las coordenadas nativas del cursor de la máquina
    /// (las de sus [`Screen`](crate::geometry::Screen)). Es lo que inyecta el
    /// cliente, y así la resincronización corrige datagramas perdidos.
    MotionAbsolute {
        x: f64,
        y: f64,
    },
    Scroll(Scroll),
    /// Estado completo de modificadores, para sincronizar al entrar a la pantalla.
    Modifiers(Modifiers),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modificadores_ocupan_e0_a_e7_en_orden_hid() {
        for (i, k) in ModifierKey::ALL.into_iter().enumerate() {
            assert_eq!(k.usage(), HidUsage::key(0xE0 + i as u16));
            assert_eq!(usize::from(k.bit()), i);
            assert_eq!(ModifierKey::from_usage(k.usage()), Some(k));
        }
    }

    #[test]
    fn solo_la_pagina_de_teclado_tiene_modificadores() {
        assert_eq!(HidUsage::consumer(0xE0).modifier(), None);
        assert_eq!(HidUsage::key(0xDF).modifier(), None);
        assert_eq!(HidUsage::key(0xE8).modifier(), None);
        assert_eq!(HidUsage::key(0x04).modifier(), None);
    }

    #[test]
    fn modifiers_inserta_quita_e_itera() {
        let mut m = Modifiers::EMPTY;
        assert!(m.is_empty());
        m.insert(ModifierKey::MetaLeft);
        m.insert(ModifierKey::ShiftRight);
        assert_eq!(m.bits(), 0b0010_1000);
        assert!(m.contains(ModifierKey::MetaLeft));
        assert_eq!(
            m.iter().collect::<Vec<_>>(),
            vec![ModifierKey::MetaLeft, ModifierKey::ShiftRight]
        );
        m.remove(ModifierKey::MetaLeft);
        assert_eq!(m, Modifiers::from_bits(0b0010_0000));
        assert_eq!(
            [ModifierKey::ShiftRight].into_iter().collect::<Modifiers>(),
            m
        );
    }

    #[test]
    fn eventos_ida_y_vuelta_por_postcard() {
        let events = [
            InputEvent::Key {
                usage: HidUsage::consumer(0xCD),
                state: KeyState::Pressed,
            },
            InputEvent::Button {
                button: Button::Other(7),
                state: KeyState::Released,
            },
            InputEvent::MotionRelative { dx: 0.25, dy: -3.0 },
            InputEvent::MotionAbsolute { x: 1919.5, y: 0.0 },
            InputEvent::Scroll(Scroll::Wheel {
                dx_v120: -30,
                dy_v120: 120,
            }),
            InputEvent::Scroll(Scroll::Pixels { dx: 0.0, dy: -12.5 }),
            InputEvent::Modifiers(Modifiers::from_bits(0b1000_0001)),
        ];
        for e in events {
            let bytes = postcard::to_stdvec(&e).unwrap();
            assert_eq!(postcard::from_bytes::<InputEvent>(&bytes).unwrap(), e);
        }
    }
}
