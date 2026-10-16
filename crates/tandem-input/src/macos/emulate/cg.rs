//! [`MacSink`] real: publica con `CGEventPost` en `kCGHIDEventTap`.
//!
//! Requiere el permiso de Accesibilidad (TCC) para el binario; sin él macOS
//! descarta los eventos en silencio.

use std::time::Duration;

use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
use objc2_core_foundation::{CFRetained, CGPoint};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGError, CGEvent, CGEventField, CGEventFlags,
    CGEventSource, CGEventSourceStateID, CGEventTapLocation, CGEventType, CGGetActiveDisplayList,
    CGMouseButton, CGScrollEventUnit,
};
use objc2_foundation::{NSUserDefaults, ns_string};

use super::{EmulateError, MacEvent, MacSink, MouseKind, ScrollUnit};
use crate::geometry::{Point, Rect};

/// Máximo de pantallas que se piden a CoreGraphics; sobra para un escritorio.
const MAX_DISPLAYS: usize = 32;

/// Subtipo de `NSEvent` `SystemDefined` para las teclas auxiliares
/// (`NX_SUBTYPE_AUX_CONTROL_BUTTONS`).
const AUX_CONTROL_BUTTONS: i16 = 8;

pub struct CgSink {
    source: CFRetained<CGEventSource>,
}

// SAFETY: `CGEventSource` es un objeto CF con conteo de referencias atómico;
// `CgSink` es su único dueño y solo lo usa con `&mut self`, así que nunca se
// toca desde dos hilos a la vez. Moverlo de hilo es seguro.
unsafe impl Send for CgSink {}

impl std::fmt::Debug for CgSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CgSink").finish_non_exhaustive()
    }
}

impl CgSink {
    pub fn new() -> Result<Self, EmulateError> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .ok_or_else(|| platform("CGEventSourceCreate devolvió NULL"))?;
        // Por defecto macOS ignora el mouse y el teclado locales 0,25 s después
        // de cada evento inyectado; en la Mac cliente el usuario también puede
        // usar su trackpad.
        CGEventSource::set_local_events_suppression_interval(Some(&source), 0.0);
        Ok(Self { source })
    }

    fn post_cg(event: &CGEvent) {
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(event));
    }

    fn keyboard(&self, keycode: u16, down: bool) -> Result<CFRetained<CGEvent>, EmulateError> {
        CGEvent::new_keyboard_event(Some(&self.source), keycode, down)
            .ok_or_else(|| platform("CGEventCreateKeyboardEvent devolvió NULL"))
    }
}

fn platform(msg: &str) -> EmulateError {
    EmulateError::Platform(msg.to_owned())
}

fn mouse_type(kind: MouseKind) -> CGEventType {
    match kind {
        MouseKind::Moved => CGEventType::MouseMoved,
        MouseKind::LeftDown => CGEventType::LeftMouseDown,
        MouseKind::LeftUp => CGEventType::LeftMouseUp,
        MouseKind::LeftDragged => CGEventType::LeftMouseDragged,
        MouseKind::RightDown => CGEventType::RightMouseDown,
        MouseKind::RightUp => CGEventType::RightMouseUp,
        MouseKind::RightDragged => CGEventType::RightMouseDragged,
        MouseKind::OtherDown => CGEventType::OtherMouseDown,
        MouseKind::OtherUp => CGEventType::OtherMouseUp,
        MouseKind::OtherDragged => CGEventType::OtherMouseDragged,
    }
}

