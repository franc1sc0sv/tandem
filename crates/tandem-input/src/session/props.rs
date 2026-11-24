//! Server and client together, without a network, with datagrams that get lost.

use proptest::prelude::*;
use tandem_core::Machine;

use super::testutil::layout_pc_mac;
use super::{ClientSession, ServerOutput, ServerSession};
use crate::event::{Button, HidUsage, InputEvent, KeyState, Pressed, Scroll};
use crate::geometry::Point;

fn any_event() -> impl Strategy<Value = InputEvent> {
    let state = prop_oneof![Just(KeyState::Pressed), Just(KeyState::Released)];
    let usage = prop_oneof![0x04u16..0x08, 0xE0u16..=0xE3].prop_map(HidUsage::key);
    let button = prop_oneof![Just(Button::Left), Just(Button::Right)];
    prop_oneof![
        4 => (-40.0f64..60.0, -30.0f64..30.0)
            .prop_map(|(dx, dy)| InputEvent::MotionRelative { dx, dy }),
        2 => (usage, state.clone()).prop_map(|(usage, state)| InputEvent::Key { usage, state }),
        1 => (button, state).prop_map(|(button, state)| InputEvent::Button { button, state }),
        1 => (-240i32..240).prop_map(|dy_v120| InputEvent::Scroll(Scroll::Wheel {
            dx_v120: 0,
            dy_v120,
        })),
    ]
}

/// How many times each key or button is pressed according to what was injected.
fn injected(system: &mut Pressed, events: &[InputEvent]) -> Result<(), TestCaseError> {
    for e in events {
        let released = matches!(
            e,
            InputEvent::Key {
                state: KeyState::Released,
                ..
            } | InputEvent::Button {
                state: KeyState::Released,
                ..
            }
        );
        let change = system.record(e);
        if released {
            prop_assert_eq!(
                change,
                crate::event::Change::Changed,
                "release without a press"
            );
        }
    }
    Ok(())
}

proptest! {
    #[test]
    fn the_client_follows_the_server_even_if_datagrams_are_lost(
        steps in prop::collection::vec((any_event(), any::<bool>(), any::<bool>()), 0..120),
    ) {
        let mut server = ServerSession::new(
            Machine::B,
            layout_pc_mac(),
            Point::new(3400.0, 720.0),
            8,
        );
        let mut client = ClientSession::new(layout_pc_mac().screens(Machine::A).to_vec());
        let mut physical = Pressed::new();
        let mut system = Pressed::new();

        let deliver = |outs: Vec<ServerOutput>, drop: bool, client: &mut ClientSession,
                           system: &mut Pressed| -> Result<(), TestCaseError> {
            for o in outs {
                let events = match o {
                    ServerOutput::Stream(m) => client.on_stream(m),
                    ServerOutput::Datagram(d) if !drop => client.on_datagram(d),
                    _ => Vec::new(),
                };
                injected(system, &events)?;
            }
            Ok(())
        };

        for (event, flush, drop) in steps {
            physical.record(&event);
            let mut outs = server.handle(event);
            if flush {
                outs.extend(server.flush().map(ServerOutput::Datagram));
            }
            deliver(outs, drop, &mut client, &mut system)?;

            // Nothing can be pressed on the client if the user does not have it
            // really pressed.
            for id in (0x04u16..0x08).chain(0xE0..=0xE3) {
                let u = HidUsage::key(id);
                prop_assert!(!system.is_key_pressed(u) || physical.is_key_pressed(u));
            }
            for b in [Button::Left, Button::Right] {
                prop_assert!(!system.is_button_pressed(b) || physical.is_button_pressed(b));
            }
            prop_assert_eq!(client.is_active(), server.is_remote());
        }

        // The mouse stops: what is pending goes out and the resynchronization arrives.
        let outs = server.flush().map(ServerOutput::Datagram).into_iter().collect();
        deliver(outs, true, &mut client, &mut system)?;
        let outs = server.resync();
        deliver(outs, true, &mut client, &mut system)?;
        prop_assert_eq!(client.cursor(), server.remote_cursor());
    }
}

/// The client with two screens of different scales side by side, to the
/// right of the server.
fn layout_mixed_scales() -> crate::geometry::Layout {
    use crate::geometry::{Arrangement, Layout, LayoutOptions, Rect, Screen, Side};
    let a = vec![
        Screen::new("a1".into(), Rect::new(3440, 0, 1920, 1080), 1.0),
        Screen::new("a2".into(), Rect::new(5360, 0, 3840, 2160), 2.0),
    ];
    let b = vec![Screen::new("b1".into(), Rect::new(0, 0, 3440, 1440), 1.25)];
    Layout::compute(
        &Arrangement {
            a,
            b,
            a_side: Side::Right,
            align: None,
        },
        LayoutOptions { corner_margin: 0.0 },
    )
    .unwrap()
}

