//! Traducción de lo que entrega el CGEventTap al modelo portable de eventos.
//!
//! No toca ninguna API de Apple: `sys` lee cada `CGEvent` a un [`RawEvent`] y
//! esto decide qué [`InputEvent`] salen y si el evento se traga. Así la lógica
//! se prueba en Linux.

use crate::event::keymap::{self, MacKeycode};
use crate::event::{Button, HidUsage, InputEvent, KeyState, ModifierKey, Scroll};
use crate::geometry::Point;

/// Marca que la inyección de Tandem pone en `kCGEventSourceUserData` de sus
/// eventos ("TNDM"). La captura los deja pasar sin reenviarlos, para no hacer eco
/// si las dos cosas corren en la misma Mac.
pub const INJECTED_MARKER: i64 = 0x544E_444D;

/// Keycode de Bloq Mayús en Mac. macOS manda un `FlagsChanged` por cada pulsación
/// (al activarse y al desactivarse), nunca el soltar.
const MAC_CAPS_LOCK: u16 = 0x39;

/// Bits dependientes del dispositivo de `CGEventFlags` (los `NX_DEVICE*KEYMASK`
/// de IOKit): dicen qué lado de cada modificador está abajo.
const fn device_bit(key: ModifierKey) -> u64 {
    match key {
        ModifierKey::ControlLeft => 0x0000_0001,
        ModifierKey::ShiftLeft => 0x0000_0002,
        ModifierKey::ShiftRight => 0x0000_0004,
        ModifierKey::MetaLeft => 0x0000_0008,
        ModifierKey::MetaRight => 0x0000_0010,
        ModifierKey::AltLeft => 0x0000_0020,
        ModifierKey::AltRight => 0x0000_0040,
        ModifierKey::ControlRight => 0x0000_2000,
    }
}

const ALL_DEVICE_BITS: u64 = 0x0000_207F;

/// `NSEventSubtype` de las teclas especiales (volumen, brillo, multimedia) que
/// llegan como `NSSystemDefined`.
pub const SUBTYPE_AUX_CONTROL_BUTTONS: i16 = 8;

/// Un `CGEvent` leído, sin punteros de Apple.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RawEvent {
    KeyDown {
        keycode: u16,
    },
    KeyUp {
        keycode: u16,
    },
    /// Cambió un modificador. `flags` es `CGEventGetFlags` completo.
    FlagsChanged {
        keycode: u16,
        flags: u64,
    },
    /// `button`: `kCGMouseEventButtonNumber` (0 izquierdo, 1 derecho, 2 medio…).
    MouseDown {
        button: i64,
        location: Point,
    },
    MouseUp {
        button: i64,
        location: Point,
    },
    /// Movimiento con o sin botón (`*Dragged` incluido). `dx`/`dy`:
    /// `kCGMouseEventDeltaX/Y`, que siguen llegando con el cursor desasociado.
    Moved {
        dx: f64,
        dy: f64,
        location: Point,
    },
    /// `axis1` es vertical y `axis2` horizontal, con el signo que entrega macOS
    /// (ya invertido si el "scroll natural" está activo).
    Scroll {
        continuous: bool,
        /// `kCGScrollWheelEventFixedPtDeltaAxis*`: líneas, con fracción.
        lines_axis1: f64,
        lines_axis2: f64,
        /// `kCGScrollWheelEventPointDeltaAxis*`: píxeles, para trackpad.
        points_axis1: f64,
        points_axis2: f64,
    },
    /// `NSSystemDefined`: `subtype` y `data1` del `NSEvent`.
    SystemDefined {
        subtype: i16,
        data1: i64,
    },
    /// El sistema apagó el tap por tardar (`kCGEventTapDisabledByTimeout`) o por
    /// entrada del usuario (`kCGEventTapDisabledByUserInput`).
    TapDisabled,
    /// Cualquier otro tipo: no es entrada que nos importe.
    Other,
}

