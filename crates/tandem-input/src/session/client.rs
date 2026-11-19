//! Client state machine: the machine that receives the input.
//!
//! It follows the server: applies what arrives on the two channels and returns the
//! events that have to be injected, in order. It keeps what it left pressed so as to
//! release it all when the cursor leaves, when the link drops or changes, or on silence
//! from the server; otherwise a key would be left stuck. It does no I/O.

use super::msg::{MotionDatagram, StreamMsg};
use crate::event::{Change, InputEvent, KeyState, ModifierKey, Pressed};
use crate::geometry::{Point, Screen};

#[derive(Debug, Clone)]
pub struct ClientSession {
    /// Visible screens of this machine, to convert logical deltas into
    /// native units.
    screens: Vec<Screen>,
    /// Cursor position while the focus is here.
    cursor: Option<Point>,
    /// Sequence of the last applied position: a movement that is not
    /// newer is discarded.
    last_seq: u64,
    /// Sequence of the Enter: anything earlier belongs to another visit of the cursor.
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

    /// This machine's screens changed.
    pub fn set_screens(&mut self, screens: Vec<Screen>) {
        self.screens = screens;
    }

    /// Whether the cursor is on this machine.
    pub fn is_active(&self) -> bool {
        self.cursor.is_some()
    }

    pub fn cursor(&self) -> Option<Point> {
        self.cursor
    }

    /// The id of the visible screen that contains `p` (the nearest if it falls in a gap);
    /// empty if none are known.
    pub fn screen_id(&self, p: Point) -> String {
        self.screens
            .iter()
            .find(|s| s.rect.contains(p))
            .or_else(|| {
                self.screens
                    .iter()
                    .min_by(|a, b| a.rect.distance(p).total_cmp(&b.rect.distance(p)))
            })
            .map(|s| s.id.0.clone())
            .unwrap_or_default()
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
                // An Enter without a Leave (the server restarted): what came before is released.
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
            StreamMsg::Key { usage, state, .. } if self.is_active() => {
                self.apply(InputEvent::Key { usage, state }, &mut out);
            }
            StreamMsg::Button {
                button,
                state,
                seq,
                pos,
                ..
            } if self.is_active() => {
                self.sync(seq, pos, &mut out);
                self.apply(InputEvent::Button { button, state }, &mut out);
            }
            StreamMsg::Sync { seq, pos } if self.is_active() => self.sync(seq, pos, &mut out),
            StreamMsg::Key { .. }
            | StreamMsg::Button { .. }
            | StreamMsg::Sync { .. }
            | StreamMsg::Heartbeat(_)
            | StreamMsg::Latency(_) => {}
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
        // A Sync or a button over the stream can get ahead of its datagram: the
        // position is already there, but the scroll it carried is not lost.
        if d.seq > self.last_seq {
            self.last_seq = d.seq;
            let target = match d.resync {
                Some(p) => p,
                // The delta already comes in this machine's native units.
                None => self.on_screen(Point::new(pos.x + d.dx, pos.y + d.dy)),
            };
            self.move_to(target, &mut out);
        }
        out.extend(d.scroll.map(InputEvent::Scroll));
        out
    }

    /// The link dropped or changed, or the server stopped giving signs of life: everything is released
    /// and a new Enter is awaited.
    pub fn link_lost(&mut self) -> Vec<InputEvent> {
        self.release()
    }

    fn release(&mut self) -> Vec<InputEvent> {
        self.cursor = None;
        self.pressed.release_all()
    }

    /// Position the server had when sending datagram `seq`; if a newer one already
    /// arrived, its position wins.
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

    /// Injects the key or button if anything changes (or it is a repeat); a release
    /// of something not pressed here is discarded.
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