impl MacSink for CgSink {
    fn post(&mut self, event: &MacEvent) -> Result<(), EmulateError> {
        match *event {
            MacEvent::Key {
                keycode,
                down,
                autorepeat,
                flags,
            } => {
                let ev = self.keyboard(keycode, down)?;
                CGEvent::set_flags(Some(&ev), CGEventFlags(flags));
                CGEvent::set_integer_value_field(
                    Some(&ev),
                    CGEventField::KeyboardEventAutorepeat,
                    i64::from(autorepeat),
                );
                Self::post_cg(&ev);
            }
            MacEvent::FlagsChanged { keycode, flags } => {
                let ev = self.keyboard(keycode, true)?;
                CGEvent::set_type(Some(&ev), CGEventType::FlagsChanged);
                CGEvent::set_flags(Some(&ev), CGEventFlags(flags));
                Self::post_cg(&ev);
            }
            MacEvent::Mouse {
                kind,
                pos,
                button,
                click_state,
                delta,
                flags,
            } => {
                let ev = CGEvent::new_mouse_event(
                    Some(&self.source),
                    mouse_type(kind),
                    CGPoint::new(pos.x, pos.y),
                    CGMouseButton(button),
                )
                .ok_or_else(|| platform("CGEventCreateMouseEvent devolvió NULL"))?;
                CGEvent::set_integer_value_field(
                    Some(&ev),
                    CGEventField::MouseEventClickState,
                    click_state,
                );
                CGEvent::set_integer_value_field(
                    Some(&ev),
                    CGEventField::MouseEventButtonNumber,
                    i64::from(button),
                );
                // Los campos de delta son enteros; `as` redondea hacia cero y satura.
                CGEvent::set_integer_value_field(
                    Some(&ev),
                    CGEventField::MouseEventDeltaX,
                    delta.0.round() as i64,
                );
                CGEvent::set_integer_value_field(
                    Some(&ev),
                    CGEventField::MouseEventDeltaY,
                    delta.1.round() as i64,
                );
                CGEvent::set_flags(Some(&ev), CGEventFlags(flags));
                Self::post_cg(&ev);
            }
            MacEvent::Scroll {
                unit,
                wheel1,
                wheel2,
                flags,
            } => {
                let unit = match unit {
                    ScrollUnit::Line => CGScrollEventUnit::Line,
                    ScrollUnit::Pixel => CGScrollEventUnit::Pixel,
                };
                let ev = CGEvent::new_scroll_wheel_event2(
                    Some(&self.source),
                    unit,
                    2,
                    wheel1,
                    wheel2,
                    0,
                )
                .ok_or_else(|| platform("CGEventCreateScrollWheelEvent2 devolvió NULL"))?;
                CGEvent::set_flags(Some(&ev), CGEventFlags(flags));
                Self::post_cg(&ev);
            }
            MacEvent::SystemKey {
                data1,
                modifier_flags,
            } => {
                let modifier_flags = usize::try_from(modifier_flags)
                    .map_err(|_| platform("modifierFlags fuera de rango"))?;
                let data1 = isize::try_from(data1).map_err(|_| platform("data1 fuera de rango"))?;
                let ns = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
                    NSEventType::SystemDefined,
                    CGPoint::new(0.0, 0.0),
                    NSEventModifierFlags(modifier_flags),
                    0.0,
                    0,
                    None,
                    AUX_CONTROL_BUTTONS,
                    data1,
                    -1,
                )
                .ok_or_else(|| platform("NSEvent otherEventWithType devolvió nil"))?;
                let ev = ns
                    .CGEvent()
                    .ok_or_else(|| platform("NSEvent sin CGEvent"))?;
                Self::post_cg(&ev);
            }
        }
        Ok(())
    }

    fn cursor(&mut self) -> Result<Point, EmulateError> {
        // Un evento vacío trae la posición actual del cursor.
        let ev = CGEvent::new(None).ok_or_else(|| platform("CGEventCreate devolvió NULL"))?;
        let p = CGEvent::location(Some(&ev));
        Ok(Point::new(p.x, p.y))
    }

    fn screens(&mut self) -> Result<Vec<Rect>, EmulateError> {
        Ok(active_displays()?
            .into_iter()
            .map(|id| {
                let b = CGDisplayBounds(id);
                // Los límites en puntos son enteros en la práctica; `as` satura.
                Rect::new(
                    b.origin.x.round() as i32,
                    b.origin.y.round() as i32,
                    b.size.width.round() as u32,
                    b.size.height.round() as u32,
                )
            })
            .filter(|r| r.width > 0 && r.height > 0)
            .collect())
    }

    fn double_click_interval(&mut self) -> Duration {
        Duration::try_from_secs_f64(NSEvent::doubleClickInterval())
            .unwrap_or(Duration::from_millis(500))
    }

    fn natural_scroll(&mut self) -> bool {
        let defaults = NSUserDefaults::standardUserDefaults();
        let key = ns_string!("com.apple.swipescrolldirection");
        // Sin la clave, macOS usa su valor de fábrica: scroll natural activado.
        defaults.objectForKey(key).is_none() || defaults.boolForKey(key)
    }
}

fn active_displays() -> Result<Vec<CGDirectDisplayID>, EmulateError> {
    let mut ids: [CGDirectDisplayID; MAX_DISPLAYS] = [0; MAX_DISPLAYS];
    let mut count: u32 = 0;
    // SAFETY: `ids` tiene espacio para `MAX_DISPLAYS` y `count` es un u32 válido;
    // CoreGraphics no escribe más de lo que se le indica.
    let err = unsafe { CGGetActiveDisplayList(MAX_DISPLAYS as u32, ids.as_mut_ptr(), &mut count) };
    if err != CGError::Success {
        return Err(EmulateError::Platform(format!(
            "CGGetActiveDisplayList falló: {}",
            err.0
        )));
    }
    let count = (count as usize).min(MAX_DISPLAYS);
    Ok(ids[..count].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InputEvent;
    use crate::macos::emulate::MacEmulator;

    #[test]
    fn tipos_de_mouse_de_coregraphics() {
        let cases = [
            (MouseKind::Moved, 5),
            (MouseKind::LeftDown, 1),
            (MouseKind::LeftUp, 2),
            (MouseKind::LeftDragged, 6),
            (MouseKind::RightDown, 3),
            (MouseKind::RightUp, 4),
            (MouseKind::RightDragged, 7),
            (MouseKind::OtherDown, 25),
            (MouseKind::OtherUp, 26),
            (MouseKind::OtherDragged, 27),
        ];
        for (kind, raw) in cases {
            assert_eq!(mouse_type(kind), CGEventType(raw), "{kind:?}");
        }
    }

    #[test]
    #[ignore = "hardware"]
    fn lee_pantallas_y_preferencias() {
        let mut sink = CgSink::new().unwrap();
        let screens = sink.screens().unwrap();
        assert!(!screens.is_empty());
        let p = sink.cursor().unwrap();
        assert!(
            screens.iter().any(|r| r.contains(p)),
            "{p:?} fuera de {screens:?}"
        );
        assert!(sink.double_click_interval() > Duration::ZERO);
        eprintln!(
            "pantallas {screens:?}, cursor {p:?}, natural {}",
            sink.natural_scroll()
        );
    }

    /// Requiere Accesibilidad. Mueve el cursor 40 px y lo devuelve.
    #[test]
    #[ignore = "hardware"]
    fn mueve_el_cursor_y_lo_devuelve() {
        let mut e = MacEmulator::new(CgSink::new().unwrap()).unwrap();
        let start = e.sink_mut().cursor().unwrap();
        e.emit(&InputEvent::MotionRelative { dx: 40.0, dy: 0.0 })
            .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let moved = e.sink_mut().cursor().unwrap();
        e.emit(&InputEvent::MotionAbsolute {
            x: start.x,
            y: start.y,
        })
        .unwrap();
        assert!((moved.x - start.x).abs() > 1.0, "{start:?} → {moved:?}");
    }
}