/// Crosses the server's cursor to the other machine and delivers the `Enter` to the client.
fn cross_mixed(server: &mut ServerSession, client: &mut ClientSession) {
    server.handle(InputEvent::MotionAbsolute {
        x: 3439.0,
        y: 700.0,
    });
    for o in server.handle(InputEvent::MotionRelative { dx: 5.0, dy: 0.0 }) {
        if let ServerOutput::Stream(m) = o {
            client.on_stream(m);
        }
    }
}

proptest! {
    /// Without losses, after each datagram the client's cursor is exactly
    /// where the server believes, even if the accumulated movement passes through screens of
    /// different scales (T-093: per-screen scale).
    #[test]
    fn without_losses_the_client_matches_the_server_across_scales(
        moves in prop::collection::vec((-300.0f64..300.0, -200.0f64..200.0, any::<bool>()), 1..60),
    ) {
        let layout = layout_mixed_scales();
        // Without periodic resynchronization: only the deltas count.
        let mut server = ServerSession::new(Machine::B, layout.clone(), Point::new(3400.0, 700.0), u32::MAX);
        let mut client = ClientSession::new(layout.screens(Machine::A).to_vec());
        cross_mixed(&mut server, &mut client);
        prop_assert!(server.is_remote());
        for (dx, dy, flush) in moves {
            for o in server.handle(InputEvent::MotionRelative { dx, dy }) {
                if let ServerOutput::Stream(m) = o {
                    client.on_stream(m);
                }
            }
            if flush && let Some(d) = server.flush() {
                client.on_datagram(d);
                if let (Some(c), Some(s)) = (client.cursor(), server.remote_cursor()) {
                    prop_assert!((c.x - s.x).abs() < 1e-6 && (c.y - s.y).abs() < 1e-6, "{c:?} vs {s:?}");
                }
            }
        }
    }

    /// The cursor keeps its physical speed on crossing: a native delta of the server is
    /// converted on the client with the ratio of the scales of the two screens.
    #[test]
    fn physical_speed_is_kept_between_scales(
        local_scale in prop::sample::select(vec![1.0, 1.25, 1.5, 2.0]),
        remote_scale in prop::sample::select(vec![1.0, 1.25, 1.5, 2.0]),
        dx in 1.0f64..40.0,
        dy in -40.0f64..40.0,
    ) {
        use crate::geometry::{Arrangement, Layout, LayoutOptions, Rect, Screen, Side};
        let layout = Layout::compute(
            &Arrangement {
                a: vec![Screen::new("a".into(), Rect::new(3440, 0, 3840, 2160), remote_scale)],
                b: vec![Screen::new("b".into(), Rect::new(0, 0, 3440, 1440), local_scale)],
                a_side: Side::Right,
                align: None,
            },
            LayoutOptions { corner_margin: 0.0 },
        )
        .unwrap();
        let mut server = ServerSession::new(Machine::B, layout.clone(), Point::new(3400.0, 700.0), 0);
        let mut client = ClientSession::new(layout.screens(Machine::A).to_vec());
        cross_mixed(&mut server, &mut client);
        let before = client.cursor().unwrap();
        server.handle(InputEvent::MotionRelative { dx, dy });
        client.on_datagram(server.flush().unwrap());
        let after = client.cursor().unwrap();
        let k = remote_scale / local_scale;
        prop_assert!((after.x - before.x - dx * k).abs() < 1e-6, "{before:?} → {after:?}");
        prop_assert!((after.y - before.y - dy * k).abs() < 1e-6, "{before:?} → {after:?}");
    }
}

proptest! {
    /// T-093, respecting screens: with the complete session (server and client, without a network),
    /// the cursor the server follows and the one the client injects never end up outside
    /// a screen, whether they go back and forth between the two machines over screens of different scales and
    /// heights.
    #[test]
    fn the_injected_cursor_never_leaves_the_screens(
        moves in prop::collection::vec((-600.0f64..600.0, -500.0f64..500.0), 1..80),
    ) {
        let layout = layout_mixed_scales();
        let mut server = ServerSession::new(Machine::B, layout.clone(), Point::new(3000.0, 700.0), 4);
        let mut client = ClientSession::new(layout.screens(Machine::A).to_vec());
        let on = |m: Machine, p: Point| layout.screens(m).iter().any(|s| s.rect.contains(p));
        for (dx, dy) in moves {
            let mut outs = server.handle(InputEvent::MotionRelative { dx, dy });
            outs.extend(server.flush().map(ServerOutput::Datagram));
            for o in outs {
                let events = match o {
                    ServerOutput::Stream(m) => client.on_stream(m),
                    ServerOutput::Datagram(d) => client.on_datagram(d),
                    _ => Vec::new(),
                };
                for e in events {
                    if let InputEvent::MotionAbsolute { x, y } = e {
                        prop_assert!(on(Machine::A, Point::new(x, y)), "injected outside: ({x}, {y})");
                    }
                }
            }
            match server.remote_cursor() {
                Some(p) => prop_assert!(on(Machine::A, p), "{p:?}"),
                None => prop_assert!(on(Machine::B, server.local_cursor()), "{:?}", server.local_cursor()),
            }
        }
    }
}
