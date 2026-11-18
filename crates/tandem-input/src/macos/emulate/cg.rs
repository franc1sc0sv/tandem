//! Real [`MacSink`]: publishes with `CGEventPost` at `kCGHIDEventTap`.
//!
//! It requires the Accessibility permission (TCC) for the binary; without it macOS
//! drops the events silently.

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

/// Maximum number of screens requested from CoreGraphics; plenty for a desktop.
const MAX_DISPLAYS: usize = 32;

/// `NSEvent` `SystemDefined` subtype for auxiliary keys
/// (`NX_SUBTYPE_AUX_CONTROL_BUTTONS`).
const AUX_CONTROL_BUTTONS: i16 = 8;

pub struct CgSink {
    source: CFRetained<CGEventSource>,
}

// SAFETY: `CGEventSource` is a CF object with atomic reference counting;
// `CgSink` is its only owner and only uses it with `&mut self`, so it is never
// touched from two threads at once. Moving it across threads is safe.
unsafe impl Send for CgSink {}

impl std::fmt::Debug for CgSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CgSink").finish_non_exhaustive()
    }
}

impl CgSink {
    pub fn new() -> Result<Self, EmulateError> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .ok_or_else(|| platform("CGEventSourceCreate returned NULL"))?;
        // By default macOS ignores the local mouse and keyboard for 0.25 s after
        // each injected event; on the client Mac the user can also
        // use their trackpad.
        CGEventSource::set_local_events_suppression_interval(Some(&source), 0.0);
        Ok(Self { source })
    }

    fn post_cg(event: &CGEvent) {
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(event));
    }

    fn keyboard(&self, keycode: u16, down: bool) -> Result<CFRetained<CGEvent>, EmulateError> {
        CGEvent::new_keyboard_event(Some(&self.source), keycode, down)
            .ok_or_else(|| platform("CGEventCreateKeyboardEvent returned NULL"))
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
                .ok_or_else(|| platform("CGEventCreateMouseEvent returned NULL"))?;
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
                // The delta fields are integers; `as` rounds toward zero and saturates.
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
                .ok_or_else(|| platform("CGEventCreateScrollWheelEvent2 returned NULL"))?;
                CGEvent::set_flags(Some(&ev), CGEventFlags(flags));
                Self::post_cg(&ev);
            }
            MacEvent::SystemKey {
                data1,
                modifier_flags,
            } => {
                let modifier_flags = usize::try_from(modifier_flags)
                    .map_err(|_| platform("modifierFlags out of range"))?;
                let data1 = isize::try_from(data1).map_err(|_| platform("data1 out of range"))?;
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
                .ok_or_else(|| platform("NSEvent otherEventWithType returned nil"))?;
                let ev = ns
                    .CGEvent()
                    .ok_or_else(|| platform("NSEvent without CGEvent"))?;
                Self::post_cg(&ev);
            }
        }
        Ok(())
    }

    fn cursor(&mut self) -> Result<Point, EmulateError> {
        // An empty event carries the cursor's current position.
        let ev = CGEvent::new(None).ok_or_else(|| platform("CGEventCreate returned NULL"))?;
        let p = CGEvent::location(Some(&ev));
        Ok(Point::new(p.x, p.y))
    }

    fn screens(&mut self) -> Result<Vec<Rect>, EmulateError> {
        Ok(active_displays()?
            .into_iter()
            .map(|id| {
                let b = CGDisplayBounds(id);
                // The bounds in points are integers in practice; `as` saturates.
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
        // Without the key, macOS uses its factory value: natural scrolling on.
        defaults.objectForKey(key).is_none() || defaults.boolForKey(key)
    }
}

fn active_displays() -> Result<Vec<CGDirectDisplayID>, EmulateError> {
    let mut ids: [CGDirectDisplayID; MAX_DISPLAYS] = [0; MAX_DISPLAYS];
    let mut count: u32 = 0;
    // SAFETY: `ids` has room for `MAX_DISPLAYS` and `count` is a valid u32;
    // CoreGraphics writes no more than it is told.
    let err = unsafe { CGGetActiveDisplayList(MAX_DISPLAYS as u32, ids.as_mut_ptr(), &mut count) };
    if err != CGError::Success {
        return Err(EmulateError::Platform(format!(
            "CGGetActiveDisplayList failed: {}",
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
    fn coregraphics_mouse_types() {
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
    fn reads_screens_and_preferences() {
        let mut sink = CgSink::new().unwrap();
        let screens = sink.screens().unwrap();
        assert!(!screens.is_empty());
        let p = sink.cursor().unwrap();
        assert!(
            screens.iter().any(|r| r.contains(p)),
            "{p:?} outside {screens:?}"
        );
        assert!(sink.double_click_interval() > Duration::ZERO);
        eprintln!(
            "screens {screens:?}, cursor {p:?}, natural {}",
            sink.natural_scroll()
        );
    }

    /// Requires Accessibility. Moves the cursor 40 px and puts it back.
    #[test]
    #[ignore = "hardware"]
    fn moves_the_cursor_and_puts_it_back() {
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