/// Qué hacer con el evento original.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapAction {
    /// Devolverlo: sigue su camino en esta Mac.
    Pass,
    /// Devolver NULL: la Mac no lo ve (el foco está en la otra máquina).
    Swallow,
    /// Volver a encender el tap y dejar pasar el evento.
    Reenable,
}

/// Traduce eventos del tap. Lleva los modificadores que vio abajo, porque
/// `FlagsChanged` no dice si la tecla bajó o subió.
#[derive(Debug, Clone, Default)]
pub struct Translator {
    modifiers_down: crate::event::Modifiers,
}

impl Translator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Traduce `raw` y decide su destino.
    ///
    /// - `grabbed`: el foco está en la otra máquina; la entrada se traga y no sale
    ///   la posición absoluta (el cursor local está congelado).
    /// - `natural_scroll`: la preferencia de macOS, para deshacerla (ver [`Scroll`]).
    pub fn translate(
        &mut self,
        raw: &RawEvent,
        grabbed: bool,
        natural_scroll: bool,
        emit: &mut impl FnMut(InputEvent),
    ) -> TapAction {
        let input = match *raw {
            RawEvent::TapDisabled => return TapAction::Reenable,
            RawEvent::Other => return TapAction::Pass,
            RawEvent::KeyDown { keycode } => {
                key(keycode, KeyState::Pressed, emit);
                true
            }
            RawEvent::KeyUp { keycode } => {
                key(keycode, KeyState::Released, emit);
                true
            }
            RawEvent::FlagsChanged { keycode, flags } => {
                self.flags_changed(keycode, flags, emit);
                true
            }
            RawEvent::MouseDown { button, location } => {
                if !grabbed {
                    emit(InputEvent::MotionAbsolute {
                        x: location.x,
                        y: location.y,
                    });
                }
                emit(InputEvent::Button {
                    button: mac_button(button),
                    state: KeyState::Pressed,
                });
                true
            }
            RawEvent::MouseUp { button, location } => {
                if !grabbed {
                    emit(InputEvent::MotionAbsolute {
                        x: location.x,
                        y: location.y,
                    });
                }
                emit(InputEvent::Button {
                    button: mac_button(button),
                    state: KeyState::Released,
                });
                true
            }
            RawEvent::Moved { dx, dy, location } => {
                // Primero el delta y después la posición real: el servidor suma el
                // delta a la posición anterior, así que en el borde (donde macOS ya
                // dejó el cursor fijo) el empuje cruza, y si no cruza la posición
                // real corrige lo que la aceleración haya cambiado.
                if dx != 0.0 || dy != 0.0 {
                    emit(InputEvent::MotionRelative { dx, dy });
                }
                if !grabbed {
                    emit(InputEvent::MotionAbsolute {
                        x: location.x,
                        y: location.y,
                    });
                }
                true
            }
            RawEvent::Scroll {
                continuous,
                lines_axis1,
                lines_axis2,
                points_axis1,
                points_axis2,
            } => {
                if let Some(s) = scroll(
                    continuous,
                    lines_axis1,
                    lines_axis2,
                    points_axis1,
                    points_axis2,
                    natural_scroll,
                ) {
                    emit(InputEvent::Scroll(s));
                }
                true
            }
            RawEvent::SystemDefined { subtype, data1 } => {
                if subtype != SUBTYPE_AUX_CONTROL_BUTTONS {
                    return TapAction::Pass;
                }
                if let Some((usage, state)) = aux_key(data1) {
                    emit(InputEvent::Key { usage, state });
                }
                true
            }
        };
        if input && grabbed {
            TapAction::Swallow
        } else {
            TapAction::Pass
        }
    }

    /// Modificadores que el traductor cree que están abajo.
    pub fn modifiers_down(&self) -> crate::event::Modifiers {
        self.modifiers_down
    }

    fn flags_changed(&mut self, keycode: u16, flags: u64, emit: &mut impl FnMut(InputEvent)) {
        if keycode == MAC_CAPS_LOCK {
            // Cada FlagsChanged de Bloq Mayús es una pulsación completa: el otro
            // lado alterna su propio bloqueo.
            let usage = HidUsage::key(0x39);
            emit(InputEvent::Key {
                usage,
                state: KeyState::Pressed,
            });
            emit(InputEvent::Key {
                usage,
                state: KeyState::Released,
            });
            return;
        }
        // Fn (0x3F) y cualquier otro sin uso HID estándar no viajan.
        let Some(usage) = keymap::from_mac(MacKeycode(keycode)) else {
            return;
        };
        let Some(modifier) = usage.modifier() else {
            return;
        };
        let was_down = self.modifiers_down.contains(modifier);
        // Los eventos sintéticos a veces no traen los bits por lado: entonces cada
        // FlagsChanged de esa tecla alterna su estado.
        let down = if flags & ALL_DEVICE_BITS != 0 {
            flags & device_bit(modifier) != 0
        } else {
            !was_down
        };
        if down == was_down {
            return;
        }
        if down {
            self.modifiers_down.insert(modifier);
        } else {
            self.modifiers_down.remove(modifier);
        }
        emit(InputEvent::Key {
            usage,
            state: if down {
                KeyState::Pressed
            } else {
                KeyState::Released
            },
        });
    }
}

