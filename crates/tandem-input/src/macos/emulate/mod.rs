//! Inyección de entrada en macOS.
//!
//! [`MacEmulator`] traduce cada [`InputEvent`] a los eventos de CoreGraphics que
//! macOS espera de un teclado y un mouse reales ([`MacEvent`]) y los entrega a un
//! [`MacSink`]. Toda la traducción es portable y se prueba en Linux con
//! [`RecordingSink`]; el sink real, [`CgSink`], solo existe en macOS.
//!
//! Lo que macOS necesita y no viene en el evento portable:
//! - Los modificadores no son teclas: llegan como `kCGEventFlagsChanged`, y cada
//!   tecla y cada clic lleva los flags vigentes (con el lado en los bits del
//!   dispositivo, para que Option derecha se distinga de la izquierda).
//! - Las flechas llevan `NumericPad|SecondaryFn`; las F, Inicio/Fin, RePág/AvPág y
//!   Supr llevan `SecondaryFn`; el teclado numérico, `NumericPad`.
//! - Repetición: una tecla presionada otra vez sin soltarla sale con
//!   `kCGKeyboardEventAutorepeat`.
//! - Mouse: con un botón presionado, el movimiento es `*Dragged`; doble y triple
//!   clic se marcan con `kCGMouseEventClickState`, contado aquí.
//! - Scroll: la rueda va en líneas (acumulando fracciones de v120) y el trackpad en
//!   píxeles, aplicando el "scroll natural" de la Mac.
//! - Multimedia: macOS no las acepta como keycodes; van como `NSEvent`
//!   `SystemDefined` subtipo 8 con el `NX_KEYTYPE_*`.

#[cfg(target_os = "macos")]
mod cg;
mod sink;

use std::time::{Duration, Instant};

use thiserror::Error;

use crate::event::keymap;
use crate::event::{
    Button, Change, HidUsage, InputEvent, KeyState, ModifierKey, Modifiers, PAGE_CONSUMER,
    PAGE_KEYBOARD, Pressed, Scroll,
};
use crate::geometry::{Point, Rect};

#[cfg(target_os = "macos")]
pub use cg::CgSink;
pub use sink::RecordingSink;

/// Bits de `CGEventFlags`. Los de abajo (`DEVICE_*`) son los `NX_DEVICE*` que
/// macOS pone en los eventos reales para saber de qué lado está el modificador.
pub mod flags {
    pub const ALPHA_SHIFT: u64 = 0x0001_0000;
    pub const SHIFT: u64 = 0x0002_0000;
    pub const CONTROL: u64 = 0x0004_0000;
    pub const ALTERNATE: u64 = 0x0008_0000;
    pub const COMMAND: u64 = 0x0010_0000;
    pub const NUMERIC_PAD: u64 = 0x0020_0000;
    pub const SECONDARY_FN: u64 = 0x0080_0000;
    /// Lo llevan todos los eventos del HID; algunas apps lo esperan.
    pub const NON_COALESCED: u64 = 0x0000_0100;

    pub const DEVICE_LEFT_CONTROL: u64 = 0x0000_0001;
    pub const DEVICE_LEFT_SHIFT: u64 = 0x0000_0002;
    pub const DEVICE_RIGHT_SHIFT: u64 = 0x0000_0004;
    pub const DEVICE_LEFT_COMMAND: u64 = 0x0000_0008;
    pub const DEVICE_RIGHT_COMMAND: u64 = 0x0000_0010;
    pub const DEVICE_LEFT_ALTERNATE: u64 = 0x0000_0020;
    pub const DEVICE_RIGHT_ALTERNATE: u64 = 0x0000_0040;
    pub const DEVICE_RIGHT_CONTROL: u64 = 0x0000_2000;
}

/// Tipo de evento de mouse de CoreGraphics (`kCGEvent*Mouse*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Moved,
    LeftDown,
    LeftUp,
    LeftDragged,
    RightDown,
    RightUp,
    RightDragged,
    OtherDown,
    OtherUp,
    OtherDragged,
}

