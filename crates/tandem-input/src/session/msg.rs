//! What travels between server and client of the input session.
//!
//! Two channels over the same connection (docs/research/input-engine.md):
//! - [`StreamMsg`], over a reliable ordered stream: enter, leave, keys,
//!   buttons and the heartbeat;
//! - [`Datagram`], unreliable: coalesced movement and scroll.
//!
//! Sequence numbers tie the two together: every datagram carries an increasing one, and
//! stream messages that depend on position (enter, buttons, the
//! exact position on stopping) carry the last one sent. The client discards any datagram that is not newer
//! than the last one it applied, so a late one never moves the cursor backwards.
//!
//! The `t_us` stamps are from the server's monotonic clock ([`super::latency::now_us`])
//! at capture: the client uses them to measure latency (T-093).

use serde::{Deserialize, Serialize};
use tandem_proto::ProtoError;

use tandem_proto::InputLatency;

use super::latency::Beat;
use crate::event::{Button, HidUsage, KeyState, Modifiers, Scroll};
use crate::geometry::Point;

/// Messages of the reliable input stream. Positions go in the
/// client cursor's native coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum StreamMsg {
    /// The cursor enters the client's screen at `pos`, with these modifiers
    /// pressed (the user may cross with Shift held). `seq` is the last
    /// datagram sent: earlier ones are no longer valid.
    Enter {
        seq: u64,
        pos: Point,
        modifiers: Modifiers,
    },
    /// The cursor goes back to the server: the client releases everything.
    Leave,
    Key {
        usage: HidUsage,
        state: KeyState,
        t_us: u64,
    },
    /// A click at `pos`: if the datagram with the last movement was lost or arrives
    /// later, the click still lands where the user saw it.
    Button {
        button: Button,
        state: KeyState,
        seq: u64,
        pos: Point,
        t_us: u64,
    },
    /// Exact position when the mouse stops, so that the client's cursor
    /// ends up where it should even if datagrams were lost.
    Sync { seq: u64, pos: Point },
    /// Heartbeat, in both directions: prolonged silence counts as a dropped link. With
    /// its stamp and the echo of the last one from the other side they estimate the clock offset.
    Heartbeat(Beat),
    /// From the client to the server: the latency it measured in the last window.
    Latency(InputLatency),
}

/// Datagrams of the connection between agents. It is an enum so that other uses
/// can be added without being confused with the input ones.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Datagram {
    Motion(MotionDatagram),
}

/// Movement and scroll accumulated since the previous datagram.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MotionDatagram {
    pub seq: u64,
    /// Delta in the client's native units, already converted by the server with the
    /// scale of each screen it crossed: the client just adds it, so its cursor ends up
    /// exactly where the server believes even if the stretch passes through screens of
    /// different scales.
    pub dx: f64,
    pub dy: f64,
    pub scroll: Option<Scroll>,
    /// Absolute resynchronization: where the cursor has to end up after this
    /// datagram (the client's native coordinates). It corrects whatever was lost.
    pub resync: Option<Point>,
    /// Stamp of the first event it accumulates (the oldest).
    pub t_us: u64,
}

impl Datagram {
    pub fn encode(&self) -> Result<Vec<u8>, ProtoError> {
        tandem_proto::encode(self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ProtoError> {
        tandem_proto::decode(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ModifierKey;

    #[test]
    fn messages_round_trip() {
        let msgs = [
            StreamMsg::Enter {
                seq: 9,
                pos: Point::new(3440.0, 540.5),
                modifiers: Modifiers::from_iter([ModifierKey::ShiftLeft]),
            },
            StreamMsg::Leave,
            StreamMsg::Key {
                usage: HidUsage::key(0x04),
                state: KeyState::Pressed,
                t_us: 17,
            },
            StreamMsg::Button {
                button: Button::Back,
                state: KeyState::Released,
                seq: u64::MAX,
                pos: Point::new(-10.0, 0.25),
                t_us: u64::MAX,
            },
            StreamMsg::Sync {
                seq: 3,
                pos: Point::new(1.0, 2.0),
            },
            StreamMsg::Heartbeat(Beat {
                sent_us: 1,
                echo: None,
            }),
            StreamMsg::Heartbeat(Beat {
                sent_us: 9_000_000,
                echo: Some(super::super::latency::Echo {
                    sent_us: 8_000_000,
                    held_us: 999_000,
                }),
            }),
            StreamMsg::Latency(InputLatency {
                link: None,
                count: 1000,
                p50_us: 400,
                p99_us: 900,
                max_us: 3000,
            }),
        ];
        for m in msgs {
            let bytes = tandem_proto::encode(&m).unwrap();
            assert_eq!(tandem_proto::decode::<StreamMsg>(&bytes).unwrap(), m);
        }
    }

    #[test]
    fn datagram_round_trip_and_compact() {
        let d = Datagram::Motion(MotionDatagram {
            seq: 1_000_000,
            dx: 1.5,
            dy: -2.0,
            scroll: Some(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: -240,
            }),
            resync: Some(Point::new(100.0, 200.0)),
            t_us: 3_600_000_000,
        });
        let bytes = d.encode().unwrap();
        // It has to fit comfortably in any MTU.
        assert!(bytes.len() < 64, "{} bytes", bytes.len());
        assert_eq!(Datagram::decode(&bytes).unwrap(), d);
        assert!(Datagram::decode(&[0xFF]).is_err());
    }
}
