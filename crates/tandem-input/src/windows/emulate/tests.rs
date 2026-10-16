use proptest::prelude::*;

use super::*;
use crate::event::ModifierKey;

/// Doble de `SendInput`: guarda cada llamada y mueve el cursor como lo haría Windows.
struct FakeSink {
    desktop: Desktop,
    cursor: Option<(i32, i32)>,
    calls: Vec<Vec<WinInput>>,
    blocked: bool,
}

impl FakeSink {
    fn new(desktop: Desktop, cursor: (i32, i32)) -> Self {
        Self {
            desktop,
            cursor: Some(cursor),
            calls: Vec::new(),
            blocked: false,
        }
    }

    fn all(&self) -> Vec<WinInput> {
        self.calls.iter().flatten().copied().collect()
    }
}

/// Lo que hace Windows con una coordenada `VIRTUALDESK` normalizada.
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
}

fn single(w: u32, h: u32) -> Desktop {
    let r = Rect::new(0, 0, w, h);
    Desktop {
        virtual_rect: r,
        monitors: vec![r],
    }
}

/// Monitor principal 1920×1080 y uno a la derecha de 1440×900 alineado arriba: queda
/// un hueco abajo a la derecha que no es de ninguna pantalla.
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
fn teclas_por_scancode_con_extendida_y_keyup() {
    let mut e = emu(single(1920, 1080), (0, 0));
    e.emulate(&key(0x04, KeyState::Pressed)).unwrap(); // A
    e.emulate(&key(0x04, KeyState::Released)).unwrap();
    e.emulate(&key(0x52, KeyState::Pressed)).unwrap(); // Flecha arriba (E0 48)
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
fn todo_lo_inyectado_lleva_la_marca() {
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
fn pausa_va_por_codigo_virtual() {
    assert_eq!(
        key_input(HidUsage::key(0x48), KeyState::Released).unwrap(),
        WinInput::Keyboard {
            vk: VK_PAUSE,
            scan: 0,
            flags: KEYEVENTF_KEYUP,
            extra_info: TANDEM_EXTRA_INFO,
        }
    );
    // Bloq Num sí va por scancode (E0 45): no se confunde con Pausa.
    assert_eq!(
        key_input(HidUsage::key(0x53), KeyState::Pressed).unwrap(),
        kb(0x45, KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY)
    );
}

#[test]
fn multimedia_de_la_pagina_de_consumo() {
    // Play/Pausa (0x0C 0xCD) → E0 22; Silencio por alias (0x0C 0xE2) → E0 20.
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
fn tecla_sin_scancode_es_error_y_no_queda_presionada() {
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
fn todas_las_teclas_de_la_tabla_se_inyectan() {
    for (usage, sc) in keymap::win_usages() {
        let input = key_input(usage, KeyState::Pressed).unwrap();
        if usage == HidUsage::key(0x48) {
            continue;
        }
        let WinInput::Keyboard { scan, flags, .. } = input else {
            panic!("{usage:?} no es de teclado");
        };
        assert_eq!(scan, u16::from(sc.code()));
        assert_eq!(flags & KEYEVENTF_EXTENDEDKEY != 0, sc.is_extended());
    }
}

#[test]
fn botones() {
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
fn movimiento_absoluto_en_las_esquinas() {
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
fn las_fracciones_se_acumulan() {
    let mut e = emu(single(1920, 1080), (100, 100));
    for _ in 0..400 {
        e.emulate(&InputEvent::MotionRelative {
            dx: 0.25,
            dy: -0.125,
        })
        .unwrap();
    }
    // 400 × 0.25 = 100 px y 400 × -0.125 = -50 px exactos, desde el centro del píxel.
    assert_eq!(e.position(), Point::new(200.5, 50.5));
    assert_eq!(e.sink().cursor, Some((200, 50)));
    // Solo se manda algo cuando cambia de píxel (100 veces en x, 50 en y), no en cada evento.
    assert!(e.sink().calls.len() <= 151, "{}", e.sink().calls.len());
}

#[test]
fn si_el_mouse_local_movio_el_cursor_se_parte_de_ahi() {
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
fn el_cursor_no_cae_en_el_hueco_del_escritorio() {
    let mut e = emu(two(), (100, 100));
    e.emulate(&InputEvent::MotionAbsolute {
        x: 2500.0,
        y: 1050.0,
    })
    .unwrap();
    assert_eq!(e.sink().cursor, Some((2500, 899)));
    // Y se puede seguir moviendo desde ahí hacia el monitor principal.
    e.emulate(&InputEvent::MotionRelative {
        dx: -1000.0,
        dy: 100.0,
    })
    .unwrap();
    assert_eq!(e.sink().cursor, Some((1500, 999)));
}

#[test]
fn origen_negativo_monitor_a_la_izquierda() {
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
fn movimiento_no_finito_se_ignora() {
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
fn rueda_v120_vertical_y_horizontal() {
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
fn pixeles_de_trackpad_acumulan_fracciones_de_muesca() {
    let mut e = emu(single(800, 600), (0, 0));
    for _ in 0..100 {
        e.emulate(&InputEvent::Scroll(Scroll::Pixels { dx: -0.5, dy: 1.0 }))
            .unwrap();
    }
    // 100 px = una muesca (120); -50 px = -60.
    assert_eq!(wheel_total(&e, MOUSEEVENTF_WHEEL), 120);
    assert_eq!(wheel_total(&e, MOUSEEVENTF_HWHEEL), -60);
}

proptest! {
    /// Cualquier píxel del escritorio virtual vuelve exacto tras normalizar y
    /// desnormalizar como Windows.
    #[test]
    fn normalizar_aterriza_en_el_mismo_pixel(
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

    /// Los píxeles de trackpad no se pierden: lo emitido difiere del total en menos de
    /// una unidad v120.
    #[test]
    fn el_scroll_en_pixeles_no_pierde_nada(deltas in prop::collection::vec(-50.0f64..50.0, 1..200)) {
        let mut e = emu(single(800, 600), (0, 0));
        for dy in &deltas {
            e.emulate(&InputEvent::Scroll(Scroll::Pixels { dx: 0.0, dy: *dy })).unwrap();
        }
        let expected: f64 = deltas.iter().sum::<f64>() * WHEEL_DELTA / PIXELS_PER_NOTCH;
        let got = wheel_total(&e, MOUSEEVENTF_WHEEL) as f64;
        prop_assert!((expected - got).abs() < 1.0 + 1e-6, "{expected} vs {got}");
    }

    /// Con cualquier secuencia de movimientos, el cursor queda dentro de un monitor.
    #[test]
    fn el_cursor_siempre_queda_en_un_monitor(
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
fn sincroniza_modificadores_soltando_primero() {
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
    // Pedir el mismo estado no manda nada.
    let before = e.sink().calls.len();
    e.emulate(&InputEvent::Modifiers(target)).unwrap();
    assert_eq!(e.sink().calls.len(), before);
}

#[test]
fn release_all_suelta_todo_en_una_llamada() {
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
    // Sin nada presionado no se llama a SendInput.
    let before = e.sink().calls.len();
    e.release_all().unwrap();
    assert_eq!(e.sink().calls.len(), before);
}

#[test]
fn si_sendinput_falla_el_estado_no_cambia() {
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
fn escritorio_vacio_es_error() {
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
fn sin_cursor_se_empieza_en_el_centro() {
    let mut sink = FakeSink::new(single(800, 600), (0, 0));
    sink.cursor = None;
    let e = Emulator::new(sink).unwrap();
    assert_eq!(e.position(), Point::new(400.0, 300.0));
}

#[test]
fn al_cambiar_las_pantallas_la_posicion_se_ajusta() {
    let mut e = emu(two(), (100, 100));
    e.emulate(&InputEvent::MotionAbsolute {
        x: 3000.0,
        y: 500.0,
    })
    .unwrap();
    // Se desconecta el segundo monitor.
    e.sink_mut().desktop = single(1920, 1080);
    e.refresh_desktop().unwrap();
    assert_eq!(e.position(), Point::new(1919.0, 500.0));
    // Tras releer, el siguiente movimiento se manda aunque caiga en el mismo píxel.
    e.sink_mut().cursor = Some((1919, 500));
    e.emulate(&InputEvent::MotionAbsolute {
        x: 1919.0,
        y: 500.0,
    })
    .unwrap();
    assert_eq!(e.sink().cursor, Some((1919, 500)));
}
