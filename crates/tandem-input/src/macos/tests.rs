//! Injection and capture on the same Mac (T-241): what the emulator posts must not come
//! back through the tap as local input, and real input must still be captured.

use proptest::prelude::*;

use super::INJECTED_MARKER;
use super::capture::{RawEvent, TapAction, Translator};
use super::emulate::{MacEmulator, MacEvent, MouseKind, RecordingSink, ScrollUnit};
use crate::event::{Button, HidUsage, InputEvent, KeyState, Scroll};
use crate::geometry::Point;

/// What the tap would read from a posted event (`sys::read_event`, without CoreGraphics).
fn as_tapped(event: &MacEvent) -> RawEvent {
    match *event {
        MacEvent::Key { keycode, down, .. } if down => RawEvent::KeyDown { keycode },
        MacEvent::Key { keycode, .. } => RawEvent::KeyUp { keycode },
        MacEvent::FlagsChanged { keycode, flags } => RawEvent::FlagsChanged { keycode, flags },
        MacEvent::Mouse {
            kind,
            pos,
            button,
            delta,
            ..
        } => match kind {
            MouseKind::LeftDown | MouseKind::RightDown | MouseKind::OtherDown => {
                RawEvent::MouseDown {
                    button: i64::from(button),
                    location: pos,
                }
            }
            MouseKind::LeftUp | MouseKind::RightUp | MouseKind::OtherUp => RawEvent::MouseUp {
                button: i64::from(button),
                location: pos,
            },
            MouseKind::Moved
            | MouseKind::LeftDragged
            | MouseKind::RightDragged
            | MouseKind::OtherDragged => RawEvent::Moved {
                dx: delta.0,
                dy: delta.1,
                location: pos,
            },
        },
        MacEvent::Scroll {
            unit,
            wheel1,
            wheel2,
            ..
        } => {
            let pixels = unit == ScrollUnit::Pixel;
            let (lines, points) = if pixels {
                ((0.0, 0.0), (f64::from(wheel1), f64::from(wheel2)))
            } else {
                ((f64::from(wheel1), f64::from(wheel2)), (0.0, 0.0))
            };
            RawEvent::Scroll {
                continuous: pixels,
                lines_axis1: lines.0,
                lines_axis2: lines.1,
                points_axis1: points.0,
                points_axis2: points.1,
            }
        }
        MacEvent::SystemKey { data1, .. } => RawEvent::SystemDefined {
            subtype: super::capture::translate::SUBTYPE_AUX_CONTROL_BUTTONS,
            data1,
        },
    }
}

/// Injects `events` and feeds what was posted, with its source user data replaced by
/// `tag` when given, to a tap on the same Mac. Returns the tap's decisions and what it
/// forwarded.
fn loopback(
    events: &[InputEvent],
    tag: Option<i64>,
    grabbed: bool,
) -> (Vec<(MacEvent, i64)>, Vec<TapAction>, Vec<InputEvent>) {
    let mut emulator = MacEmulator::new(RecordingSink::new()).unwrap();
    for e in events {
        // An unmapped key is the emulator's business, not the loop's.
        let _ = emulator.emit(e);
    }
    let posted = emulator.sink_mut().take_tagged();
    let mut tap = Translator::new();
    let mut forwarded = Vec::new();
    let actions = posted
        .iter()
        .map(|(event, user_data)| {
            tap.tap(
                &as_tapped(event),
                tag.unwrap_or(*user_data),
                grabbed,
                false,
                &mut |e| forwarded.push(e),
            )
        })
        .collect();
    (posted, actions, forwarded)
}

fn sample() -> Vec<InputEvent> {
    vec![
        InputEvent::MotionRelative { dx: 25.0, dy: -4.0 },
        InputEvent::MotionAbsolute { x: 10.0, y: 10.0 },
        InputEvent::Key {
            usage: HidUsage::key(0xE1), // left Shift
            state: KeyState::Pressed,
        },
        InputEvent::Key {
            usage: HidUsage::key(0x04), // A
            state: KeyState::Pressed,
        },
        InputEvent::Key {
            usage: HidUsage::key(0x04),
            state: KeyState::Released,
        },
        InputEvent::Key {
            usage: HidUsage::key(0xE1),
            state: KeyState::Released,
        },
        InputEvent::Button {
            button: Button::Left,
            state: KeyState::Pressed,
        },
        InputEvent::MotionRelative { dx: 3.0, dy: 3.0 },
        InputEvent::Button {
            button: Button::Left,
            state: KeyState::Released,
        },
        InputEvent::Scroll(Scroll::Wheel {
            dx_v120: 0,
            dy_v120: 240,
        }),
        InputEvent::Scroll(Scroll::Pixels { dx: 12.0, dy: 0.0 }),
        InputEvent::Key {
            usage: HidUsage::consumer(0xCD), // play/pause
            state: KeyState::Pressed,
        },
    ]
}

#[test]
fn every_injected_event_carries_the_marker() {
    let (posted, _, _) = loopback(&sample(), None, false);
    assert!(posted.len() >= sample().len(), "{posted:?}");
    for (event, user_data) in &posted {
        assert_eq!(*user_data, INJECTED_MARKER, "{event:?}");
    }
}

