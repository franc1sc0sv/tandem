//! Inyección de entrada en Windows con `SendInput`.
//!
//! [`Emulator`] traduce cada [`InputEvent`] a registros [`WinInput`] (un espejo portable
//! de la estructura `INPUT` de Win32) y los entrega a un [`InputSink`]. La traducción
//! vive aquí y se prueba en Linux; [`SendInputSink`] (solo en Windows) es el envío real.
//!
//! Decisiones (docs/research/input-engine.md):
//! - Teclas por **scancode** (`KEYEVENTF_SCANCODE`, + `EXTENDEDKEY` para las E0): así el
//!   sistema aplica su propia distribución y ES/LatAm, AltGr y teclas muertas funcionan.
//! - Mouse **absoluto** sobre el escritorio virtual (`MOVE|ABSOLUTE|VIRTUALDESK`): el
//!   movimiento relativo de `SendInput` pasa por la aceleración del puntero. La posición
//!   se guarda en `f64`, así los deltas de menos de un píxel se acumulan en vez de perderse.
//! - Rueda con `mouseData` en unidades de `WHEEL_DELTA` (120), que admite fracciones.
//! - Todo lo inyectado lleva [`TANDEM_EXTRA_INFO`] en `dwExtraInfo`.

#[cfg(windows)]
mod sys;

#[cfg(windows)]
pub use sys::{SendInputSink, set_per_monitor_v2_dpi_awareness};

use crate::event::{
    Button, HidUsage, InputEvent, KeyState, ModifierKey, Modifiers, Pressed, Scroll, keymap,
};
use crate::geometry::{Point, Rect};
use crate::windows::TANDEM_EXTRA_INFO;

// Constantes de winuser.h, repetidas aquí para que la traducción sea portable. Una prueba
// en Windows comprueba que coinciden con las del crate `windows`.
pub const KEYEVENTF_EXTENDEDKEY: u32 = 0x0001;
pub const KEYEVENTF_KEYUP: u32 = 0x0002;
pub const KEYEVENTF_SCANCODE: u32 = 0x0008;
pub const MOUSEEVENTF_MOVE: u32 = 0x0001;
pub const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
pub const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
pub const MOUSEEVENTF_RIGHTDOWN: u32 = 0x0008;
pub const MOUSEEVENTF_RIGHTUP: u32 = 0x0010;
pub const MOUSEEVENTF_MIDDLEDOWN: u32 = 0x0020;
pub const MOUSEEVENTF_MIDDLEUP: u32 = 0x0040;
pub const MOUSEEVENTF_XDOWN: u32 = 0x0080;
pub const MOUSEEVENTF_XUP: u32 = 0x0100;
pub const MOUSEEVENTF_WHEEL: u32 = 0x0800;
pub const MOUSEEVENTF_HWHEEL: u32 = 0x1000;
pub const MOUSEEVENTF_VIRTUALDESK: u32 = 0x4000;
pub const MOUSEEVENTF_ABSOLUTE: u32 = 0x8000;
pub const XBUTTON1: u32 = 0x0001;
pub const XBUTTON2: u32 = 0x0002;
/// `VK_PAUSE`: Pausa no tiene un scancode simple (es la secuencia E1 1D 45) y el 0x45
/// suelto que da la tabla de Chromium Windows lo interpreta como Bloq Num.
pub const VK_PAUSE: u16 = 0x13;

/// Rango de las coordenadas absolutas normalizadas de `SendInput`.
const ABS_SCALE: i64 = 65_536;

/// Píxeles de trackpad que equivalen a una muesca de rueda (120). Es lo que desplaza
/// Chromium en Windows por muesca a escala 100 %; afinarlo con el equipo real.
pub const PIXELS_PER_NOTCH: f64 = 100.0;
const WHEEL_DELTA: f64 = 120.0;

/// Un registro `INPUT` de Win32, sin depender del crate `windows`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinInput {
    Keyboard {
        vk: u16,
        scan: u16,
        flags: u32,
        extra_info: usize,
    },
    Mouse {
        dx: i32,
        dy: i32,
        /// Delta de rueda (con signo, como lo guarda Win32) o botón X.
        data: u32,
        flags: u32,
        extra_info: usize,
    },
}

