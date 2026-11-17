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

/// El cliente con dos pantallas de escalas distintas una al lado de la otra, a la
/// derecha del servidor.
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

/// Cruza el cursor del servidor a la otra máquina y entrega el `Enter` al cliente.
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
    /// Sin pérdidas, después de cada datagrama el cursor del cliente está exactamente
    /// donde el servidor cree, aunque el movimiento acumulado pase por pantallas de
    /// escalas distintas (T-093: escala por pantalla).
    #[test]
    fn sin_perdidas_el_cliente_coincide_con_el_servidor_entre_escalas(
        moves in prop::collection::vec((-300.0f64..300.0, -200.0f64..200.0, any::<bool>()), 1..60),
    ) {
        let layout = layout_mixed_scales();
        // Sin resincronización periódica: solo cuentan los deltas.
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

    /// El cursor mantiene la velocidad física al cruzar: un delta nativo del servidor se
    /// convierte en el cliente con la razón de las escalas de las dos pantallas.
    #[test]
    fn la_velocidad_fisica_se_conserva_entre_escalas(
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
    /// T-093, respetar pantallas: con la sesión completa (servidor y cliente, sin red),
    /// el cursor que sigue el servidor y el que inyecta el cliente nunca quedan fuera de
    /// una pantalla, vayan y vengan entre las dos máquinas por pantallas de escalas y
    /// alturas distintas.
    #[test]
    fn el_cursor_inyectado_nunca_sale_de_las_pantallas(
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
                        prop_assert!(on(Machine::A, Point::new(x, y)), "inyectado fuera: ({x}, {y})");
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