/// Unidad del scroll (`kCGScrollEventUnit*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollUnit {
    Line,
    Pixel,
}

/// Un evento listo para `CGEventPost`, sin tipos de CoreGraphics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MacEvent {
    Key {
        keycode: u16,
        down: bool,
        autorepeat: bool,
        flags: u64,
    },
    /// Cambio de un modificador: `keycode` es la tecla y `flags` el estado nuevo.
    FlagsChanged { keycode: u16, flags: u64 },
    Mouse {
        kind: MouseKind,
        /// Posición en puntos globales (origen arriba a la izquierda de la principal).
        pos: Point,
        /// `kCGMouseEventButtonNumber`: 0 izquierdo, 1 derecho, 2 medio, …
        button: u32,
        /// `kCGMouseEventClickState`: 1 simple, 2 doble, 3 triple.
        click_state: i64,
        /// `kCGMouseEventDeltaX/Y`, para juegos y apps que leen el delta.
        delta: (f64, f64),
        flags: u64,
    },
    /// `wheel1` vertical, `wheel2` horizontal, con el signo de CoreGraphics.
    Scroll {
        unit: ScrollUnit,
        wheel1: i32,
        wheel2: i32,
        flags: u64,
    },
    /// Tecla multimedia: `NSEvent` `SystemDefined` subtipo 8.
    SystemKey { data1: i64, modifier_flags: u64 },
}

#[derive(Debug, Error)]
pub enum EmulateError {
    #[error("la tecla {0:?} no tiene keycode en macOS")]
    Unmapped(HidUsage),
    #[error("no hay pantallas activas para poner el cursor")]
    NoScreens,
    #[error("CoreGraphics: {0}")]
    Platform(String),
}

/// Lo que el emulador necesita del sistema: publicar eventos y leer el cursor.
pub trait MacSink {
    fn post(&mut self, event: &MacEvent) -> Result<(), EmulateError>;

    /// Posición real del cursor, en puntos globales.
    fn cursor(&mut self) -> Result<Point, EmulateError>;

    /// Rectángulos de las pantallas activas, en puntos globales.
    fn screens(&mut self) -> Result<Vec<Rect>, EmulateError>;

    /// Ventana del doble clic que el usuario configuró en la Mac.
    fn double_click_interval(&mut self) -> Duration;

    /// `true` si la Mac tiene el "scroll natural" activado.
    fn natural_scroll(&mut self) -> bool;
}

/// Distancia máxima, en puntos, entre clics para que cuenten como doble clic.
const CLICK_SLOP: f64 = 4.0;

/// Una muesca de rueda en unidades v120.
const V120: i32 = 120;

/// Traduce eventos portables a eventos de macOS. Lleva el estado que macOS
/// espera ver coherente: teclas y botones presionados, Bloq Mayús, posición del
/// cursor, contador de clics y restos de scroll.
#[derive(Debug)]
pub struct MacEmulator<S: MacSink> {
    sink: S,
    pressed: Pressed,
    caps_lock: bool,
    screens: Vec<Rect>,
    natural_scroll: bool,
    double_click: Duration,
    clicks: ClickCounter,
    wheel_rest: (i32, i32),
    pixel_rest: (f64, f64),
}