#[test]
fn injected_events_are_not_captured_again() {
    for grabbed in [false, true] {
        let (posted, actions, forwarded) = loopback(&sample(), None, grabbed);
        assert!(forwarded.is_empty(), "grabbed {grabbed}: {forwarded:?}");
        // Neither swallowed: they are meant for this Mac.
        assert_eq!(actions, vec![TapAction::Pass; posted.len()], "{grabbed}");
    }
}

#[test]
fn the_same_events_from_a_real_device_are_captured() {
    // The control: without the marker, the very same events are local input.
    let (_, _, forwarded) = loopback(&sample(), Some(0), false);
    let has = |f: fn(&InputEvent) -> bool| forwarded.iter().any(f);
    assert!(has(|e| matches!(e, InputEvent::MotionRelative { .. })));
    assert!(has(|e| matches!(e, InputEvent::Key { .. })));
    assert!(has(|e| matches!(e, InputEvent::Button { .. })));
    assert!(has(|e| matches!(e, InputEvent::Scroll(_))));

    // Another app's tag is not ours either.
    let (_, _, forwarded) = loopback(&sample(), Some(INJECTED_MARKER + 1), false);
    assert!(!forwarded.is_empty());
}

#[test]
fn injected_motion_does_not_reach_the_edge_logic() {
    // The echo that started T-241: motion injected on the receiving Mac pushed against an
    // edge would come back as `MotionRelative` and could cross back to the other machine.
    let push = vec![InputEvent::MotionRelative { dx: -6.0, dy: 0.0 }; 50];
    let (_, _, forwarded) = loopback(&push, None, false);
    assert!(forwarded.is_empty(), "{forwarded:?}");
}

#[test]
fn an_injected_press_does_not_count_as_local() {
    // The tap passes the release of what the Mac saw pressed locally even when grabbed;
    // an injected press must not open that door.
    let mut tap = Translator::new();
    let mut out = Vec::new();
    let down = RawEvent::KeyDown { keycode: 0x00 };
    let up = RawEvent::KeyUp { keycode: 0x00 };
    assert_eq!(
        tap.tap(&down, INJECTED_MARKER, false, false, &mut |e| out.push(e)),
        TapAction::Pass
    );
    assert_eq!(
        tap.tap(&up, 0, true, false, &mut |e| out.push(e)),
        TapAction::Swallow
    );
    assert_eq!(
        out,
        vec![InputEvent::Key {
            usage: HidUsage::key(0x04),
            state: KeyState::Released,
        }]
    );
}

#[test]
fn the_tap_notices_are_handled_whatever_their_user_data() {
    use super::capture::TapDisabledBy;
    let mut tap = Translator::new();
    let action = tap.tap(
        &RawEvent::TapDisabled(TapDisabledBy::Timeout),
        INJECTED_MARKER,
        true,
        false,
        &mut |_| {},
    );
    assert_eq!(action, TapAction::Reenable);
    assert_eq!(tap.tap_disabled().timeout, 1);
}

fn any_event() -> impl Strategy<Value = InputEvent> {
    let state = prop_oneof![Just(KeyState::Pressed), Just(KeyState::Released)];
    let usage = prop_oneof![
        (0x04u16..=0x65).prop_map(HidUsage::key),
        (0xE0u16..=0xE7).prop_map(HidUsage::key),
        Just(HidUsage::consumer(0xCD)),
    ];
    let button = prop_oneof![
        Just(Button::Left),
        Just(Button::Right),
        Just(Button::Middle),
        Just(Button::Back),
        (5u8..8).prop_map(Button::Other),
    ];
    prop_oneof![
        (usage, state.clone()).prop_map(|(usage, state)| InputEvent::Key { usage, state }),
        (button, state).prop_map(|(button, state)| InputEvent::Button { button, state }),
        (-50.0f64..50.0, -50.0f64..50.0).prop_map(|(dx, dy)| InputEvent::MotionRelative { dx, dy }),
        (0.0f64..1920.0, 0.0f64..1080.0).prop_map(|(x, y)| InputEvent::MotionAbsolute { x, y }),
        (-480i32..480, -480i32..480)
            .prop_map(|(dx_v120, dy_v120)| InputEvent::Scroll(Scroll::Wheel { dx_v120, dy_v120 })),
        (-40.0f64..40.0, -40.0f64..40.0)
            .prop_map(|(dx, dy)| InputEvent::Scroll(Scroll::Pixels { dx, dy })),
    ]
}

proptest! {
    /// Whatever Tandem injects, grabbed or not, the tap on the same Mac forwards nothing and
    /// swallows nothing.
    #[test]
    fn nothing_injected_echoes(
        events in prop::collection::vec(any_event(), 0..48),
        grabbed in any::<bool>(),
    ) {
        let (posted, actions, forwarded) = loopback(&events, None, grabbed);
        prop_assert!(forwarded.is_empty(), "{:?}", forwarded);
        prop_assert!(actions.iter().all(|a| *a == TapAction::Pass));
        prop_assert!(posted.iter().all(|(_, d)| *d == INJECTED_MARKER));
    }
}

#[test]
fn as_tapped_keeps_the_position_of_motion() {
    let ev = MacEvent::Mouse {
        kind: MouseKind::Moved,
        pos: Point::new(5.0, 6.0),
        button: 0,
        click_state: 0,
        delta: (1.0, 2.0),
        flags: 0,
    };
    assert_eq!(
        as_tapped(&ev),
        RawEvent::Moved {
            dx: 1.0,
            dy: 2.0,
            location: Point::new(5.0, 6.0)
        }
    );
}
