//! Máquina de estados del cliente: la máquina que recibe la entrada.
//!
//! Sigue al servidor: aplica lo que llega por los dos canales y devuelve los
//! eventos que hay que inyectar, en orden. Lleva lo que dejó presionado para
//! soltarlo todo al salir el cursor, al caer o cambiar el enlace o por silencio
//! del servidor; si no, una tecla quedaría pegada. No hace E/S.

use super::msg::{MotionDatagram, StreamMsg};
use crate::event::{Change, InputEvent, KeyState, ModifierKey, Pressed};
use crate::geometry::{Point, Screen};

#[derive(Debug, Clone)]
pub struct ClientSession {
    /// Pantallas visibles de esta máquina, para pasar los deltas lógicos a
    /// unidades nativas.
    screens: Vec<Screen>,
    /// Posición del cursor mientras el foco está aquí.
    cursor: Option<Point>,
    /// Secuencia de la última posición aplicada: un movimiento que no sea más
    /// nuevo se descarta.
    last_seq: u64,
    /// Secuencia del Enter: lo anterior es de otra visita del cursor.
    enter_seq: u64,
    pressed: Pressed,
}

impl ClientSession {
    pub fn new(screens: Vec<Screen>) -> Self {
        Self {
            screens,
            cursor: None,
            last_seq: 0,
            enter_seq: 0,
            pressed: Pressed::new(),
        }
    }

    /// Las pantallas de esta máquina cambiaron.
    pub fn set_screens(&mut self, screens: Vec<Screen>) {
        self.screens = screens;
    }

    /// Si el cursor está en esta máquina.
    pub fn is_active(&self) -> bool {
        self.cursor.is_some()
    }

    pub fn cursor(&self) -> Option<Point> {
        self.cursor
    }

    pub fn pressed(&self) -> &Pressed {
        &self.pressed
    }

    pub fn on_stream(&mut self, msg: StreamMsg) -> Vec<InputEvent> {
        let mut out = Vec::new();
        match msg {
            StreamMsg::Enter {
                seq,
                pos,
                modifiers,
            } => {
                // Un Enter sin Leave (el servidor se reinició): lo anterior se suelta.
                out.extend(self.pressed.release_all());
                self.last_seq = seq;
                self.enter_seq = seq;
                self.cursor = Some(pos);
                out.push(absolute(pos));
                for m in modifiers.iter() {
                    self.press(ModifierKey::usage(m), &mut out);
                }
            }
            StreamMsg::Leave => out.extend(self.release()),
            StreamMsg::Key { usage, state } if self.is_active() => {
                self.apply(InputEvent::Key { usage, state }, &mut out);
            }
            StreamMsg::Button {
                button,
                state,
                seq,
                pos,
            } if self.is_active() => {
                self.sync(seq, pos, &mut out);
                self.apply(InputEvent::Button { button, state }, &mut out);
            }
            StreamMsg::Sync { seq, pos } if self.is_active() => self.sync(seq, pos, &mut out),
            StreamMsg::Key { .. }
            | StreamMsg::Button { .. }
            | StreamMsg::Sync { .. }
            | StreamMsg::Heartbeat => {}
        }
        out
    }

    pub fn on_datagram(&mut self, d: MotionDatagram) -> Vec<InputEvent> {
        let mut out = Vec::new();
        let Some(pos) = self.cursor else {
            return out;
        };
        if d.seq <= self.enter_seq {
            return out;
        }
        // Un Sync o un botón por el stream puede adelantarse a su datagrama: la
        // posición ya está, pero el scroll que traía no se pierde.
        if d.seq > self.last_seq {
            self.last_seq = d.seq;
            let target = match d.resync {
                Some(p) => p,
                None => {
                    let scale = self.scale_at(pos);
                    Point::new(pos.x + d.dx * scale, pos.y + d.dy * scale)
                }
            };
            self.move_to(target, &mut out);
        }
        out.extend(d.scroll.map(InputEvent::Scroll));
        out
    }

    /// El enlace cayó o cambió, o el servidor dejó de dar señales: se suelta
    /// todo y se espera un Enter nuevo.
    pub fn link_lost(&mut self) -> Vec<InputEvent> {
        self.release()
    }