impl WinInput {
    fn key(scan: u16, flags: u32) -> Self {
        WinInput::Keyboard {
            vk: 0,
            scan,
            flags,
            extra_info: TANDEM_EXTRA_INFO,
        }
    }

    fn mouse(dx: i32, dy: i32, data: u32, flags: u32) -> Self {
        WinInput::Mouse {
            dx,
            dy,
            data,
            flags,
            extra_info: TANDEM_EXTRA_INFO,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EmulateError {
    #[error("la tecla {0:?} no tiene scancode en Windows")]
    UnmappedKey(HidUsage),
    #[error("Windows no tiene el botón {0}")]
    UnsupportedButton(u8),
    #[error("el escritorio no tiene pantallas")]
    NoDesktop,
    /// `SendInput` aceptó menos eventos de los pedidos: típicamente UIPI (la ventana
    /// activa es de un proceso con más integridad) o el escritorio seguro.
    #[error("SendInput inyectó {sent} de {expected} eventos (¿UIPI o escritorio seguro?)")]
    Blocked { sent: usize, expected: usize },
    #[error("{op} falló: {message} ({code:#010X})")]
    Os {
        op: &'static str,
        code: u32,
        message: String,
    },
}

/// Pantallas del escritorio, en píxeles físicos (proceso Per-Monitor-V2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Desktop {
    /// Rectángulo que encierra a todas (`SM_*VIRTUALSCREEN`): la base de `VIRTUALDESK`.
    pub virtual_rect: Rect,
    /// Cada monitor. Puede haber huecos dentro de `virtual_rect` donde no hay pantalla.
    pub monitors: Vec<Rect>,
}

impl Desktop {
    /// Lleva el punto al píxel más cercano que esté en algún monitor, para que la
    /// posición interna no se pierda en un hueco del escritorio virtual.
    fn clamp(&self, p: Point) -> Point {
        let mut best: Option<(f64, Point)> = None;
        for m in &self.monitors {
            if m.contains(p) {
                return p;
            }
            let c = m.clamp(p);
            let d = (c.x - p.x).hypot(c.y - p.y);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, c));
            }
        }
        best.map_or_else(|| self.virtual_rect.clamp(p), |(_, c)| c)
    }

    /// Píxel → coordenada normalizada 0–65535 de `VIRTUALDESK`. Windows convierte de
    /// vuelta con `n * ancho / 65536` truncado; redondear hacia arriba aterriza exacto.
    fn normalize(&self, px: i32, py: i32) -> (i32, i32) {
        let r = &self.virtual_rect;
        (
            normalize_axis(px, r.x, r.width),
            normalize_axis(py, r.y, r.height),
        )
    }
}

fn normalize_axis(p: i32, origin: i32, size: u32) -> i32 {
    let size = i64::from(size.max(1));
    let off = (i64::from(p) - i64::from(origin)).clamp(0, size - 1);
    let n = (off * ABS_SCALE + size - 1) / size;
    // `n` está en 0..=65535 por construcción; el clamp solo documenta el rango.
    n.clamp(0, ABS_SCALE - 1) as i32
}

/// Lo que necesita el emulador del sistema. En Windows es [`SendInputSink`].
pub trait InputSink {
    /// Inyecta los eventos en una sola llamada, sin intercalar con la entrada física.
    fn send(&mut self, inputs: &[WinInput]) -> Result<(), EmulateError>;
    fn desktop(&self) -> Result<Desktop, EmulateError>;
    /// Posición real del cursor, para notar si otro (el mouse local) lo movió.
    fn cursor_pos(&self) -> Option<(i32, i32)>;
}

/// Estado de la inyección: posición del cursor con fracciones, restos de la rueda y lo
/// que quedó presionado.
pub struct Emulator<S: InputSink> {
    sink: S,
    desktop: Desktop,
    pos: Point,
    /// Último píxel enviado; si el cursor real ya no está ahí, alguien más lo movió.
    last_px: Option<(i32, i32)>,
    wheel_rest: (f64, f64),
    pressed: Pressed,
}