    /// The point inside some screen: if it falls in a gap (screens that changed
    /// and the server does not know yet), on the edge of the nearest one. Without known
    /// screens, as is.
    fn on_screen(&self, pos: Point) -> Point {
        if self.screens.is_empty() || self.screens.iter().any(|s| s.rect.contains(pos)) {
            return pos;
        }
        self.screens
            .iter()
            .min_by(|a, b| a.rect.distance(pos).total_cmp(&b.rect.distance(pos)))
            .map_or(pos, |s| s.rect.clamp(pos))
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
            t_us: 0,
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
            t_us: 0,
        }
    }

    #[test]
    fn without_an_enter_nothing_is_injected() {
        let mut c = client();
        assert!(c.on_datagram(motion(1, 1.0, 1.0)).is_empty());
        assert!(c.on_stream(key(0x04, KeyState::Pressed)).is_empty());
        assert!(c.on_stream(StreamMsg::Heartbeat(beat())).is_empty());
        assert!(!c.is_active());
    }

    #[test]
    fn enter_moves_and_holds_the_modifiers() {
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
        // Datagrams from before the Enter are not valid.
        assert!(c.on_datagram(motion(5, 1.0, 0.0)).is_empty());
        assert_eq!(
            c.on_datagram(motion(6, 1.0, 0.0)),
            vec![InputEvent::MotionAbsolute { x: 11.0, y: 20.0 }]
        );
    }

    fn beat() -> crate::session::latency::Beat {
        crate::session::latency::Beat {
            sent_us: 0,
            echo: None,
        }
    }

    #[test]
    fn the_delta_already_comes_in_native_units() {
        let mut c = client();
        c.on_stream(enter(0, 4000.0));
        // On the 32" at 150 %: the server already converted it.
        assert_eq!(
            c.on_datagram(motion(1, 15.0, -3.0)),
            vec![InputEvent::MotionAbsolute { x: 4015.0, y: 97.0 }]
        );
    }

    #[test]
    fn a_delta_that_falls_outside_the_screens_stays_at_the_edge() {
        let mut c = client();
        c.on_stream(enter(0, 100.0));
        // Below the 34" (1440 tall) there is no screen.
        assert_eq!(
            c.on_datagram(motion(1, 0.0, 5000.0)),
            vec![InputEvent::MotionAbsolute {
                x: 100.0,
                y: 1439.0
            }]
        );
    }

    #[test]
    fn a_late_datagram_is_discarded_and_the_resync_wins() {
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
            t_us: 0,
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
    fn the_button_lands_at_its_position_unless_something_newer_exists() {
        let mut c = client();
        c.on_stream(enter(0, 100.0));
        let click = |seq, x| StreamMsg::Button {
            button: Button::Left,
            state: KeyState::Pressed,
            seq,
            pos: Point::new(x, 100.0),
            t_us: 0,
        };
        // Datagram 1 was lost: the button carries the position.
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
        // Button with seq 2 after datagram 3: it does not move backwards.
        let out = c.on_stream(StreamMsg::Button {
            button: Button::Left,
            state: KeyState::Released,
            seq: 2,
            pos: Point::new(155.0, 100.0),
            t_us: 0,
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
    fn sync_corrects_the_position_without_going_back() {
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
    fn the_scroll_of_a_datagram_overtaken_by_the_stream_is_not_lost() {
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
        // One from before the Enter carries nothing from this visit.
        let old = MotionDatagram {
            scroll: Some(scroll),
            ..motion(4, 10.0, 0.0)
        };
        assert!(c.on_datagram(old).is_empty());
    }

    #[test]
    fn release_of_something_not_pressed_is_ignored_and_the_repeat_passes() {
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
    fn leave_releases_everything() {
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
    fn link_drop_with_a_pressed_key_releases_it() {
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
            Just(StreamMsg::Heartbeat(beat())),
            (usage, state.clone()).prop_map(|(usage, state)| StreamMsg::Key {
                usage,
                state,
                t_us: 0
            }),
            (button, state, 0u64..4).prop_map(|(button, state, seq)| StreamMsg::Button {
                button,
                state,
                seq,
                pos: Point::new(1.0, 1.0),
                t_us: 0,
            }),
        ]
    }

    proptest! {
        /// Whatever happens, when the link drops everything injected is left released: what
        /// was pressed on the system was released exactly once.
        #[test]
        fn nothing_is_left_stuck(msgs in prop::collection::vec(any_msg(), 0..64)) {
            let mut c = client();
            let mut system = Pressed::new();
            let inject = |events: Vec<InputEvent>, system: &mut Pressed| {
                for e in events {
                    if let InputEvent::Key { state: KeyState::Released, .. }
                    | InputEvent::Button { state: KeyState::Released, .. } = e {
                        // Something the system does not have pressed is never released.
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