impl<S: MacSink> MacEmulator<S> {
    /// Lee del sink las pantallas, el doble clic y el scroll natural.
    pub fn new(mut sink: S) -> Result<Self, EmulateError> {
        let screens = sink.screens()?;
        let double_click = sink.double_click_interval();
        let natural_scroll = sink.natural_scroll();
        Ok(Self {
            sink,
            pressed: Pressed::new(),
            caps_lock: false,
            screens,
            natural_scroll,
            double_click,
            clicks: ClickCounter::default(),
            wheel_rest: (0, 0),
            pixel_rest: (0.0, 0.0),
        })
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    /// Vuelve a leer las pantallas y las preferencias; llamarlo cuando cambie la
    /// configuración de pantallas.
    pub fn refresh(&mut self) -> Result<(), EmulateError> {
        self.screens = self.sink.screens()?;
        self.double_click = self.sink.double_click_interval();
        self.natural_scroll = self.sink.natural_scroll();
        Ok(())
    }

    /// Inyecta un evento.
    pub fn emit(&mut self, event: &InputEvent) -> Result<(), EmulateError> {
        self.emit_at(event, Instant::now())
    }

    /// Suelta todo lo que esté presionado (al salir de la pantalla, al
    /// desconectarse o por timeout), para que nada quede pegado en la Mac.
    pub fn release_all(&mut self) -> Result<(), EmulateError> {
        let now = Instant::now();
        let mut first_err = None;
        // Se sueltan de a uno, en el orden de `Pressed::release_all`, para que
        // cada evento lleve los modificadores que siguen abajo en ese momento
        // (soltar C con Cmd todavía presionado, como un teclado real).
        for event in self.pressed.clone().release_all() {
            // Se intenta soltar todo aunque una falle.
            if let Err(e) = self.emit_at(&event, now) {
                first_err.get_or_insert(e);
            }
        }
        first_err.map_or(Ok(()), Err)
    }

    fn emit_at(&mut self, event: &InputEvent, now: Instant) -> Result<(), EmulateError> {
        match *event {
            InputEvent::Key { usage, state } => {
                // Validar antes de registrar: una tecla sin keycode no debe quedar
                // como presionada para siempre.
                if media_key(usage).is_none() && keymap::to_mac(usage).is_none() {
                    return Err(EmulateError::Unmapped(usage));
                }
                let change = self.pressed.record(event);
                self.key(usage, state, change)
            }
            InputEvent::Button { button, state } => {
                // `Other(0)` y `Left` son el mismo botón: un solo nombre para que
                // el conjunto de presionados y el arrastre no se confundan.
                let button = normalize_button(button);
                match self.pressed.record(&InputEvent::Button { button, state }) {
                    Change::Changed => self.button(button, state, now),
                    Change::Repeat | Change::Ignored => Ok(()),
                }
            }
            InputEvent::MotionRelative { dx, dy } => {
                let from = self.sink.cursor()?;
                self.motion(Point::new(from.x + dx, from.y + dy), from)
            }
            InputEvent::MotionAbsolute { x, y } => {
                let from = self.sink.cursor()?;
                self.motion(Point::new(x, y), from)
            }
            InputEvent::Scroll(scroll) => self.scroll(scroll),
            InputEvent::Modifiers(target) => self.sync_modifiers(target, now),
        }
    }

    fn key(
        &mut self,
        usage: HidUsage,
        state: KeyState,
        change: Change,
    ) -> Result<(), EmulateError> {
        if change == Change::Ignored {
            return Ok(());
        }
        let down = state == KeyState::Pressed;
        let repeat = change == Change::Repeat;

        if let Some(key) = media_key(usage) {
            return self.sink.post(&MacEvent::SystemKey {
                data1: system_key_data1(key, down, repeat),
                modifier_flags: system_key_modifier_flags(down),
            });
        }
        let keycode = keymap::to_mac(usage)
            .ok_or(EmulateError::Unmapped(usage))?
            .0;

        if usage.modifier().is_some() {
            // Un modificador mantenido no se repite en macOS.
            if repeat {
                return Ok(());
            }
            let flags = self.flags();
            return self.sink.post(&MacEvent::FlagsChanged { keycode, flags });
        }
        if usage == CAPS_LOCK {
            // Bloq Mayús alterna al presionar; soltarla no cambia nada.
            if !down || repeat {
                return Ok(());
            }
            self.caps_lock = !self.caps_lock;
            let flags = self.flags();
            return self.sink.post(&MacEvent::FlagsChanged { keycode, flags });
        }
        self.sink.post(&MacEvent::Key {
            keycode,
            down,
            autorepeat: repeat,
            flags: self.flags() | key_flags(keycode),
        })
    }

    fn button(
        &mut self,
        button: Button,
        state: KeyState,
        now: Instant,
    ) -> Result<(), EmulateError> {
        let pos = self.sink.cursor()?;
        let down = state == KeyState::Pressed;
        let click_state = if down {
            self.clicks.press(button, pos, now, self.double_click)
        } else {
            self.clicks.release(button)
        };
        let number = button_number(button);
        let kind = match (number, down) {
            (0, true) => MouseKind::LeftDown,
            (0, false) => MouseKind::LeftUp,
            (1, true) => MouseKind::RightDown,
            (1, false) => MouseKind::RightUp,
            (_, true) => MouseKind::OtherDown,
            (_, false) => MouseKind::OtherUp,
        };
        self.sink.post(&MacEvent::Mouse {
            kind,
            pos,
            button: number,
            click_state,
            delta: (0.0, 0.0),
            flags: self.flags(),
        })
    }

    fn motion(&mut self, to: Point, from: Point) -> Result<(), EmulateError> {
        let to = clamp_to_screens(&self.screens, to).ok_or(EmulateError::NoScreens)?;
        let (kind, button) = self.drag_kind();
        self.sink.post(&MacEvent::Mouse {
            kind,
            pos: to,
            button,
            click_state: 0,
            delta: (to.x - from.x, to.y - from.y),
            flags: self.flags(),
        })
    }

    /// Con un botón presionado el movimiento es un arrastre del botón más bajo.
    fn drag_kind(&self) -> (MouseKind, u32) {
        let named = [
            Button::Left,
            Button::Right,
            Button::Middle,
            Button::Back,
            Button::Forward,
        ];
        let held = named
            .into_iter()
            .chain((5..=u8::MAX).map(Button::Other))
            .find(|b| self.pressed.is_button_pressed(*b));
        match held.map(button_number) {
            None => (MouseKind::Moved, 0),
            Some(0) => (MouseKind::LeftDragged, 0),
            Some(1) => (MouseKind::RightDragged, 1),
            Some(n) => (MouseKind::OtherDragged, n),
        }
    }

    fn scroll(&mut self, scroll: Scroll) -> Result<(), EmulateError> {
        // Convención portable: y+ = rueda hacia adelante, x+ = derecha. En
        // CoreGraphics sin scroll natural, adelante es wheel1 positivo y derecha
        // es wheel2 negativo; con scroll natural los dos se invierten.
        let sign = if self.natural_scroll { -1 } else { 1 };
        let (unit, dx, dy) = match scroll {
            Scroll::Wheel { dx_v120, dy_v120 } => {
                let (rx, x) = split_v120(self.wheel_rest.0.saturating_add(dx_v120));
                let (ry, y) = split_v120(self.wheel_rest.1.saturating_add(dy_v120));
                self.wheel_rest = (rx, ry);
                (ScrollUnit::Line, x, y)
            }
            Scroll::Pixels { dx, dy } => {
                let (rx, x) = split_pixels(self.pixel_rest.0 + dx);
                let (ry, y) = split_pixels(self.pixel_rest.1 + dy);
                self.pixel_rest = (rx, ry);
                (ScrollUnit::Pixel, x, y)
            }
        };
        if dx == 0 && dy == 0 {
            return Ok(());
        }
        self.sink.post(&MacEvent::Scroll {
            unit,
            wheel1: dy.saturating_mul(sign),
            wheel2: dx.saturating_mul(-sign),
            flags: self.flags(),
        })
    }

    /// Deja los modificadores presionados iguales a `target`: suelta los que
    /// sobran y presiona los que faltan.
    fn sync_modifiers(&mut self, target: Modifiers, now: Instant) -> Result<(), EmulateError> {
        let current = self.pressed.modifiers();
        for key in ModifierKey::ALL {
            let state = match (current.contains(key), target.contains(key)) {
                (true, false) => KeyState::Released,
                (false, true) => KeyState::Pressed,
                _ => continue,
            };
            let event = InputEvent::Key {
                usage: key.usage(),
                state,
            };
            self.emit_at(&event, now)?;
        }
        Ok(())
    }

    /// Flags vigentes: modificadores presionados, con su lado, y Bloq Mayús.
    fn flags(&self) -> u64 {
        modifier_flags(self.pressed.modifiers(), self.caps_lock)
    }
}

const CAPS_LOCK: HidUsage = HidUsage::key(0x39);

/// `CGEventFlags` de un conjunto de modificadores.
pub fn modifier_flags(mods: Modifiers, caps_lock: bool) -> u64 {
    use flags::*;
    let mut f = NON_COALESCED;
    if caps_lock {
        f |= ALPHA_SHIFT;
    }
    for key in mods.iter() {
        f |= match key {
            ModifierKey::ControlLeft => CONTROL | DEVICE_LEFT_CONTROL,
            ModifierKey::ControlRight => CONTROL | DEVICE_RIGHT_CONTROL,
            ModifierKey::ShiftLeft => SHIFT | DEVICE_LEFT_SHIFT,
            ModifierKey::ShiftRight => SHIFT | DEVICE_RIGHT_SHIFT,
            ModifierKey::AltLeft => ALTERNATE | DEVICE_LEFT_ALTERNATE,
            ModifierKey::AltRight => ALTERNATE | DEVICE_RIGHT_ALTERNATE,
            ModifierKey::MetaLeft => COMMAND | DEVICE_LEFT_COMMAND,
            ModifierKey::MetaRight => COMMAND | DEVICE_RIGHT_COMMAND,
        };
    }
    f
}

/// Flags propios de la tecla, como los pone el teclado de una Mac.
pub fn key_flags(keycode: u16) -> u64 {
    use flags::*;
    match keycode {
        // Flechas: ← → ↓ ↑.
        0x7B..=0x7E => NUMERIC_PAD | SECONDARY_FN,
        // F1–F20, Ayuda/Insert, Inicio, RePág, Supr, Fin, AvPág.
        0x7A | 0x78 | 0x63 | 0x76 | 0x60 | 0x61 | 0x62 | 0x64 | 0x65 | 0x6D | 0x67 | 0x6F
        | 0x69 | 0x6B | 0x71 | 0x6A | 0x40 | 0x4F | 0x50 | 0x5A | 0x72 | 0x73 | 0x74 | 0x75
        | 0x77 | 0x79 => SECONDARY_FN,
        // Teclado numérico.
        0x41 | 0x43 | 0x45 | 0x47 | 0x4B | 0x4C | 0x4E | 0x51 | 0x52..=0x59 | 0x5B | 0x5C => {
            NUMERIC_PAD
        }
        _ => 0,
    }
}

/// `NX_KEYTYPE_*` de una tecla multimedia, si lo es. Incluye el volumen de la
/// página de teclado (0x7F–0x81): como keycode macOS lo ignora.
pub fn media_key(usage: HidUsage) -> Option<u32> {
    let key = match (usage.page, usage.id) {
        (PAGE_CONSUMER, 0xE9) | (PAGE_KEYBOARD, 0x80) => 0, // NX_KEYTYPE_SOUND_UP
        (PAGE_CONSUMER, 0xEA) | (PAGE_KEYBOARD, 0x81) => 1, // NX_KEYTYPE_SOUND_DOWN
        (PAGE_CONSUMER, 0x6F) => 2,                         // NX_KEYTYPE_BRIGHTNESS_UP
        (PAGE_CONSUMER, 0x70) => 3,                         // NX_KEYTYPE_BRIGHTNESS_DOWN
        (PAGE_CONSUMER, 0xE2) | (PAGE_KEYBOARD, 0x7F) => 7, // NX_KEYTYPE_MUTE
        (PAGE_CONSUMER, 0xB8) => 14,                        // NX_KEYTYPE_EJECT
        (PAGE_CONSUMER, 0xCD | 0xB0) => 16,                 // NX_KEYTYPE_PLAY
        (PAGE_CONSUMER, 0xB5) => 17,                        // NX_KEYTYPE_NEXT
        (PAGE_CONSUMER, 0xB6) => 18,                        // NX_KEYTYPE_PREVIOUS
        (PAGE_CONSUMER, 0xB3) => 19,                        // NX_KEYTYPE_FAST
        (PAGE_CONSUMER, 0xB4) => 20,                        // NX_KEYTYPE_REWIND
        _ => return None,
    };
    Some(key)
}

/// `data1` del `NSEvent` de una tecla multimedia: tipo en los 16 bits altos,
/// 0xA (abajo) o 0xB (arriba) en el byte siguiente y la repetición en el bit 0.
pub fn system_key_data1(key: u32, down: bool, repeat: bool) -> i64 {
    let state: i64 = if down { 0xA } else { 0xB };
    (i64::from(key) << 16) | (state << 8) | i64::from(repeat)
}

/// `modifierFlags` del `NSEvent` de una tecla multimedia.
pub fn system_key_modifier_flags(down: bool) -> u64 {
    if down { 0xA00 } else { 0xB00 }
}

/// Número de botón de CoreGraphics. Atrás y adelante son 3 y 4, como en un
/// mouse USB en la Mac.
pub fn button_number(button: Button) -> u32 {
    match button {
        Button::Left => 0,
        Button::Right => 1,
        Button::Middle => 2,
        Button::Back => 3,
        Button::Forward => 4,
        Button::Other(n) => u32::from(n),
    }
}

/// `Other(n)` con el nombre propio del botón, si lo tiene.
fn normalize_button(button: Button) -> Button {
    match button {
        Button::Other(0) => Button::Left,
        Button::Other(1) => Button::Right,
        Button::Other(2) => Button::Middle,
        Button::Other(3) => Button::Back,
        Button::Other(4) => Button::Forward,
        b => b,
    }
}

/// Lleva el punto a la pantalla activa más cercana, para que el cursor nunca
/// quede fuera de las pantallas (macOS lo dejaría en un lugar raro).
pub fn clamp_to_screens(screens: &[Rect], p: Point) -> Option<Point> {
    screens
        .iter()
        .map(|r| r.clamp(p))
        .min_by(|a, b| dist2(*a, p).total_cmp(&dist2(*b, p)))
}

fn dist2(a: Point, b: Point) -> f64 {
    (a.x - b.x).powi(2) + (a.y - b.y).powi(2)
}

/// Separa unidades v120 en líneas enteras y el resto, hacia cero.
fn split_v120(total: i32) -> (i32, i32) {
    (total % V120, total / V120)
}

/// Separa píxeles en un entero y el resto fraccionario, hacia cero.
fn split_pixels(total: f64) -> (f64, i32) {
    let whole = total.trunc();
    // `as` satura en los extremos, que es lo que queremos con valores absurdos.
    (total - whole, whole as i32)
}

/// Cuenta clics seguidos del mismo botón, en el mismo lugar y dentro de la
/// ventana del doble clic, como hace macOS con un mouse real.
#[derive(Debug, Default)]
struct ClickCounter {
    last: Option<LastClick>,
}

#[derive(Debug, Clone, Copy)]
struct LastClick {
    button: Button,
    at: Instant,
    pos: Point,
    count: i64,
}

impl ClickCounter {
    fn press(&mut self, button: Button, pos: Point, now: Instant, window: Duration) -> i64 {
        let count = match self.last {
            Some(l)
                if l.button == button
                    && now.saturating_duration_since(l.at) <= window
                    && dist2(l.pos, pos) <= CLICK_SLOP * CLICK_SLOP =>
            {
                l.count.saturating_add(1)
            }
            _ => 1,
        };
        self.last = Some(LastClick {
            button,
            at: now,
            pos,
            count,
        });
        count
    }

    /// El evento de soltar lleva la misma cuenta que el de presionar.
    fn release(&self, button: Button) -> i64 {
        match self.last {
            Some(l) if l.button == button => l.count,
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests;