fn key(keycode: u16, state: KeyState, emit: &mut impl FnMut(InputEvent)) {
    // Se reenvía también la repetición automática como otro `Pressed`: el destino
    // no repite solo una tecla inyectada. Los modificadores solo cuentan por
    // FlagsChanged, que es lo que manda macOS; un KeyDown de uno se ignora para no
    // desincronizar el estado.
    if let Some(usage) = keymap::from_mac(MacKeycode(keycode))
        && usage.modifier().is_none()
    {
        emit(InputEvent::Key { usage, state });
    }
}

/// Botón por `kCGMouseEventButtonNumber`.
pub fn mac_button(number: i64) -> Button {
    match number {
        0 => Button::Left,
        1 => Button::Right,
        2 => Button::Middle,
        3 => Button::Back,
        4 => Button::Forward,
        n => Button::Other(u8::try_from(n).unwrap_or(u8::MAX)),
    }
}

/// Convierte el scroll de macOS a la convención portable: `y` positivo = rueda
/// hacia adelante, `x` positivo = derecha, sin "scroll natural".
///
/// En macOS sin scroll natural, la rueda hacia adelante da `axis1` positivo y la
/// inclinación a la izquierda `axis2` positivo; con scroll natural los dos llegan
/// invertidos.
pub fn scroll(
    continuous: bool,
    lines_axis1: f64,
    lines_axis2: f64,
    points_axis1: f64,
    points_axis2: f64,
    natural: bool,
) -> Option<Scroll> {
    let sign = if natural { -1.0 } else { 1.0 };
    if continuous {
        let dy = points_axis1 * sign;
        let dx = -points_axis2 * sign;
        if dx == 0.0 && dy == 0.0 {
            return None;
        }
        // `+ 0.0` evita un -0.0 que no aporta nada.
        Some(Scroll::Pixels {
            dx: dx + 0.0,
            dy: dy + 0.0,
        })
    } else {
        let dy_v120 = to_v120(lines_axis1 * sign);
        let dx_v120 = to_v120(-lines_axis2 * sign);
        if dx_v120 == 0 && dy_v120 == 0 {
            return None;
        }
        Some(Scroll::Wheel { dx_v120, dy_v120 })
    }
}

fn to_v120(lines: f64) -> i32 {
    let v = (lines * 120.0).round();
    if v.is_nan() {
        0
    } else {
        // `as` satura en los extremos, que es lo que queremos.
        v as i32
    }
}