    fn release(&mut self) -> Vec<InputEvent> {
        self.cursor = None;
        self.pressed.release_all()
    }

    /// Posición que el servidor tenía al enviar el datagrama `seq`; si ya llegó
    /// uno más nuevo, su posición manda.
    fn sync(&mut self, seq: u64, pos: Point, out: &mut Vec<InputEvent>) {
        if seq >= self.last_seq {
            self.last_seq = seq;
            self.move_to(pos, out);
        }
    }

    fn press(&mut self, usage: crate::event::HidUsage, out: &mut Vec<InputEvent>) {
        self.apply(
            InputEvent::Key {
                usage,
                state: KeyState::Pressed,
            },
            out,
        );
    }

    /// Inyecta la tecla o el botón si cambia algo (o es repetición); una suelta
    /// de algo que aquí no está presionado se descarta.
    fn apply(&mut self, event: InputEvent, out: &mut Vec<InputEvent>) {
        match self.pressed.record(&event) {
            Change::Changed | Change::Repeat => out.push(event),
            Change::Ignored => {}
        }
    }

    fn move_to(&mut self, pos: Point, out: &mut Vec<InputEvent>) {
        if self.cursor != Some(pos) {
            self.cursor = Some(pos);
            out.push(absolute(pos));
        }
    }

    fn scale_at(&self, pos: Point) -> f64 {
        self.screens
            .iter()
            .find(|s| s.rect.contains(pos))
            .or_else(|| {
                self.screens
                    .iter()
                    .min_by(|a, b| a.rect.distance(pos).total_cmp(&b.rect.distance(pos)))
            })
            .map_or(1.0, |s| s.scale)
    }
}

