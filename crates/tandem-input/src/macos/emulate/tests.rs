use std::time::{Duration, Instant};

use proptest::prelude::*;

use super::flags::*;
use super::*;

fn emulator() -> MacEmulator<RecordingSink> {
    MacEmulator::new(RecordingSink::new()).unwrap()
}

fn key(id: u16, state: KeyState) -> InputEvent {
    InputEvent::Key {
        usage: HidUsage::key(id),
        state,
    }
}

fn press(id: u16) -> InputEvent {
    key(id, KeyState::Pressed)
}

fn release(id: u16) -> InputEvent {
    key(id, KeyState::Released)
}

fn btn(button: Button, state: KeyState) -> InputEvent {
    InputEvent::Button { button, state }
}

fn take(e: &mut MacEmulator<RecordingSink>) -> Vec<MacEvent> {
    e.sink_mut().take()
}

const NC: u64 = NON_COALESCED;

#[test]
fn a_simple_key_goes_down_and_up_with_its_keycode() {
    let mut e = emulator();
    e.emit(&press(0x04)).unwrap(); // A → kVK_ANSI_A (0x00)
    e.emit(&release(0x04)).unwrap();
    assert_eq!(
        take(&mut e),
        vec![
            MacEvent::Key {
                keycode: 0x00,
                down: true,
                autorepeat: false,
                flags: NC,
            },
            MacEvent::Key {
                keycode: 0x00,
                down: false,
                autorepeat: false,
                flags: NC,
            },
        ]
    );
}

#[test]
fn a_modifier_goes_as_flags_changed_and_marks_the_keys() {
    let mut e = emulator();
    e.emit(&press(0xE1)).unwrap(); // left Shift
    e.emit(&press(0x04)).unwrap();
    e.emit(&release(0x04)).unwrap();
    e.emit(&release(0xE1)).unwrap();
    let shift = NC | SHIFT | DEVICE_LEFT_SHIFT;
    assert_eq!(
        take(&mut e),
        vec![
            MacEvent::FlagsChanged {
                keycode: 0x38,
                flags: shift,
            },
            MacEvent::Key {
                keycode: 0x00,
                down: true,
                autorepeat: false,
                flags: shift,
            },
            MacEvent::Key {
                keycode: 0x00,
                down: false,
                autorepeat: false,
                flags: shift,
            },
            MacEvent::FlagsChanged {
                keycode: 0x38,
                flags: NC,
            },
        ]
    );
}

#[test]
fn each_modifier_carries_its_side_and_its_keycode() {
    // (HID, kVK, flags)
    let cases = [
        (0xE0, 0x3B, CONTROL | DEVICE_LEFT_CONTROL),
        (0xE1, 0x38, SHIFT | DEVICE_LEFT_SHIFT),
        (0xE2, 0x3A, ALTERNATE | DEVICE_LEFT_ALTERNATE),
        (0xE3, 0x37, COMMAND | DEVICE_LEFT_COMMAND),
        (0xE4, 0x3E, CONTROL | DEVICE_RIGHT_CONTROL),
        (0xE5, 0x3C, SHIFT | DEVICE_RIGHT_SHIFT),
        (0xE6, 0x3D, ALTERNATE | DEVICE_RIGHT_ALTERNATE),
        (0xE7, 0x36, COMMAND | DEVICE_RIGHT_COMMAND),
    ];
    for (hid, kvk, f) in cases {
        let mut e = emulator();
        e.emit(&press(hid)).unwrap();
        assert_eq!(
            take(&mut e),
            vec![MacEvent::FlagsChanged {
                keycode: kvk,
                flags: NC | f,
            }],
            "HID {hid:#x}"
        );
    }
}

#[test]
fn releasing_one_side_keeps_the_other() {
    let mut e = emulator();
    e.emit(&press(0xE1)).unwrap();
    e.emit(&press(0xE5)).unwrap();
    e.emit(&release(0xE1)).unwrap();
    let events = take(&mut e);
    assert_eq!(
        events.last(),
        Some(&MacEvent::FlagsChanged {
            keycode: 0x38,
            flags: NC | SHIFT | DEVICE_RIGHT_SHIFT,
        })
    );
}