/// Teclas especiales de `NSSystemDefined` subtipo 8: `data1` lleva el
/// `NX_KEYTYPE_*` en los bits 16–31 y el estado en los 8–15 (0x0A abajo, 0x0B
/// arriba). Devuelve el uso HID equivalente.
pub fn aux_key(data1: i64) -> Option<(HidUsage, KeyState)> {
    let code = (data1 >> 16) & 0xFFFF;
    let state = match (data1 >> 8) & 0xFF {
        0x0A => KeyState::Pressed,
        0x0B => KeyState::Released,
        _ => return None,
    };
    let usage = match code {
        0 => HidUsage::consumer(0xE9),  // NX_KEYTYPE_SOUND_UP
        1 => HidUsage::consumer(0xEA),  // NX_KEYTYPE_SOUND_DOWN
        2 => HidUsage::consumer(0x6F),  // NX_KEYTYPE_BRIGHTNESS_UP
        3 => HidUsage::consumer(0x70),  // NX_KEYTYPE_BRIGHTNESS_DOWN
        7 => HidUsage::consumer(0xE2),  // NX_KEYTYPE_MUTE
        16 => HidUsage::consumer(0xCD), // NX_KEYTYPE_PLAY
        17 => HidUsage::consumer(0xB5), // NX_KEYTYPE_NEXT
        18 => HidUsage::consumer(0xB6), // NX_KEYTYPE_PREVIOUS
        19 => HidUsage::consumer(0xB3), // NX_KEYTYPE_FAST
        20 => HidUsage::consumer(0xB4), // NX_KEYTYPE_REWIND
        _ => return None,
    };
    Some((usage, state))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::event::Modifiers;

    const ORIGIN: Point = Point::new(0.0, 0.0);

    fn run(t: &mut Translator, raw: RawEvent, grabbed: bool) -> (TapAction, Vec<InputEvent>) {
        let mut out = Vec::new();
        let action = t.translate(&raw, grabbed, false, &mut |e| out.push(e));
        (action, out)
    }

    fn key_ev(id: u16, state: KeyState) -> InputEvent {
        InputEvent::Key {
            usage: HidUsage::key(id),
            state,
        }
    }

    #[test]
    fn teclas_por_keycode_de_mac() {
        let mut t = Translator::new();
        // kVK_ANSI_A = 0x00 → HID 0x04.
        let (a, ev) = run(&mut t, RawEvent::KeyDown { keycode: 0x00 }, false);
        assert_eq!(a, TapAction::Pass);
        assert_eq!(ev, vec![key_ev(0x04, KeyState::Pressed)]);
        let (_, ev) = run(&mut t, RawEvent::KeyUp { keycode: 0x00 }, false);
        assert_eq!(ev, vec![key_ev(0x04, KeyState::Released)]);
        // kVK_Return = 0x24 → HID 0x28.
        let (_, ev) = run(&mut t, RawEvent::KeyDown { keycode: 0x24 }, false);
        assert_eq!(ev, vec![key_ev(0x28, KeyState::Pressed)]);
    }

    #[test]
    fn todas_las_teclas_de_la_tabla_mac_se_capturan() {
        // Cada keycode con uso HID directo sale con ese uso (salvo los
        // modificadores, que llegan como FlagsChanged).
        let mut t = Translator::new();
        for (usage, mac) in keymap::mac_usages() {
            if keymap::from_mac(mac) != Some(usage) || usage.modifier().is_some() {
                continue;
            }
            let (_, ev) = run(&mut t, RawEvent::KeyDown { keycode: mac.0 }, true);
            assert_eq!(
                ev,
                vec![InputEvent::Key {
                    usage,
                    state: KeyState::Pressed
                }]
            );
        }
    }

    #[test]
    fn keycode_desconocido_no_emite_pero_se_traga_si_hay_grab() {
        let mut t = Translator::new();
        let (a, ev) = run(&mut t, RawEvent::KeyDown { keycode: 0x7FFF }, true);
        assert_eq!(a, TapAction::Swallow);
        assert!(ev.is_empty());
    }

    #[test]
    fn con_grab_se_traga_la_entrada_y_sin_grab_pasa() {
        let inputs = [
            RawEvent::KeyDown { keycode: 0 },
            RawEvent::KeyUp { keycode: 0 },
            RawEvent::FlagsChanged {
                keycode: 0x38,
                flags: 0x0002_0002,
            },
            RawEvent::MouseDown {
                button: 0,
                location: ORIGIN,
            },
            RawEvent::MouseUp {
                button: 0,
                location: ORIGIN,
            },
            RawEvent::Moved {
                dx: 1.0,
                dy: 0.0,
                location: ORIGIN,
            },
            RawEvent::Scroll {
                continuous: false,
                lines_axis1: 1.0,
                lines_axis2: 0.0,
                points_axis1: 10.0,
                points_axis2: 0.0,
            },
            RawEvent::SystemDefined {
                subtype: SUBTYPE_AUX_CONTROL_BUTTONS,
                data1: 0x0000_0A00,
            },
        ];
        for raw in inputs {
            assert_eq!(run(&mut Translator::new(), raw, true).0, TapAction::Swallow);
            assert_eq!(run(&mut Translator::new(), raw, false).0, TapAction::Pass);
        }
    }

    #[test]
    fn otros_eventos_pasan_siempre() {
        for grabbed in [false, true] {
            let mut t = Translator::new();
            assert_eq!(
                run(&mut t, RawEvent::Other, grabbed),
                (TapAction::Pass, vec![])
            );
            // Otro subtipo de NSSystemDefined (p. ej. 7, botones auxiliares viejos).
            let raw = RawEvent::SystemDefined {
                subtype: 7,
                data1: 0,
            };
            assert_eq!(run(&mut t, raw, grabbed), (TapAction::Pass, vec![]));
        }
    }

    #[test]
    fn tap_apagado_se_reactiva() {
        let mut t = Translator::new();
        assert_eq!(
            run(&mut t, RawEvent::TapDisabled, true),
            (TapAction::Reenable, vec![])
        );
    }

    #[test]
    fn modificadores_por_lado_con_bits_del_dispositivo() {
        let mut t = Translator::new();
        // Shift izquierdo abajo: bit genérico 0x20000 + NX_DEVICELSHIFTKEYMASK.
        let (_, ev) = run(
            &mut t,
            RawEvent::FlagsChanged {
                keycode: 0x38,
                flags: 0x0002_0002,
            },
            false,
        );
        assert_eq!(ev, vec![key_ev(0xE1, KeyState::Pressed)]);
        // Shift derecho abajo con el izquierdo todavía abajo.
        let (_, ev) = run(
            &mut t,
            RawEvent::FlagsChanged {
                keycode: 0x3C,
                flags: 0x0002_0006,
            },
            false,
        );
        assert_eq!(ev, vec![key_ev(0xE5, KeyState::Pressed)]);
        // Suelta el izquierdo: el genérico sigue puesto por el derecho.
        let (_, ev) = run(
            &mut t,
            RawEvent::FlagsChanged {
                keycode: 0x38,
                flags: 0x0002_0004,
            },
            false,
        );
        assert_eq!(ev, vec![key_ev(0xE1, KeyState::Released)]);
        assert_eq!(
            t.modifiers_down(),
            [ModifierKey::ShiftRight].into_iter().collect::<Modifiers>()
        );
        // Cmd derecho (0x36), Option izquierdo (0x3A), Ctrl derecho (0x3E).
        let (_, ev) = run(
            &mut t,
            RawEvent::FlagsChanged {
                keycode: 0x36,
                flags: 0x0010_0014,
            },
            false,
        );
        assert_eq!(ev, vec![key_ev(0xE7, KeyState::Pressed)]);
        let (_, ev) = run(
            &mut t,
            RawEvent::FlagsChanged {
                keycode: 0x3A,
                flags: 0x0018_0034,
            },
            false,
        );
        assert_eq!(ev, vec![key_ev(0xE2, KeyState::Pressed)]);
        let (_, ev) = run(
            &mut t,
            RawEvent::FlagsChanged {
                keycode: 0x3E,
                flags: 0x001C_2034,
            },
            false,
        );
        assert_eq!(ev, vec![key_ev(0xE4, KeyState::Pressed)]);
    }

    #[test]
    fn flags_sin_bits_por_lado_alternan() {
        let mut t = Translator::new();
        // Cmd izquierdo sintético: solo el bit genérico.
        let raw = RawEvent::FlagsChanged {
            keycode: 0x37,
            flags: 0x0010_0000,
        };
        assert_eq!(
            run(&mut t, raw, false).1,
            vec![key_ev(0xE3, KeyState::Pressed)]
        );
        let raw = RawEvent::FlagsChanged {
            keycode: 0x37,
            flags: 0,
        };
        assert_eq!(
            run(&mut t, raw, false).1,
            vec![key_ev(0xE3, KeyState::Released)]
        );
    }

    #[test]
    fn flags_repetidos_no_duplican() {
        let mut t = Translator::new();
        let raw = RawEvent::FlagsChanged {
            keycode: 0x3B,
            flags: 0x0004_0001,
        };
        assert_eq!(run(&mut t, raw, false).1.len(), 1);
        assert!(run(&mut t, raw, false).1.is_empty());
    }

    #[test]
    fn bloq_mayus_es_una_pulsacion_completa_cada_vez() {
        let mut t = Translator::new();
        for flags in [0x0001_0000, 0] {
            let (_, ev) = run(
                &mut t,
                RawEvent::FlagsChanged {
                    keycode: 0x39,
                    flags,
                },
                true,
            );
            assert_eq!(
                ev,
                vec![
                    key_ev(0x39, KeyState::Pressed),
                    key_ev(0x39, KeyState::Released)
                ]
            );
        }
    }

    #[test]
    fn fn_no_viaja() {
        let mut t = Translator::new();
        let raw = RawEvent::FlagsChanged {
            keycode: 0x3F,
            flags: 0x0080_0000,
        };
        assert_eq!(run(&mut t, raw, true), (TapAction::Swallow, vec![]));
    }

    #[test]
    fn botones_por_numero() {
        assert_eq!(mac_button(0), Button::Left);
        assert_eq!(mac_button(1), Button::Right);
        assert_eq!(mac_button(2), Button::Middle);
        assert_eq!(mac_button(3), Button::Back);
        assert_eq!(mac_button(4), Button::Forward);
        assert_eq!(mac_button(9), Button::Other(9));
        assert_eq!(mac_button(1000), Button::Other(255));
        assert_eq!(mac_button(-1), Button::Other(255));
    }

    #[test]
    fn clic_con_foco_local_informa_la_posicion_antes() {
        let mut t = Translator::new();
        let at = Point::new(10.0, 20.0);
        let (_, ev) = run(
            &mut t,
            RawEvent::MouseDown {
                button: 1,
                location: at,
            },
            false,
        );
        assert_eq!(
            ev,
            vec![
                InputEvent::MotionAbsolute { x: 10.0, y: 20.0 },
                InputEvent::Button {
                    button: Button::Right,
                    state: KeyState::Pressed
                }
            ]
        );
        let (_, ev) = run(
            &mut t,
            RawEvent::MouseUp {
                button: 1,
                location: at,
            },
            true,
        );
        assert_eq!(
            ev,
            vec![InputEvent::Button {
                button: Button::Right,
                state: KeyState::Released
            }]
        );
    }

    #[test]
    fn movimiento_local_manda_delta_y_despues_posicion() {
        let mut t = Translator::new();
        let (_, ev) = run(
            &mut t,
            RawEvent::Moved {
                dx: 3.0,
                dy: -2.0,
                location: Point::new(1919.0, 500.0),
            },
            false,
        );
        assert_eq!(
            ev,
            vec![
                InputEvent::MotionRelative { dx: 3.0, dy: -2.0 },
                InputEvent::MotionAbsolute {
                    x: 1919.0,
                    y: 500.0
                }
            ]
        );
    }

    #[test]
    fn movimiento_remoto_solo_manda_delta() {
        let mut t = Translator::new();
        let (a, ev) = run(
            &mut t,
            RawEvent::Moved {
                dx: 3.0,
                dy: 0.0,
                location: ORIGIN,
            },
            true,
        );
        assert_eq!(a, TapAction::Swallow);
        assert_eq!(ev, vec![InputEvent::MotionRelative { dx: 3.0, dy: 0.0 }]);
        // Un movimiento nulo (p. ej. tras un warp) no manda nada.
        let (_, ev) = run(
            &mut t,
            RawEvent::Moved {
                dx: 0.0,
                dy: 0.0,
                location: ORIGIN,
            },
            true,
        );
        assert!(ev.is_empty());
    }

    #[test]
    fn rueda_en_v120_sin_scroll_natural() {
        // Rueda hacia adelante una muesca, sin scroll natural: axis1 = +1.
        assert_eq!(
            scroll(false, 1.0, 0.0, 10.0, 0.0, false),
            Some(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: 120
            })
        );
        // Con scroll natural macOS la entrega invertida; se deshace.
        assert_eq!(
            scroll(false, -1.0, 0.0, -10.0, 0.0, true),
            Some(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: 120
            })
        );
        // Inclinación a la izquierda: axis2 positivo → x negativo.
        assert_eq!(
            scroll(false, 0.0, 0.5, 0.0, 5.0, false),
            Some(Scroll::Wheel {
                dx_v120: -60,
                dy_v120: 0
            })
        );
        // Rueda de alta resolución: fracciones de muesca.
        assert_eq!(
            scroll(false, 0.1, 0.0, 1.0, 0.0, false),
            Some(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: 12
            })
        );
    }

    #[test]
    fn trackpad_en_pixeles() {
        assert_eq!(
            scroll(true, 0.0, 0.0, 12.5, -3.0, false),
            Some(Scroll::Pixels { dx: 3.0, dy: 12.5 })
        );
        assert_eq!(
            scroll(true, 0.0, 0.0, 12.5, -3.0, true),
            Some(Scroll::Pixels {
                dx: -3.0,
                dy: -12.5
            })
        );
    }

    #[test]
    fn scroll_nulo_no_sale() {
        assert_eq!(scroll(true, 1.0, 1.0, 0.0, 0.0, false), None);
        assert_eq!(scroll(false, 0.001, 0.0, 9.0, 0.0, false), None);
        assert_eq!(scroll(false, f64::NAN, 0.0, 0.0, 0.0, false), None);
    }

    #[test]
    fn el_traductor_deshace_el_scroll_natural() {
        let mut t = Translator::new();
        let raw = RawEvent::Scroll {
            continuous: true,
            lines_axis1: 0.0,
            lines_axis2: 0.0,
            points_axis1: -4.0,
            points_axis2: 0.0,
        };
        let mut out = Vec::new();
        t.translate(&raw, true, true, &mut |e| out.push(e));
        assert_eq!(
            out,
            vec![InputEvent::Scroll(Scroll::Pixels { dx: 0.0, dy: 4.0 })]
        );
    }

    #[test]
    fn teclas_multimedia() {
        let down = |code: i64| (code << 16) | 0x0A00;
        let up = |code: i64| (code << 16) | 0x0B00;
        assert_eq!(
            aux_key(down(0)),
            Some((HidUsage::consumer(0xE9), KeyState::Pressed))
        );
        assert_eq!(
            aux_key(up(1)),
            Some((HidUsage::consumer(0xEA), KeyState::Released))
        );
        assert_eq!(
            aux_key(down(7)),
            Some((HidUsage::consumer(0xE2), KeyState::Pressed))
        );
        assert_eq!(
            aux_key(down(16)),
            Some((HidUsage::consumer(0xCD), KeyState::Pressed))
        );
        // La repetición (bit 0) no cambia el estado.
        assert_eq!(
            aux_key(down(17) | 1),
            Some((HidUsage::consumer(0xB5), KeyState::Pressed))
        );
        // Bloq Mayús llega también por FlagsChanged: aquí no.
        assert_eq!(aux_key(down(4)), None);
        // Estado desconocido.
        assert_eq!(aux_key(16 << 16), None);

        let mut t = Translator::new();
        let raw = RawEvent::SystemDefined {
            subtype: SUBTYPE_AUX_CONTROL_BUTTONS,
            data1: up(18),
        };
        assert_eq!(
            run(&mut t, raw, true),
            (
                TapAction::Swallow,
                vec![InputEvent::Key {
                    usage: HidUsage::consumer(0xB6),
                    state: KeyState::Released
                }]
            )
        );
    }

    fn arb_raw() -> impl Strategy<Value = RawEvent> {
        let p = (-5000.0..5000.0f64, -5000.0..5000.0f64).prop_map(|(x, y)| Point::new(x, y));
        prop_oneof![
            any::<u16>().prop_map(|keycode| RawEvent::KeyDown { keycode }),
            any::<u16>().prop_map(|keycode| RawEvent::KeyUp { keycode }),
            (0x36u16..0x40, any::<u64>())
                .prop_map(|(keycode, flags)| RawEvent::FlagsChanged { keycode, flags }),
            (any::<i64>(), p.clone())
                .prop_map(|(button, location)| RawEvent::MouseDown { button, location }),
            (any::<i64>(), p.clone())
                .prop_map(|(button, location)| RawEvent::MouseUp { button, location }),
            (-100.0..100.0f64, -100.0..100.0f64, p)
                .prop_map(|(dx, dy, location)| RawEvent::Moved { dx, dy, location }),
            (
                any::<bool>(),
                -10.0..10.0f64,
                -10.0..10.0f64,
                -100.0..100.0f64,
                -100.0..100.0f64
            )
                .prop_map(|(continuous, l1, l2, p1, p2)| RawEvent::Scroll {
                    continuous,
                    lines_axis1: l1,
                    lines_axis2: l2,
                    points_axis1: p1,
                    points_axis2: p2,
                }),
            (any::<i16>(), any::<i64>())
                .prop_map(|(subtype, data1)| RawEvent::SystemDefined { subtype, data1 }),
            Just(RawEvent::TapDisabled),
            Just(RawEvent::Other),
        ]
    }

    proptest! {
        /// Con el foco remoto nunca se informa la posición del cursor congelado, y
        /// los modificadores que el traductor cree abajo coinciden con los
        /// `Pressed`/`Released` de modificadores que emitió.
        #[test]
        fn invariantes_del_traductor(events in prop::collection::vec((arb_raw(), any::<bool>()), 0..64)) {
            let mut t = Translator::new();
            let mut mods = Modifiers::EMPTY;
            for (raw, grabbed) in events {
                let mut out = Vec::new();
                let action = t.translate(&raw, grabbed, false, &mut |e| out.push(e));
                if !grabbed {
                    prop_assert_ne!(action, TapAction::Swallow);
                }
                for e in out {
                    match e {
                        InputEvent::MotionAbsolute { .. } => prop_assert!(!grabbed),
                        InputEvent::Key { usage, state } => {
                            if let Some(m) = usage.modifier() {
                                match state {
                                    KeyState::Pressed => {
                                        prop_assert!(!mods.contains(m));
                                        mods.insert(m);
                                    }
                                    KeyState::Released => {
                                        prop_assert!(mods.contains(m));
                                        mods.remove(m);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                prop_assert_eq!(mods, t.modifiers_down());
            }
        }
    }
}