impl<S: InputSink> Emulator<S> {
    pub fn new(sink: S) -> Result<Self, EmulateError> {
        let desktop = checked(sink.desktop()?)?;
        let pos = sink
            .cursor_pos()
            .map_or_else(|| center(&desktop.virtual_rect), px_center);
        Ok(Self {
            sink,
            desktop,
            pos,
            last_px: None,
            wheel_rest: (0.0, 0.0),
            pressed: Pressed::new(),
        })
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    /// Posición interna del cursor, con fracciones.
    pub fn position(&self) -> Point {
        self.pos
    }

    /// Lo que se inyectó presionado y sigue sin soltarse.
    pub fn pressed(&self) -> &Pressed {
        &self.pressed
    }

    /// Vuelve a leer las pantallas; llamarlo con `WM_DISPLAYCHANGE`.
    pub fn refresh_desktop(&mut self) -> Result<(), EmulateError> {
        self.desktop = checked(self.sink.desktop()?)?;
        self.pos = self.desktop.clamp(self.pos);
        self.last_px = None;
        Ok(())
    }

    /// Inyecta un evento. Si falla, el estado de teclas presionadas no cambia.
    pub fn emulate(&mut self, event: &InputEvent) -> Result<(), EmulateError> {
        let inputs = match *event {
            InputEvent::Key { usage, state } => vec![key_input(usage, state)?],
            InputEvent::Button { button, state } => vec![button_input(button, state)?],
            InputEvent::MotionRelative { dx, dy } => {
                self.resync();
                let p = Point::new(self.pos.x + dx, self.pos.y + dy);
                return self.move_to(p);
            }
            InputEvent::MotionAbsolute { x, y } => {
                return self.move_to(Point::new(x, y));
            }
            InputEvent::Scroll(scroll) => self.scroll_inputs(scroll),
            InputEvent::Modifiers(target) => return self.sync_modifiers(target),
        };
        self.send(&inputs)?;
        self.pressed.record(event);
        Ok(())
    }

    /// Suelta todo lo que se inyectó presionado: al salir de la pantalla, al
    /// desconectarse o por timeout.
    pub fn release_all(&mut self) -> Result<(), EmulateError> {
        let mut pending = self.pressed.clone();
        let events = pending.release_all();
        self.send_presses(&events)?;
        self.pressed = pending;
        Ok(())
    }

    /// Lleva los modificadores al estado pedido: primero suelta, después presiona.
    fn sync_modifiers(&mut self, target: Modifiers) -> Result<(), EmulateError> {
        let current = self.pressed.modifiers();
        let change = |m: ModifierKey, state| InputEvent::Key {
            usage: m.usage(),
            state,
        };
        let events: Vec<_> = current
            .iter()
            .filter(|m| !target.contains(*m))
            .map(|m| change(m, KeyState::Released))
            .chain(
                target
                    .iter()
                    .filter(|m| !current.contains(*m))
                    .map(|m| change(m, KeyState::Pressed)),
            )
            .collect();
        self.send_presses(&events)?;
        for e in &events {
            self.pressed.record(e);
        }
        Ok(())
    }

    /// Inyecta en una sola llamada una serie de eventos de tecla o botón.
    fn send_presses(&mut self, events: &[InputEvent]) -> Result<(), EmulateError> {
        let inputs = events
            .iter()
            .filter_map(|e| match *e {
                InputEvent::Key { usage, state } => Some(key_input(usage, state)),
                InputEvent::Button { button, state } => Some(button_input(button, state)),
                _ => None,
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.send(&inputs)
    }

    /// Si el cursor real no está donde lo dejamos, se parte de donde está.
    fn resync(&mut self) {
        if let (Some(real), Some(last)) = (self.sink.cursor_pos(), self.last_px)
            && real != last
        {
            self.pos = px_center(real);
            self.last_px = Some(real);
        }
    }

    fn move_to(&mut self, p: Point) -> Result<(), EmulateError> {
        if !(p.x.is_finite() && p.y.is_finite()) {
            return Ok(());
        }
        self.pos = self.desktop.clamp(p);
        let px = (self.pos.x.floor() as i32, self.pos.y.floor() as i32);
        // Sin cambio de píxel no hay nada que mandar: la fracción queda acumulada.
        if self.last_px == Some(px) {
            return Ok(());
        }
        let (nx, ny) = self.desktop.normalize(px.0, px.1);
        let flags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
        self.send(&[WinInput::mouse(nx, ny, 0, flags)])?;
        self.last_px = Some(px);
        Ok(())
    }

    fn scroll_inputs(&mut self, scroll: Scroll) -> Vec<WinInput> {
        let (dx, dy) = match scroll {
            Scroll::Wheel { dx_v120, dy_v120 } => (dx_v120, dy_v120),
            Scroll::Pixels { dx, dy } => {
                let factor = WHEEL_DELTA / PIXELS_PER_NOTCH;
                let (rx, ry) = &mut self.wheel_rest;
                (take_whole(rx, dx * factor), take_whole(ry, dy * factor))
            }
        };
        let mut out = Vec::new();
        // Misma convención que Windows: positivo = adelante en vertical, derecha en horizontal.
        if dy != 0 {
            out.push(WinInput::mouse(0, 0, dy as u32, MOUSEEVENTF_WHEEL));
        }
        if dx != 0 {
            out.push(WinInput::mouse(0, 0, dx as u32, MOUSEEVENTF_HWHEEL));
        }
        out
    }

    fn send(&mut self, inputs: &[WinInput]) -> Result<(), EmulateError> {
        if inputs.is_empty() {
            return Ok(());
        }
        self.sink.send(inputs)
    }
}

/// Suma `delta` al resto y saca la parte entera; la fracción queda para la próxima.
fn take_whole(rest: &mut f64, delta: f64) -> i32 {
    if !delta.is_finite() {
        return 0;
    }
    *rest += delta;
    // El margen evita perder una unidad por redondeo (100 × 1.2 suma 119.999…).
    let whole = (*rest + rest.signum() * 1e-9).trunc();
    *rest -= whole;
    whole.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

fn checked(desktop: Desktop) -> Result<Desktop, EmulateError> {
    let empty = |r: &Rect| r.width == 0 || r.height == 0;
    if empty(&desktop.virtual_rect) {
        return Err(EmulateError::NoDesktop);
    }
    Ok(Desktop {
        monitors: desktop.monitors.into_iter().filter(|m| !empty(m)).collect(),
        ..desktop
    })
}

fn center(r: &Rect) -> Point {
    Point::new(
        r.left() + f64::from(r.width) / 2.0,
        r.top() + f64::from(r.height) / 2.0,
    )
}

fn px_center((x, y): (i32, i32)) -> Point {
    Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5)
}

/// Registro de teclado para una tecla física.
pub fn key_input(usage: HidUsage, state: KeyState) -> Result<WinInput, EmulateError> {
    let up = if state == KeyState::Released {
        KEYEVENTF_KEYUP
    } else {
        0
    };
    if usage == HidUsage::key(0x48) {
        return Ok(WinInput::Keyboard {
            vk: VK_PAUSE,
            scan: 0,
            flags: up,
            extra_info: TANDEM_EXTRA_INFO,
        });
    }
    let sc = keymap::to_win(usage).ok_or(EmulateError::UnmappedKey(usage))?;
    let ext = if sc.is_extended() {
        KEYEVENTF_EXTENDEDKEY
    } else {
        0
    };
    Ok(WinInput::key(
        u16::from(sc.code()),
        KEYEVENTF_SCANCODE | ext | up,
    ))
}

/// Registro de mouse para un botón.
pub fn button_input(button: Button, state: KeyState) -> Result<WinInput, EmulateError> {
    let down = state == KeyState::Pressed;
    let pick = |d: u32, u: u32| if down { d } else { u };
    let (flags, data) = match normalize_button(button)? {
        Button::Left => (pick(MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP), 0),
        Button::Right => (pick(MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP), 0),
        Button::Middle => (pick(MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP), 0),
        Button::Back => (pick(MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP), XBUTTON1),
        Button::Forward => (pick(MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP), XBUTTON2),
        Button::Other(n) => return Err(EmulateError::UnsupportedButton(n)),
    };
    Ok(WinInput::mouse(0, 0, data, flags))
}

/// `Other(n)` con un número que Windows sí tiene → su botón con nombre.
fn normalize_button(button: Button) -> Result<Button, EmulateError> {
    Ok(match button {
        Button::Other(0) => Button::Left,
        Button::Other(1) => Button::Right,
        Button::Other(2) => Button::Middle,
        Button::Other(3) => Button::Back,
        Button::Other(4) => Button::Forward,
        Button::Other(n) => return Err(EmulateError::UnsupportedButton(n)),
        b => b,
    })
}

#[cfg(test)]
mod tests;
