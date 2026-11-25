use proptest::prelude::*;

use super::*;
use crate::event::ModifierKey;

/// `SendInput` double: stores each call and moves the cursor as Windows would.
struct FakeSink {
    desktop: Desktop,
    cursor: Option<(i32, i32)>,
    calls: Vec<Vec<WinInput>>,
    blocked: bool,
    /// Windows has no mouse: it does not draw the cursor unless MouseKeys is on.
    no_mouse: bool,
    mousekeys: bool,
    /// Each call to `force_cursor`.
    forced: Vec<bool>,
}

impl FakeSink {
    fn new(desktop: Desktop, cursor: (i32, i32)) -> Self {
        Self {
            desktop,
            cursor: Some(cursor),
            calls: Vec::new(),
            blocked: false,
            no_mouse: false,
            mousekeys: false,
            forced: Vec::new(),
        }
    }

    fn all(&self) -> Vec<WinInput> {
        self.calls.iter().flatten().copied().collect()
    }
}

/// What Windows does with a normalized `VIRTUALDESK` coordinate.
fn denormalize(n: i32, origin: i32, size: u32) -> i32 {
    origin + ((i64::from(n) * i64::from(size)) >> 16) as i32
}

impl InputSink for FakeSink {
    fn send(&mut self, inputs: &[WinInput]) -> Result<(), EmulateError> {
        if self.blocked {
            return Err(EmulateError::Blocked {
                sent: 0,
                expected: inputs.len(),
            });
        }
        for i in inputs {
            if let WinInput::Mouse { dx, dy, flags, .. } = *i
                && flags & MOUSEEVENTF_ABSOLUTE != 0
            {
                let r = self.desktop.virtual_rect;
                self.cursor = Some((
                    denormalize(dx, r.x, r.width),
                    denormalize(dy, r.y, r.height),
                ));
            }
        }
        self.calls.push(inputs.to_vec());
        Ok(())
    }

    fn desktop(&self) -> Result<Desktop, EmulateError> {
        Ok(self.desktop.clone())
    }

    fn cursor_pos(&self) -> Option<(i32, i32)> {
        self.cursor
    }

    fn cursor_hidden(&self) -> bool {
        self.no_mouse && !self.mousekeys
    }

    fn force_cursor(&mut self, on: bool) -> Result<(), EmulateError> {
        self.mousekeys = on;
        self.forced.push(on);
        Ok(())
    }
}

fn single(w: u32, h: u32) -> Desktop {
    let r = Rect::new(0, 0, w, h);
    Desktop {
        virtual_rect: r,
        monitors: vec![r],
    }
}

/// Primary monitor 1920×1080 and one to the right of 1440×900 aligned at the top: there is
/// a gap at the bottom right that belongs to no screen.
fn two() -> Desktop {
    Desktop {
        virtual_rect: Rect::new(0, 0, 3360, 1080),
        monitors: vec![Rect::new(0, 0, 1920, 1080), Rect::new(1920, 0, 1440, 900)],
    }
}

fn emu(desktop: Desktop, cursor: (i32, i32)) -> Emulator<FakeSink> {
    Emulator::new(FakeSink::new(desktop, cursor)).unwrap()
}

fn key(id: u16, state: KeyState) -> InputEvent {
    InputEvent::Key {
        usage: HidUsage::key(id),
        state,
    }
}

fn kb(scan: u16, flags: u32) -> WinInput {
    WinInput::key(scan, flags)
}

