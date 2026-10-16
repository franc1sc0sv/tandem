//! Servidor y cliente juntos, sin red, con datagramas que se pierden.

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

/// Cuántas veces cada tecla o botón está presionado según lo inyectado.
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
            prop_assert_eq!(change, crate::event::Change::Changed, "suelta sin presión");
        }
    }
    Ok(())
}

proptest! {
    #[test]
    fn el_cliente_sigue_al_servidor_aunque_se_pierdan_datagramas(
        steps in prop::collection::vec((any_event(), any::<bool>(), any::<bool>()), 0..120),
    ) {
        let mut server = ServerSession::new(
            Machine::Pc,
            layout_pc_mac(),
            Point::new(3400.0, 720.0),
            8,
        );
        let mut client = ClientSession::new(layout_pc_mac().screens(Machine::Mac).to_vec());
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

            // Nada puede estar presionado en el cliente si el usuario no lo tiene
            // presionado de verdad.
            for id in (0x04u16..0x08).chain(0xE0..=0xE3) {
                let u = HidUsage::key(id);
                prop_assert!(!system.is_key_pressed(u) || physical.is_key_pressed(u));
            }
            for b in [Button::Left, Button::Right] {
                prop_assert!(!system.is_button_pressed(b) || physical.is_button_pressed(b));
            }
            prop_assert_eq!(client.is_active(), server.is_remote());
        }

        // El mouse se detiene: lo pendiente sale y la resincronización llega.
        let outs = server.flush().map(ServerOutput::Datagram).into_iter().collect();
        deliver(outs, true, &mut client, &mut system)?;
        let outs = server.resync();
        deliver(outs, true, &mut client, &mut system)?;
        prop_assert_eq!(client.cursor(), server.remote_cursor());
    }
}