fn absolute(p: Point) -> InputEvent {
    InputEvent::MotionAbsolute { x: p.x, y: p.y }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::event::{Button, HidUsage, Modifiers, Scroll};
    use crate::session::testutil::mon;

    fn client() -> ClientSession {
        ClientSession::new(vec![
            mon("m34", 0, 3440, 1440, 1.0),
            mon("m32", 3440, 3840, 2160, 1.5),
        ])
    }

    fn enter(seq: u64, x: f64) -> StreamMsg {
        StreamMsg::Enter {
            seq,
            pos: Point::new(x, 100.0),
            modifiers: Modifiers::EMPTY,
        }
    }

    fn key(id: u16, state: KeyState) -> StreamMsg {
        StreamMsg::Key {
            usage: HidUsage::key(id),
            state,
        }
    }

    fn ev_key(id: u16, state: KeyState) -> InputEvent {
        InputEvent::Key {
            usage: HidUsage::key(id),
            state,
        }
    }

    fn motion(seq: u64, dx: f64, dy: f64) -> MotionDatagram {
        MotionDatagram {
            seq,
            dx,
            dy,
            scroll: None,
            resync: None,
        }
    }

    #[test]
    fn sin_enter_no_se_inyecta_nada() {
        let mut c = client();
        assert!(c.on_datagram(motion(1, 1.0, 1.0)).is_empty());
        assert!(c.on_stream(key(0x04, KeyState::Pressed)).is_empty());
        assert!(c.on_stream(StreamMsg::Heartbeat).is_empty());
        assert!(!c.is_active());
    }

    #[test]
    fn enter_mueve_y_aprieta_los_modificadores() {
        let mut c = client();
        let out = c.on_stream(StreamMsg::Enter {
            seq: 5,
            pos: Point::new(10.0, 20.0),
            modifiers: Modifiers::from_iter([ModifierKey::ShiftLeft, ModifierKey::MetaRight]),
        });
        assert_eq!(
            out,
            vec![
                InputEvent::MotionAbsolute { x: 10.0, y: 20.0 },
                ev_key(0xE1, KeyState::Pressed),
                ev_key(0xE7, KeyState::Pressed),
            ]
        );
        // Datagramas de antes del Enter no valen.
        assert!(c.on_datagram(motion(5, 1.0, 0.0)).is_empty());
        assert_eq!(
            c.on_datagram(motion(6, 1.0, 0.0)),
            vec![InputEvent::MotionAbsolute { x: 11.0, y: 20.0 }]
        );
    }

    #[test]
    fn el_delta_logico_se_escala_por_la_pantalla() {
        let mut c = client();
        c.on_stream(enter(0, 4000.0));
        assert_eq!(
            c.on_datagram(motion(1, 10.0, -2.0)),
            vec![InputEvent::MotionAbsolute { x: 4015.0, y: 97.0 }]
        );
    }

    #[test]
    fn datagrama_atrasado_se_descarta_y_la_resincronizacion_manda() {
        let mut c = client();
        c.on_stream(enter(0, 100.0));
        c.on_datagram(motion(2, 5.0, 0.0));
        assert!(c.on_datagram(motion(1, 50.0, 0.0)).is_empty());
        assert!(c.on_datagram(motion(2, 50.0, 0.0)).is_empty());
        let out = c.on_datagram(MotionDatagram {
            seq: 3,
            dx: 1.0,
            dy: 0.0,
            scroll: Some(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: 120,
            }),
            resync: Some(Point::new(300.0, 300.0)),
        });
        assert_eq!(
            out,
            vec![
                InputEvent::MotionAbsolute { x: 300.0, y: 300.0 },
                InputEvent::Scroll(Scroll::Wheel {
                    dx_v120: 0,
                    dy_v120: 120
                }),
            ]
        );
        assert_eq!(c.cursor(), Some(Point::new(300.0, 300.0)));
    }

    #[test]
    fn el_boton_cae_en_su_posicion_salvo_que_haya_algo_mas_nuevo() {
        let mut c = client();
        c.on_stream(enter(0, 100.0));
        let click = |seq, x| StreamMsg::Button {
            button: Button::Left,
            state: KeyState::Pressed,
            seq,
            pos: Point::new(x, 100.0),
        };
        // El datagrama 1 se perdió: el botón trae la posición.
        assert_eq!(
            c.on_stream(click(1, 150.0)),
            vec![
                InputEvent::MotionAbsolute { x: 150.0, y: 100.0 },
                InputEvent::Button {
                    button: Button::Left,
                    state: KeyState::Pressed
                },
            ]
        );
        c.on_datagram(motion(3, 10.0, 0.0));
        // Botón con seq 2 después del datagrama 3: no mueve hacia atrás.
        let out = c.on_stream(StreamMsg::Button {
            button: Button::Left,
            state: KeyState::Released,
            seq: 2,
            pos: Point::new(155.0, 100.0),
        });
        assert_eq!(
            out,
            vec![InputEvent::Button {
                button: Button::Left,
                state: KeyState::Released
            }]
        );
        assert_eq!(c.cursor(), Some(Point::new(160.0, 100.0)));
    }

    #[test]
    fn sync_corrige_la_posicion_sin_retroceder() {
        let mut c = client();
        c.on_stream(enter(0, 100.0));
        c.on_datagram(motion(2, 5.0, 0.0));
        assert!(
            c.on_stream(StreamMsg::Sync {
                seq: 1,
                pos: Point::new(0.0, 0.0)
            })
            .is_empty()
        );
        assert_eq!(
            c.on_stream(StreamMsg::Sync {
                seq: 2,
                pos: Point::new(104.0, 100.0)
            }),
            vec![InputEvent::MotionAbsolute { x: 104.0, y: 100.0 }]
        );
        assert!(c.on_datagram(motion(2, 5.0, 0.0)).is_empty());
    }

    #[test]
    fn el_scroll_de_un_datagrama_adelantado_por_el_stream_no_se_pierde() {
        let mut c = client();
        c.on_stream(enter(4, 100.0));
        c.on_stream(StreamMsg::Sync {
            seq: 5,
            pos: Point::new(110.0, 100.0),
        });
        let scroll = Scroll::Wheel {
            dx_v120: 0,
            dy_v120: -120,
        };
        let late = MotionDatagram {
            scroll: Some(scroll),
            ..motion(5, 10.0, 0.0)
        };
        assert_eq!(c.on_datagram(late), vec![InputEvent::Scroll(scroll)]);
        assert_eq!(c.cursor(), Some(Point::new(110.0, 100.0)));
        // Uno de antes del Enter no trae nada de esta visita.
        let old = MotionDatagram {
            scroll: Some(scroll),
            ..motion(4, 10.0, 0.0)
        };
        assert!(c.on_datagram(old).is_empty());
    }

    #[test]
    fn suelta_de_algo_no_presionado_se_ignora_y_la_repeticion_pasa() {
        let mut c = client();
        c.on_stream(enter(0, 0.0));
        assert!(c.on_stream(key(0x04, KeyState::Released)).is_empty());
        assert_eq!(
            c.on_stream(key(0x04, KeyState::Pressed)),
            vec![ev_key(0x04, KeyState::Pressed)]
        );
        assert_eq!(
            c.on_stream(key(0x04, KeyState::Pressed)),
            vec![ev_key(0x04, KeyState::Pressed)]
        );
    }

    #[test]
    fn leave_suelta_todo() {
        let mut c = client();
        c.on_stream(enter(0, 0.0));
        c.on_stream(key(0xE0, KeyState::Pressed));
        c.on_stream(key(0x06, KeyState::Pressed));
        assert_eq!(
            c.on_stream(StreamMsg::Leave),
            vec![
                ev_key(0x06, KeyState::Released),
                ev_key(0xE0, KeyState::Released)
            ]
        );
        assert!(!c.is_active());
        assert!(c.on_datagram(motion(9, 1.0, 0.0)).is_empty());
    }

    #[test]
    fn enter_repetido_suelta_lo_anterior() {
        let mut c = client();
        c.on_stream(enter(0, 0.0));
        c.on_stream(key(0x04, KeyState::Pressed));
        let out = c.on_stream(enter(0, 10.0));
        assert_eq!(out[0], ev_key(0x04, KeyState::Released));
    }

    #[test]
    fn caida_del_enlace_con_una_tecla_presionada_la_suelta() {
        let mut c = client();
        c.on_stream(enter(0, 0.0));
        c.on_stream(key(0x16, KeyState::Pressed)); // S
        assert!(c.pressed().is_key_pressed(HidUsage::key(0x16)));
        assert_eq!(c.link_lost(), vec![ev_key(0x16, KeyState::Released)]);
        assert!(c.pressed().is_empty());
        assert!(!c.is_active());
        assert!(c.link_lost().is_empty());
    }

    fn any_msg() -> impl Strategy<Value = StreamMsg> {
        let state = prop_oneof![Just(KeyState::Pressed), Just(KeyState::Released)];
        let usage = prop_oneof![0x04u16..0x0A, 0xE0u16..=0xE7].prop_map(HidUsage::key);
        let button = prop_oneof![Just(Button::Left), Just(Button::Right)];
        prop_oneof![
            (0u64..4, any::<u8>()).prop_map(|(seq, bits)| StreamMsg::Enter {
                seq,
                pos: Point::new(0.0, 0.0),
                modifiers: Modifiers::from_bits(bits),
            }),
            Just(StreamMsg::Leave),
            Just(StreamMsg::Heartbeat),
            (usage, state.clone()).prop_map(|(usage, state)| StreamMsg::Key { usage, state }),
            (button, state, 0u64..4).prop_map(|(button, state, seq)| StreamMsg::Button {
                button,
                state,
                seq,
                pos: Point::new(1.0, 1.0),
            }),
        ]
    }

    proptest! {
        /// Pase lo que pase, al caer el enlace lo inyectado queda todo suelto: lo
        /// que se presionó en el sistema se soltó exactamente una vez.
        #[test]
        fn nada_queda_pegado(msgs in prop::collection::vec(any_msg(), 0..64)) {
            let mut c = client();
            let mut system = Pressed::new();
            let inject = |events: Vec<InputEvent>, system: &mut Pressed| {
                for e in events {
                    if let InputEvent::Key { state: KeyState::Released, .. }
                    | InputEvent::Button { state: KeyState::Released, .. } = e {
                        // Nunca se suelta algo que el sistema no tiene presionado.
                        prop_assert_eq!(system.record(&e), Change::Changed);
                    } else {
                        system.record(&e);
                    }
                }
                Ok(())
            };
            for m in msgs {
                let out = c.on_stream(m);
                inject(out, &mut system)?;
            }
            let out = c.link_lost();
            inject(out, &mut system)?;
            prop_assert!(system.is_empty());
        }
    }
}