#[test]
fn keys_by_scancode_with_extended_and_keyup() {
    let mut e = emu(single(1920, 1080), (0, 0));
    e.emulate(&key(0x04, KeyState::Pressed)).unwrap(); // A
    e.emulate(&key(0x04, KeyState::Released)).unwrap();
    e.emulate(&key(0x52, KeyState::Pressed)).unwrap(); // Up arrow (E0 48)
    e.emulate(&key(0xE6, KeyState::Released)).unwrap(); // AltGr (E0 38)
    assert_eq!(
        e.sink().all(),
        vec![
            kb(0x1E, KEYEVENTF_SCANCODE),
            kb(0x1E, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
            kb(0x48, KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY),
            kb(
                0x38,
                KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP
            ),
        ]
    );
}

#[test]
fn everything_injected_carries_the_mark() {
    let mut e = emu(single(800, 600), (0, 0));
    e.emulate(&key(0x04, KeyState::Pressed)).unwrap();
    e.emulate(&InputEvent::Button {
        button: Button::Left,
        state: KeyState::Pressed,
    })
    .unwrap();
    e.emulate(&InputEvent::MotionAbsolute { x: 10.0, y: 10.0 })
        .unwrap();
    e.emulate(&InputEvent::Scroll(Scroll::Wheel {
        dx_v120: 0,
        dy_v120: 120,
    }))
    .unwrap();
    let all = e.sink().all();
    assert_eq!(all.len(), 4);
    for i in all {
        let extra = match i {
            WinInput::Keyboard { extra_info, .. } | WinInput::Mouse { extra_info, .. } => {
                extra_info
            }
        };
        assert_eq!(extra, TANDEM_EXTRA_INFO);
    }
}

#[test]
fn pause_goes_by_virtual_code() {
    assert_eq!(
        key_input(HidUsage::key(0x48), KeyState::Released).unwrap(),
        WinInput::Keyboard {
            vk: VK_PAUSE,
            scan: 0,
            flags: KEYEVENTF_KEYUP,
            extra_info: TANDEM_EXTRA_INFO,
        }
    );
    // Num Lock does go by scancode (E0 45): it is not confused with Pause.
    assert_eq!(
        key_input(HidUsage::key(0x53), KeyState::Pressed).unwrap(),
        kb(0x45, KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY)
    );
}

#[test]
fn media_from_the_consumer_page() {
    // Play/Pause (0x0C 0xCD) → E0 22; Mute by alias (0x0C 0xE2) → E0 20.
    assert_eq!(
        key_input(HidUsage::consumer(0xCD), KeyState::Pressed).unwrap(),
        kb(0x22, KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY)
    );
    assert_eq!(
        key_input(HidUsage::consumer(0xE2), KeyState::Pressed).unwrap(),
        kb(0x20, KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY)
    );
}

#[test]
fn a_key_without_scancode_is_an_error_and_is_not_left_pressed() {
    let mut e = emu(single(800, 600), (0, 0));
    let usage = HidUsage::new(0xFF00, 1);
    let ev = InputEvent::Key {
        usage,
        state: KeyState::Pressed,
    };
    assert_eq!(e.emulate(&ev), Err(EmulateError::UnmappedKey(usage)));
    assert!(e.sink().calls.is_empty());
    assert!(e.pressed().is_empty());
}

#[test]
fn all_the_keys_of_the_table_are_injected() {
    for (usage, sc) in keymap::win_usages() {
        let input = key_input(usage, KeyState::Pressed).unwrap();
        if usage == HidUsage::key(0x48) {
            continue;
        }
        let WinInput::Keyboard { scan, flags, .. } = input else {
            panic!("{usage:?} is not a keyboard one");
        };
        assert_eq!(scan, u16::from(sc.code()));
        assert_eq!(flags & KEYEVENTF_EXTENDEDKEY != 0, sc.is_extended());
    }
}

#[test]
fn buttons() {
    let cases = [
        (Button::Left, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, 0),
        (Button::Right, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, 0),
        (
            Button::Middle,
            MOUSEEVENTF_MIDDLEDOWN,
            MOUSEEVENTF_MIDDLEUP,
            0,
        ),
        (Button::Back, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, XBUTTON1),
        (
            Button::Forward,
            MOUSEEVENTF_XDOWN,
            MOUSEEVENTF_XUP,
            XBUTTON2,
        ),
        (
            Button::Other(0),
            MOUSEEVENTF_LEFTDOWN,
            MOUSEEVENTF_LEFTUP,
            0,
        ),
        (
            Button::Other(4),
            MOUSEEVENTF_XDOWN,
            MOUSEEVENTF_XUP,
            XBUTTON2,
        ),
    ];
    for (b, down, up, data) in cases {
        assert_eq!(
            button_input(b, KeyState::Pressed).unwrap(),
            WinInput::mouse(0, 0, data, down)
        );
        assert_eq!(
            button_input(b, KeyState::Released).unwrap(),
            WinInput::mouse(0, 0, data, up)
        );
    }
    assert_eq!(
        button_input(Button::Other(5), KeyState::Pressed),
        Err(EmulateError::UnsupportedButton(5))
    );
}

const MOVE_ABS: u32 = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;

#[test]
fn absolute_movement_at_the_corners() {
    let mut e = emu(single(1920, 1080), (500, 500));
    e.emulate(&InputEvent::MotionAbsolute { x: 0.0, y: 0.0 })
        .unwrap();
    e.emulate(&InputEvent::MotionAbsolute {
        x: 1919.9,
        y: 1079.9,
    })
    .unwrap();
    assert_eq!(
        e.sink().all(),
        vec![
            WinInput::mouse(0, 0, 0, MOVE_ABS),
            WinInput::mouse(65502, 65476, 0, MOVE_ABS),
        ]
    );
    assert_eq!(e.sink().cursor, Some((1919, 1079)));
}

#[test]
fn fractions_accumulate() {
    let mut e = emu(single(1920, 1080), (100, 100));
    for _ in 0..400 {
        e.emulate(&InputEvent::MotionRelative {
            dx: 0.25,
            dy: -0.125,
        })
        .unwrap();
    }
    // 400 × 0.25 = 100 px and 400 × -0.125 = -50 px exactly, from the pixel's center.
    assert_eq!(e.position(), Point::new(200.5, 50.5));
    assert_eq!(e.sink().cursor, Some((200, 50)));
    // Something is only sent when it changes pixel (100 times in x, 50 in y), not on every event.
    assert!(e.sink().calls.len() <= 151, "{}", e.sink().calls.len());
}

#[test]
fn if_the_local_mouse_moved_the_cursor_it_starts_from_there() {
    let mut e = emu(single(1920, 1080), (100, 100));
    e.emulate(&InputEvent::MotionRelative { dx: 1.0, dy: 0.0 })
        .unwrap();
    assert_eq!(e.sink().cursor, Some((101, 100)));
    e.sink_mut().cursor = Some((700, 300));
    e.emulate(&InputEvent::MotionRelative { dx: 2.0, dy: 1.0 })
        .unwrap();
    assert_eq!(e.sink().cursor, Some((702, 301)));
}

#[test]
fn the_cursor_does_not_fall_in_the_desktop_gap() {
    let mut e = emu(two(), (100, 100));
    e.emulate(&InputEvent::MotionAbsolute {
        x: 2500.0,
        y: 1050.0,
    })
    .unwrap();
    assert_eq!(e.sink().cursor, Some((2500, 899)));
    // And it can keep moving from there toward the primary monitor.
    e.emulate(&InputEvent::MotionRelative {
        dx: -1000.0,
        dy: 100.0,
    })
    .unwrap();
    assert_eq!(e.sink().cursor, Some((1500, 999)));
}

#[test]
fn negative_origin_monitor_on_the_left() {
    let d = Desktop {
        virtual_rect: Rect::new(-2560, -360, 4480, 1800),
        monitors: vec![
            Rect::new(0, 0, 1920, 1080),
            Rect::new(-2560, -360, 2560, 1440),
        ],
    };
    let mut e = emu(d, (0, 0));
    e.emulate(&InputEvent::MotionAbsolute {
        x: -2560.0,
        y: -360.0,
    })
    .unwrap();
    assert_eq!(e.sink().cursor, Some((-2560, -360)));
    e.emulate(&InputEvent::MotionAbsolute { x: -1.0, y: 500.0 })
        .unwrap();
    assert_eq!(e.sink().cursor, Some((-1, 500)));
}

#[test]
fn non_finite_movement_is_ignored() {
    let mut e = emu(single(800, 600), (10, 10));
    e.emulate(&InputEvent::MotionRelative {
        dx: f64::NAN,
        dy: 1.0,
    })
    .unwrap();
    e.emulate(&InputEvent::MotionAbsolute {
        x: f64::INFINITY,
        y: 0.0,
    })
    .unwrap();
    assert!(e.sink().calls.is_empty());
    assert_eq!(e.position(), Point::new(10.5, 10.5));
}

#[test]
fn v120_wheel_vertical_and_horizontal() {
    let mut e = emu(single(800, 600), (0, 0));
    e.emulate(&InputEvent::Scroll(Scroll::Wheel {
        dx_v120: 30,
        dy_v120: -40,
    }))
    .unwrap();
    e.emulate(&InputEvent::Scroll(Scroll::Wheel {
        dx_v120: 0,
        dy_v120: 0,
    }))
    .unwrap();
    assert_eq!(
        e.sink().calls,
        vec![vec![
            WinInput::mouse(0, 0, (-40i32) as u32, MOUSEEVENTF_WHEEL),
            WinInput::mouse(0, 0, 30, MOUSEEVENTF_HWHEEL),
        ]]
    );
}

fn wheel_total(e: &Emulator<FakeSink>, flag: u32) -> i64 {
    e.sink()
        .all()
        .into_iter()
        .filter_map(|i| match i {
            WinInput::Mouse { data, flags, .. } if flags == flag => Some(i64::from(data as i32)),
            _ => None,
        })
        .sum()
}

#[test]
fn trackpad_pixels_accumulate_notch_fractions() {
    let mut e = emu(single(800, 600), (0, 0));
    for _ in 0..100 {
        e.emulate(&InputEvent::Scroll(Scroll::Pixels { dx: -0.5, dy: 1.0 }))
            .unwrap();
    }
    // 100 px = one notch (120); -50 px = -60.
    assert_eq!(wheel_total(&e, MOUSEEVENTF_WHEEL), 120);
    assert_eq!(wheel_total(&e, MOUSEEVENTF_HWHEEL), -60);
}

proptest! {
    /// Any pixel of the virtual desktop comes back exact after normalizing and
    /// denormalizing like Windows.
    #[test]
    fn normalizing_lands_on_the_same_pixel(
        ox in -8000i32..8000,
        oy in -8000i32..8000,
        w in 1u32..20_000,
        h in 1u32..20_000,
        fx in 0.0f64..1.0,
        fy in 0.0f64..1.0,
    ) {
        let r = Rect::new(ox, oy, w, h);
        let d = Desktop { virtual_rect: r, monitors: vec![r] };
        let px = ox + (fx * f64::from(w)) as i32;
        let py = oy + (fy * f64::from(h)) as i32;
        let px = px.min(ox + w as i32 - 1);
        let py = py.min(oy + h as i32 - 1);
        let (nx, ny) = d.normalize(px, py);
        prop_assert!((0..65536).contains(&nx) && (0..65536).contains(&ny));
        prop_assert_eq!(denormalize(nx, ox, w), px);
        prop_assert_eq!(denormalize(ny, oy, h), py);
    }

    /// Trackpad pixels are not lost: what is emitted differs from the total by less than
    /// one v120 unit.
    #[test]
    fn pixel_scroll_loses_nothing(deltas in prop::collection::vec(-50.0f64..50.0, 1..200)) {
        let mut e = emu(single(800, 600), (0, 0));
        for dy in &deltas {
            e.emulate(&InputEvent::Scroll(Scroll::Pixels { dx: 0.0, dy: *dy })).unwrap();
        }
        let expected: f64 = deltas.iter().sum::<f64>() * WHEEL_DELTA / PIXELS_PER_NOTCH;
        let got = wheel_total(&e, MOUSEEVENTF_WHEEL) as f64;
        prop_assert!((expected - got).abs() < 1.0 + 1e-6, "{expected} vs {got}");
    }

    /// With any sequence of movements, the cursor ends up inside a monitor.
    #[test]
    fn the_cursor_always_ends_up_on_a_monitor(
        moves in prop::collection::vec((-3000.0f64..3000.0, -3000.0f64..3000.0), 1..50),
    ) {
        let mut e = emu(two(), (100, 100));
        let monitors = two().monitors;
        for (dx, dy) in moves {
            e.emulate(&InputEvent::MotionRelative { dx, dy }).unwrap();
            let (x, y) = e.sink().cursor.unwrap();
            let p = Point::new(f64::from(x), f64::from(y));
            prop_assert!(monitors.iter().any(|m| m.contains(p)), "{p:?}");
        }
    }
}

#[test]
fn synchronizes_modifiers_releasing_first() {
    let mut e = emu(single(800, 600), (0, 0));
    e.emulate(&InputEvent::Key {
        usage: ModifierKey::ControlLeft.usage(),
        state: KeyState::Pressed,
    })
    .unwrap();
    let target: Modifiers = [ModifierKey::ShiftLeft, ModifierKey::AltRight]
        .into_iter()
        .collect();
    e.emulate(&InputEvent::Modifiers(target)).unwrap();
    assert_eq!(
        e.sink().calls.last().unwrap(),
        &vec![
            kb(0x1D, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
            kb(0x2A, KEYEVENTF_SCANCODE),
            kb(0x38, KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY),
        ]
    );
    assert_eq!(e.pressed().modifiers(), target);
    // Asking for the same state sends nothing.
    let before = e.sink().calls.len();
    e.emulate(&InputEvent::Modifiers(target)).unwrap();
    assert_eq!(e.sink().calls.len(), before);
}

#[test]
fn release_all_releases_everything_in_one_call() {
    let mut e = emu(single(800, 600), (0, 0));
    e.emulate(&InputEvent::Key {
        usage: ModifierKey::ControlLeft.usage(),
        state: KeyState::Pressed,
    })
    .unwrap();
    e.emulate(&key(0x06, KeyState::Pressed)).unwrap(); // C
    e.emulate(&InputEvent::Button {
        button: Button::Back,
        state: KeyState::Pressed,
    })
    .unwrap();
    e.release_all().unwrap();
    assert_eq!(
        e.sink().calls.last().unwrap(),
        &vec![
            WinInput::mouse(0, 0, XBUTTON1, MOUSEEVENTF_XUP),
            kb(0x2E, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
            kb(0x1D, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
        ]
    );
    assert!(e.pressed().is_empty());
    // With nothing pressed SendInput is not called.
    let before = e.sink().calls.len();
    e.release_all().unwrap();
    assert_eq!(e.sink().calls.len(), before);
}

#[test]
fn if_sendinput_fails_the_state_does_not_change() {
    let mut e = emu(single(800, 600), (0, 0));
    e.emulate(&key(0x04, KeyState::Pressed)).unwrap();
    e.sink_mut().blocked = true;
    assert!(matches!(
        e.emulate(&key(0x05, KeyState::Pressed)),
        Err(EmulateError::Blocked { .. })
    ));
    assert!(e.release_all().is_err());
    assert!(e.pressed().is_key_pressed(HidUsage::key(0x04)));
    assert!(!e.pressed().is_key_pressed(HidUsage::key(0x05)));
    e.sink_mut().blocked = false;
    e.release_all().unwrap();
    assert!(e.pressed().is_empty());
}

#[test]
fn an_empty_desktop_is_an_error() {
    let d = Desktop {
        virtual_rect: Rect::new(0, 0, 0, 0),
        monitors: vec![],
    };
    assert!(matches!(
        Emulator::new(FakeSink::new(d, (0, 0))),
        Err(EmulateError::NoDesktop)
    ));
}

#[test]
fn without_a_cursor_it_starts_at_the_center() {
    let mut sink = FakeSink::new(single(800, 600), (0, 0));
    sink.cursor = None;
    let e = Emulator::new(sink).unwrap();
    assert_eq!(e.position(), Point::new(400.0, 300.0));
}

#[test]
fn when_the_screens_change_the_position_adjusts() {
    let mut e = emu(two(), (100, 100));
    e.emulate(&InputEvent::MotionAbsolute {
        x: 3000.0,
        y: 500.0,
    })
    .unwrap();
    // The second monitor is disconnected.
    e.sink_mut().desktop = single(1920, 1080);
    e.refresh_desktop().unwrap();
    assert_eq!(e.position(), Point::new(1919.0, 500.0));
    // After rereading, the next movement is sent even if it lands on the same pixel.
    e.sink_mut().cursor = Some((1919, 500));
    e.emulate(&InputEvent::MotionAbsolute {
        x: 1919.0,
        y: 500.0,
    })
    .unwrap();
    assert_eq!(e.sink().cursor, Some((1919, 500)));
}

/// The bug: with the keyboard and mouse hub on the Mac, Windows had no mouse and did not
/// draw the cursor, so on the PC it moved invisible.
#[test]
fn without_a_mouse_the_cursor_is_forced_while_the_focus_is_here() {
    let mut e = emu(single(1920, 1080), (10, 10));
    e.sink_mut().no_mouse = true;
    let t0 = Instant::now();
    e.set_focus_at(true, t0).unwrap();
    assert!(e.cursor_forced());
    assert!(!e.sink().cursor_hidden(), "Windows draws it now");
    // Moving does not touch it again.
    e.emulate(&InputEvent::MotionRelative { dx: 5.0, dy: 0.0 })
        .unwrap();
    assert_eq!(e.sink().forced, vec![true]);
    // On leaving, the user's setting comes back.
    e.set_focus_at(false, t0).unwrap();
    assert!(!e.cursor_forced());
    assert_eq!(e.sink().forced, vec![true, false]);
    e.set_focus_at(false, t0).unwrap();
    assert_eq!(e.sink().forced, vec![true, false], "only once");
}

#[test]
fn with_a_mouse_nothing_is_touched() {
    let mut e = emu(single(1920, 1080), (10, 10));
    e.set_focus_at(true, Instant::now()).unwrap();
    e.emulate(&InputEvent::MotionAbsolute { x: 50.0, y: 50.0 })
        .unwrap();
    e.set_focus_at(false, Instant::now()).unwrap();
    assert!(e.sink().forced.is_empty());
}

#[test]
fn a_mouse_unplugged_while_the_focus_is_here_is_noticed_moving() {
    let mut e = emu(single(1920, 1080), (10, 10));
    let t0 = Instant::now();
    e.set_focus_at(true, t0).unwrap();
    e.sink_mut().no_mouse = true;
    // Checked at most once a second, never on every event.
    e.check_cursor(t0 + Duration::from_millis(500)).unwrap();
    assert!(!e.cursor_forced());
    e.check_cursor(t0 + CURSOR_CHECK_EVERY).unwrap();
    assert!(e.cursor_forced());
}

#[test]
fn without_the_focus_the_cursor_is_never_forced() {
    let mut e = emu(single(1920, 1080), (10, 10));
    e.sink_mut().no_mouse = true;
    e.emulate(&InputEvent::MotionAbsolute { x: 50.0, y: 50.0 })
        .unwrap();
    assert!(!e.cursor_forced());
    assert!(e.sink().forced.is_empty());
}

#[test]
fn mousekeys_leaves_the_keypad_as_the_user_types_with_it() {
    // Num Lock on (digits): MouseKeys only acts with it off.
    assert_eq!(
        forced_mousekeys_flags(true),
        MKF_MOUSEKEYSON | MKF_AVAILABLE
    );
    // Num Lock off (arrows): MouseKeys only acts with it on.
    assert_eq!(
        forced_mousekeys_flags(false),
        MKF_MOUSEKEYSON | MKF_AVAILABLE | MKF_REPLACENUMBERS
    );
}