#[test]
fn repeating_a_key_marks_autorepeat() {
    let mut e = emulator();
    for _ in 0..3 {
        e.emit(&press(0x04)).unwrap();
    }
    let repeats: Vec<bool> = take(&mut e)
        .into_iter()
        .map(|ev| match ev {
            MacEvent::Key { autorepeat, .. } => autorepeat,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(repeats, vec![false, true, true]);
}

#[test]
fn a_held_modifier_does_not_repeat() {
    let mut e = emulator();
    e.emit(&press(0xE3)).unwrap();
    e.emit(&press(0xE3)).unwrap();
    assert_eq!(take(&mut e).len(), 1);
}

#[test]
fn releasing_something_not_pressed_publishes_nothing() {
    let mut e = emulator();
    e.emit(&release(0x04)).unwrap();
    e.emit(&release(0xE0)).unwrap();
    e.emit(&btn(Button::Left, KeyState::Released)).unwrap();
    assert!(take(&mut e).is_empty());
}

#[test]
fn arrows_f_keys_and_numeric_keypad_carry_their_flags() {
    // (HID, kVK, flags)
    let cases = [
        (0x52, 0x7E, NUMERIC_PAD | SECONDARY_FN), // ↑
        (0x51, 0x7D, NUMERIC_PAD | SECONDARY_FN), // ↓
        (0x50, 0x7B, NUMERIC_PAD | SECONDARY_FN), // ←
        (0x4F, 0x7C, NUMERIC_PAD | SECONDARY_FN), // →
        (0x3E, 0x60, SECONDARY_FN),               // F5
        (0x45, 0x6F, SECONDARY_FN),               // F12
        (0x4A, 0x73, SECONDARY_FN),               // Home
        (0x4C, 0x75, SECONDARY_FN),               // Delete
        (0x59, 0x53, NUMERIC_PAD),                // Num 1
        (0x58, 0x4C, NUMERIC_PAD),                // Num Enter
        (0x28, 0x24, 0),                          // Enter
    ];
    for (hid, kvk, f) in cases {
        let mut e = emulator();
        e.emit(&press(hid)).unwrap();
        assert_eq!(
            take(&mut e),
            vec![MacEvent::Key {
                keycode: kvk,
                down: true,
                autorepeat: false,
                flags: NC | f,
            }],
            "HID {hid:#x}"
        );
    }
}

#[test]
fn caps_lock_toggles_on_press() {
    let mut e = emulator();
    e.emit(&press(0x39)).unwrap();
    e.emit(&press(0x39)).unwrap(); // repeat: nothing
    e.emit(&release(0x39)).unwrap(); // release: nothing
    e.emit(&press(0x04)).unwrap();
    e.emit(&press(0x39)).unwrap();
    assert_eq!(
        take(&mut e),
        vec![
            MacEvent::FlagsChanged {
                keycode: 0x39,
                flags: NC | ALPHA_SHIFT,
            },
            MacEvent::Key {
                keycode: 0x00,
                down: true,
                autorepeat: false,
                flags: NC | ALPHA_SHIFT,
            },
            MacEvent::FlagsChanged {
                keycode: 0x39,
                flags: NC,
            },
        ]
    );
}

#[test]
fn a_key_without_keycode_fails_and_is_not_left_pressed() {
    let mut e = emulator();
    let unknown = HidUsage::new(0xFF00, 1);
    let err = e
        .emit(&InputEvent::Key {
            usage: unknown,
            state: KeyState::Pressed,
        })
        .unwrap_err();
    assert!(matches!(err, EmulateError::Unmapped(u) if u == unknown));
    e.release_all().unwrap();
    assert!(take(&mut e).is_empty());
}

#[test]
fn media_goes_as_a_system_event() {
    // (usage, NX_KEYTYPE)
    let cases = [
        (HidUsage::consumer(0xCD), 16),
        (HidUsage::consumer(0xB5), 17),
        (HidUsage::consumer(0xB6), 18),
        (HidUsage::consumer(0xE2), 7),
        (HidUsage::consumer(0xE9), 0),
        (HidUsage::consumer(0xEA), 1),
        (HidUsage::consumer(0x6F), 2),
        (HidUsage::consumer(0x70), 3),
        (HidUsage::consumer(0xB8), 14),
        (HidUsage::key(0x7F), 7),
        (HidUsage::key(0x80), 0),
        (HidUsage::key(0x81), 1),
    ];
    for (usage, nx) in cases {
        let mut e = emulator();
        for state in [KeyState::Pressed, KeyState::Pressed, KeyState::Released] {
            e.emit(&InputEvent::Key { usage, state }).unwrap();
        }
        assert_eq!(
            take(&mut e),
            vec![
                MacEvent::SystemKey {
                    data1: (nx << 16) | 0x0A00,
                    modifier_flags: 0xA00,
                },
                MacEvent::SystemKey {
                    data1: (nx << 16) | 0x0A01,
                    modifier_flags: 0xA00,
                },
                MacEvent::SystemKey {
                    data1: (nx << 16) | 0x0B00,
                    modifier_flags: 0xB00,
                },
            ],
            "{usage:?}"
        );
    }
}

#[test]
fn relative_and_absolute_movement() {
    let mut e = emulator();
    e.emit(&InputEvent::MotionRelative {
        dx: 10.5,
        dy: -20.0,
    })
    .unwrap();
    e.emit(&InputEvent::MotionAbsolute { x: 100.0, y: 200.0 })
        .unwrap();
    let events = take(&mut e);
    assert_eq!(
        events,
        vec![
            MacEvent::Mouse {
                kind: MouseKind::Moved,
                pos: Point::new(970.5, 520.0),
                button: 0,
                click_state: 0,
                delta: (10.5, -20.0),
                flags: NC,
            },
            MacEvent::Mouse {
                kind: MouseKind::Moved,
                pos: Point::new(100.0, 200.0),
                button: 0,
                click_state: 0,
                delta: (-870.5, -320.0),
                flags: NC,
            },
        ]
    );
}

#[test]
fn the_cursor_does_not_leave_the_screens() {
    let mut sink = RecordingSink::new();
    // Main 1920×1080 and one to the left, higher up.
    sink.screens = vec![
        Rect::new(0, 0, 1920, 1080),
        Rect::new(-1440, -200, 1440, 900),
    ];
    sink.cursor = Point::new(5.0, 1000.0);
    let mut e = MacEmulator::new(sink).unwrap();
    // On the left, at a height where there is no screen: it stays at the edge.
    e.emit(&InputEvent::MotionRelative { dx: -50.0, dy: 0.0 })
        .unwrap();
    assert_eq!(e.sink().cursor, Point::new(0.0, 1000.0));
    // Higher up it does enter the other screen.
    e.emit(&InputEvent::MotionAbsolute { x: -30.0, y: 100.0 })
        .unwrap();
    assert_eq!(e.sink().cursor, Point::new(-30.0, 100.0));
    e.emit(&InputEvent::MotionAbsolute {
        x: 5000.0,
        y: 5000.0,
    })
    .unwrap();
    assert_eq!(e.sink().cursor, Point::new(1919.0, 1079.0));
}

#[test]
fn without_screens_it_does_not_move() {
    let mut sink = RecordingSink::new();
    sink.screens.clear();
    let mut e = MacEmulator::new(sink).unwrap();
    let err = e
        .emit(&InputEvent::MotionRelative { dx: 1.0, dy: 1.0 })
        .unwrap_err();
    assert!(matches!(err, EmulateError::NoScreens));
}

fn mouse_kinds(events: &[MacEvent]) -> Vec<(MouseKind, u32)> {
    events
        .iter()
        .filter_map(|ev| match ev {
            MacEvent::Mouse { kind, button, .. } => Some((*kind, *button)),
            _ => None,
        })
        .collect()
}

#[test]
fn with_a_pressed_button_the_movement_is_a_drag() {
    let mut e = emulator();
    let mv = InputEvent::MotionRelative { dx: 1.0, dy: 0.0 };
    e.emit(&btn(Button::Left, KeyState::Pressed)).unwrap();
    e.emit(&mv).unwrap();
    e.emit(&btn(Button::Right, KeyState::Pressed)).unwrap();
    e.emit(&mv).unwrap(); // left and right: the left one sends
    e.emit(&btn(Button::Left, KeyState::Released)).unwrap();
    e.emit(&mv).unwrap();
    e.emit(&btn(Button::Right, KeyState::Released)).unwrap();
    e.emit(&btn(Button::Middle, KeyState::Pressed)).unwrap();
    e.emit(&mv).unwrap();
    e.emit(&btn(Button::Middle, KeyState::Released)).unwrap();
    e.emit(&btn(Button::Other(9), KeyState::Pressed)).unwrap();
    e.emit(&mv).unwrap();
    e.emit(&btn(Button::Other(9), KeyState::Released)).unwrap();
    e.emit(&mv).unwrap();
    assert_eq!(
        mouse_kinds(&take(&mut e)),
        vec![
            (MouseKind::LeftDown, 0),
            (MouseKind::LeftDragged, 0),
            (MouseKind::RightDown, 1),
            (MouseKind::LeftDragged, 0),
            (MouseKind::LeftUp, 0),
            (MouseKind::RightDragged, 1),
            (MouseKind::RightUp, 1),
            (MouseKind::OtherDown, 2),
            (MouseKind::OtherDragged, 2),
            (MouseKind::OtherUp, 2),
            (MouseKind::OtherDown, 9),
            (MouseKind::OtherDragged, 9),
            (MouseKind::OtherUp, 9),
            (MouseKind::Moved, 0),
        ]
    );
}

#[test]
fn side_buttons_are_3_and_4_and_low_other_is_the_same_button() {
    let mut e = emulator();
    e.emit(&btn(Button::Back, KeyState::Pressed)).unwrap();
    e.emit(&btn(Button::Forward, KeyState::Pressed)).unwrap();
    e.emit(&btn(Button::Other(0), KeyState::Pressed)).unwrap();
    // It is already pressed as Other(0): Left does not go down again.
    e.emit(&btn(Button::Left, KeyState::Pressed)).unwrap();
    e.emit(&btn(Button::Left, KeyState::Released)).unwrap();
    assert_eq!(
        mouse_kinds(&take(&mut e)),
        vec![
            (MouseKind::OtherDown, 3),
            (MouseKind::OtherDown, 4),
            (MouseKind::LeftDown, 0),
            (MouseKind::LeftUp, 0),
        ]
    );
}

fn click_states(events: &[MacEvent]) -> Vec<i64> {
    events
        .iter()
        .filter_map(|ev| match ev {
            MacEvent::Mouse { click_state, .. } => Some(*click_state),
            _ => None,
        })
        .collect()
}

fn click(e: &mut MacEmulator<RecordingSink>, button: Button, at: Instant) {
    e.emit_at(&btn(button, KeyState::Pressed), at).unwrap();
    e.emit_at(&btn(button, KeyState::Released), at).unwrap();
}

#[test]
fn double_and_triple_click_within_the_window() {
    let mut e = emulator();
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    click(&mut e, Button::Left, t0);
    click(&mut e, Button::Left, t0 + ms(200));
    click(&mut e, Button::Left, t0 + ms(400));
    // Outside the window: it starts over.
    click(&mut e, Button::Left, t0 + ms(1000));
    assert_eq!(click_states(&take(&mut e)), vec![1, 1, 2, 2, 3, 3, 1, 1]);
}

#[test]
fn the_double_click_requires_the_same_button_and_the_same_place() {
    let mut e = emulator();
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    click(&mut e, Button::Left, t0);
    click(&mut e, Button::Right, t0 + ms(100));
    e.emit_at(&InputEvent::MotionRelative { dx: 3.0, dy: 0.0 }, t0)
        .unwrap();
    click(&mut e, Button::Right, t0 + ms(200)); // at 3 px: counts
    e.emit_at(&InputEvent::MotionRelative { dx: 10.0, dy: 0.0 }, t0)
        .unwrap();
    click(&mut e, Button::Right, t0 + ms(300)); // at 10 px: does not
    assert_eq!(
        click_states(&take(&mut e)),
        vec![1, 1, 1, 1, 0, 2, 2, 0, 1, 1]
    );
}

#[test]
fn the_double_click_window_is_the_macs() {
    let mut sink = RecordingSink::new();
    sink.double_click = Duration::from_millis(150);
    let mut e = MacEmulator::new(sink).unwrap();
    let t0 = Instant::now();
    click(&mut e, Button::Left, t0);
    click(&mut e, Button::Left, t0 + Duration::from_millis(200));
    assert_eq!(click_states(&take(&mut e)), vec![1, 1, 1, 1]);
}

#[test]
fn clicks_carry_the_modifiers() {
    let mut e = emulator();
    e.emit(&press(0xE3)).unwrap();
    e.emit(&btn(Button::Left, KeyState::Pressed)).unwrap();
    let events = take(&mut e);
    assert!(matches!(
        events.last(),
        Some(MacEvent::Mouse { flags, .. }) if *flags == NC | COMMAND | DEVICE_LEFT_COMMAND
    ));
}

fn scrolls(events: &[MacEvent]) -> Vec<(ScrollUnit, i32, i32)> {
    events
        .iter()
        .filter_map(|ev| match ev {
            MacEvent::Scroll {
                unit,
                wheel1,
                wheel2,
                ..
            } => Some((*unit, *wheel1, *wheel2)),
            _ => None,
        })
        .collect()
}

fn wheel(dx_v120: i32, dy_v120: i32) -> InputEvent {
    InputEvent::Scroll(Scroll::Wheel { dx_v120, dy_v120 })
}

#[test]
fn the_wheel_goes_in_lines_and_accumulates_fractions() {
    let mut e = emulator();
    e.emit(&wheel(0, 120)).unwrap(); // forward
    e.emit(&wheel(0, -240)).unwrap(); // back, two notches
    e.emit(&wheel(0, 60)).unwrap(); // half: nothing yet
    e.emit(&wheel(0, 60)).unwrap(); // the other half
    e.emit(&wheel(120, 0)).unwrap(); // right
    assert_eq!(
        scrolls(&take(&mut e)),
        vec![
            (ScrollUnit::Line, 1, 0),
            (ScrollUnit::Line, -2, 0),
            (ScrollUnit::Line, 1, 0),
            (ScrollUnit::Line, 0, -1),
        ]
    );
}

#[test]
fn natural_scrolling_inverts_both_axes() {
    let mut sink = RecordingSink::new();
    sink.natural_scroll = true;
    let mut e = MacEmulator::new(sink).unwrap();
    e.emit(&wheel(120, 120)).unwrap();
    e.emit(&InputEvent::Scroll(Scroll::Pixels { dx: -5.0, dy: 7.0 }))
        .unwrap();
    assert_eq!(
        scrolls(&take(&mut e)),
        vec![(ScrollUnit::Line, -1, 1), (ScrollUnit::Pixel, -7, -5)]
    );
}

#[test]
fn the_trackpad_goes_in_pixels_and_accumulates_fractions() {
    let mut e = emulator();
    for _ in 0..4 {
        e.emit(&InputEvent::Scroll(Scroll::Pixels { dx: 0.0, dy: 0.4 }))
            .unwrap();
    }
    e.emit(&InputEvent::Scroll(Scroll::Pixels { dx: -12.5, dy: 0.0 }))
        .unwrap();
    assert_eq!(
        scrolls(&take(&mut e)),
        vec![
            (ScrollUnit::Pixel, 1, 0), // 0.4 + 0.4 + 0.4
            (ScrollUnit::Pixel, 0, 12),
        ]
    );
}

#[test]
fn synchronizing_modifiers_releases_and_presses_what_is_needed() {
    let mut e = emulator();
    e.emit(&press(0xE0)).unwrap(); // Ctrl
    e.emit(&press(0xE1)).unwrap(); // Shift
    take(&mut e);
    let target: Modifiers = [ModifierKey::ShiftLeft, ModifierKey::MetaLeft]
        .into_iter()
        .collect();
    e.emit(&InputEvent::Modifiers(target)).unwrap();
    assert_eq!(
        take(&mut e),
        vec![
            MacEvent::FlagsChanged {
                keycode: 0x3B,
                flags: NC | SHIFT | DEVICE_LEFT_SHIFT,
            },
            MacEvent::FlagsChanged {
                keycode: 0x37,
                flags: NC | SHIFT | DEVICE_LEFT_SHIFT | COMMAND | DEVICE_LEFT_COMMAND,
            },
        ]
    );
    // Already synchronized: nothing.
    e.emit(&InputEvent::Modifiers(target)).unwrap();
    assert!(take(&mut e).is_empty());
}

#[test]
fn release_all_releases_buttons_keys_and_last_modifiers() {
    let mut e = emulator();
    e.emit(&press(0xE3)).unwrap(); // Cmd
    e.emit(&press(0x06)).unwrap(); // C
    e.emit(&btn(Button::Left, KeyState::Pressed)).unwrap();
    e.emit(&InputEvent::Key {
        usage: HidUsage::consumer(0xCD),
        state: KeyState::Pressed,
    })
    .unwrap();
    take(&mut e);
    e.release_all().unwrap();
    let events = take(&mut e);
    let cmd = NC | COMMAND | DEVICE_LEFT_COMMAND;
    assert!(matches!(
        events[0],
        MacEvent::Mouse { kind: MouseKind::LeftUp, flags, .. } if flags == cmd
    ));
    assert_eq!(
        events[1],
        MacEvent::Key {
            keycode: 0x08,
            down: false,
            autorepeat: false,
            flags: cmd,
        }
    );
    assert_eq!(
        events[2],
        MacEvent::SystemKey {
            data1: (16 << 16) | 0x0B00,
            modifier_flags: 0xB00,
        }
    );
    assert_eq!(
        events[3],
        MacEvent::FlagsChanged {
            keycode: 0x37,
            flags: NC,
        }
    );
    assert_eq!(events.len(), 4);
    // After that, the movement is no longer a drag.
    e.emit(&InputEvent::MotionRelative { dx: 1.0, dy: 1.0 })
        .unwrap();
    assert_eq!(mouse_kinds(&take(&mut e)), vec![(MouseKind::Moved, 0)]);
}

#[test]
fn release_all_goes_on_even_if_an_event_fails_and_returns_the_error() {
    let mut e = emulator();
    e.emit(&press(0x04)).unwrap();
    e.emit(&press(0xE0)).unwrap();
    take(&mut e);
    e.sink_mut().fail_post = Some("no permission".into());
    let err = e.release_all().unwrap_err();
    assert!(matches!(err, EmulateError::Platform(m) if m == "no permission"));
    // The state was left clean: there is nothing left to release.
    e.sink_mut().fail_post = None;
    e.release_all().unwrap();
    assert!(take(&mut e).is_empty());
}

#[test]
fn refresh_rereads_screens_and_preferences() {
    let mut e = emulator();
    e.sink_mut().screens = vec![Rect::new(0, 0, 100, 100)];
    e.sink_mut().natural_scroll = true;
    e.refresh().unwrap();
    e.emit(&InputEvent::MotionAbsolute { x: 500.0, y: 50.0 })
        .unwrap();
    assert_eq!(e.sink().cursor, Point::new(99.0, 50.0));
    e.emit(&wheel(0, 120)).unwrap();
    assert_eq!(scrolls(&take(&mut e)), vec![(ScrollUnit::Line, -1, 0)]);
}

#[test]
fn data1_of_system_keys() {
    assert_eq!(system_key_data1(16, true, false), 0x10_0A00);
    assert_eq!(system_key_data1(16, false, false), 0x10_0B00);
    assert_eq!(system_key_data1(0, true, true), 0x0A01);
}

// --- Propiedades ---

fn any_usage() -> impl Strategy<Value = HidUsage> {
    prop_oneof![
        (0x04u16..=0x65).prop_map(HidUsage::key),
        (0xE0u16..=0xE7).prop_map(HidUsage::key),
        Just(HidUsage::consumer(0xCD)),
        Just(HidUsage::key(0x80)),
    ]
}

fn any_button() -> impl Strategy<Value = Button> {
    prop_oneof![
        Just(Button::Left),
        Just(Button::Right),
        Just(Button::Middle),
        Just(Button::Back),
        Just(Button::Forward),
        (0u8..12).prop_map(Button::Other),
    ]
}

fn any_state() -> impl Strategy<Value = KeyState> {
    prop_oneof![Just(KeyState::Pressed), Just(KeyState::Released)]
}

fn any_event() -> impl Strategy<Value = InputEvent> {
    prop_oneof![
        (any_usage(), any_state()).prop_map(|(usage, state)| InputEvent::Key { usage, state }),
        (any_button(), any_state())
            .prop_map(|(button, state)| InputEvent::Button { button, state }),
        (-3000.0..3000.0f64, -3000.0..3000.0f64)
            .prop_map(|(dx, dy)| InputEvent::MotionRelative { dx, dy }),
        (-5000.0..5000.0f64, -5000.0..5000.0f64)
            .prop_map(|(x, y)| InputEvent::MotionAbsolute { x, y }),
        (-500i32..500, -500i32..500)
            .prop_map(|(dx_v120, dy_v120)| InputEvent::Scroll(Scroll::Wheel { dx_v120, dy_v120 })),
        any::<u8>().prop_map(|b| InputEvent::Modifiers(Modifiers::from_bits(b))),
    ]
}

/// Some keys in the range do not exist on the Mac; that is not a failure here.
fn emit_lenient(e: &mut MacEmulator<RecordingSink>, ev: &InputEvent) {
    match e.emit(ev) {
        Ok(()) | Err(EmulateError::Unmapped(_)) => {}
        Err(err) => panic!("{ev:?}: {err}"),
    }
}

proptest! {
    #[test]
    fn after_release_all_nothing_is_left_pressed(
        events in prop::collection::vec(any_event(), 0..60),
    ) {
        let mut e = emulator();
        for ev in &events {
            emit_lenient(&mut e, ev);
        }
        e.release_all().unwrap();
        take(&mut e);
        // A key and a movement no longer carry modifiers nor are a drag.
        e.emit(&press(0x04)).unwrap();
        e.emit(&InputEvent::MotionRelative { dx: 1.0, dy: 0.0 }).unwrap();
        let out = take(&mut e);
        let caps = if e.caps_lock { ALPHA_SHIFT } else { 0 };
        let key_ok = matches!(out[0], MacEvent::Key { flags, .. } if flags == NC | caps);
        prop_assert!(key_ok);
        prop_assert_eq!(mouse_kinds(&out), vec![(MouseKind::Moved, 0)]);
    }

    #[test]
    fn the_cursor_always_ends_up_on_a_screen(
        events in prop::collection::vec(any_event(), 0..60),
    ) {
        let mut sink = RecordingSink::new();
        sink.screens = vec![Rect::new(0, 0, 1920, 1080), Rect::new(1920, -300, 2560, 1440)];
        let mut e = MacEmulator::new(sink).unwrap();
        for ev in &events {
            emit_lenient(&mut e, ev);
        }
        for ev in e.sink().events.iter() {
            if let MacEvent::Mouse { pos, .. } = ev {
                prop_assert!(e.sink().screens.iter().any(|r| r.contains(*pos)), "{pos:?}");
            }
        }
    }

    #[test]
    fn the_wheel_neither_loses_nor_invents_notches(
        deltas in prop::collection::vec((-400i32..400, -400i32..400), 0..40),
    ) {
        let mut e = emulator();
        for (dx, dy) in &deltas {
            e.emit(&wheel(*dx, *dy)).unwrap();
        }
        let (mut lines_x, mut lines_y) = (0i32, 0i32);
        for (_, w1, w2) in scrolls(&take(&mut e)) {
            lines_y += w1;
            lines_x -= w2;
        }
        let total_x: i32 = deltas.iter().map(|d| d.0).sum();
        let total_y: i32 = deltas.iter().map(|d| d.1).sum();
        prop_assert_eq!(lines_x * 120 + e.wheel_rest.0, total_x);
        prop_assert_eq!(lines_y * 120 + e.wheel_rest.1, total_y);
        prop_assert!(e.wheel_rest.0.abs() < 120 && e.wheel_rest.1.abs() < 120);
    }

    #[test]
    fn the_flags_tell_each_modifier_combination_apart(a: u8, b: u8) {
        let fa = modifier_flags(Modifiers::from_bits(a), false);
        let fb = modifier_flags(Modifiers::from_bits(b), false);
        prop_assert_eq!(fa == fb, a == b);
    }
}
