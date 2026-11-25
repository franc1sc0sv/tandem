//! Input session over the link (DESIGN §7, docs/research/input-engine.md).
//!
//! The **server** is the machine with the keyboard and mouse: it captures, decides with the
//! geometry when the cursor crosses and sends the input to the **client**, which
//! injects it. Movement and scroll go in coalesced datagrams with a sequence
//! number and absolute resynchronization; keys, buttons, entering and leaving go over
//! a reliable ordered stream ([`msg`]).
//!
//! - [`ServerSession`] and [`ClientSession`]: state machines without I/O, tested
//!   event by event.
//! - [`Capture`] and [`Injector`]: what each platform implements (T-022 to T-025);
//!   [`fake`] has the test doubles.
//! - [`ServerDriver`] and [`ClientDriver`]: connect them to a QUIC connection of
//!   `tandem-link`, with a 1 s heartbeat. When the link drops or changes, the driver
//!   ends: the server gives the focus back to its machine and the client releases everything
//!   it had pressed.
//!
//! Positions are native cursor coordinates of each machine (those of their
//! [`Screen`](crate::geometry::Screen)s); the deltas that travel, logical points.

mod client;
pub mod cursor;
pub mod fake;
pub mod follow;
pub mod latency;
pub mod msg;
mod platform;
#[cfg(test)]
mod props;
mod quic;
mod server;
#[cfg(test)]
mod testutil;
mod threaded;

use std::fmt::Display;
use std::time::Duration;

pub use client::ClientSession;
pub use cursor::{Crossing, CursorEvent, CursorFeed, CursorGate, CursorTap};
pub use follow::{Follow, FollowConfig};
pub use latency::Captured;
pub use msg::{Datagram, MotionDatagram, StreamMsg};
pub use quic::{ClientDriver, ServerDriver, SessionEnd};
pub use server::{ServerOutput, ServerSession};
pub use threaded::{Job, Mailbox, ThreadedInjector};

use crate::event::InputEvent;
use crate::geometry::Point;

/// Control of the server's capture. Captured events do not pass through here:
/// the capture runs on its own thread (CGEventTap, Windows hooks) and sends them
/// over a channel to the driver.
///
/// The movements it captures are `MotionRelative` in local native units,
/// without acceleration; it may send `MotionAbsolute` with the real position of the local
/// cursor to correct tracking while the focus is local.
pub trait Capture: Send {
    type Error: Display;

    /// `true`: swallow local input and freeze and hide the cursor, because the
    /// focus is on the other machine. `false`: give it back.
    fn set_grab(&mut self, grab: bool) -> Result<(), Self::Error>;

    /// Put the local cursor at `pos` (when coming back from the other machine).
    fn warp(&mut self, pos: Point) -> Result<(), Self::Error>;
}

/// Injection on the client. It receives keys and buttons, `MotionAbsolute` in
/// local native coordinates and scroll.
pub trait Injector: Send {
    type Error: Display;

    fn inject(&mut self, event: &InputEvent) -> Result<(), Self::Error>;

    /// This machine's screens changed: read them again.
    fn refresh(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    /// The cursor arrives at this machine (`true`, the Enter) or leaves it (the Leave, or
    /// the session ends). Windows uses it to draw the cursor when it has no mouse.
    fn set_focus(&mut self, _here: bool) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Session timings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionConfig {
    /// How often each side sends a heartbeat (DESIGN §6 and research: ~1 s).
    pub heartbeat_interval: Duration,
    /// Silence from the other side that counts as a dropped link. It is the last resort:
    /// normally the link manager notifies first and the driver stops.
    pub heartbeat_timeout: Duration,
    /// How long the mouse has to be still to send the absolute position.
    pub resync_idle: Duration,
    /// How many datagrams apart one goes with the absolute position even if the mouse
    /// does not stop.
    pub resync_every: u32,
    /// Window of the latency measurement: how often the client publishes p50/p99/max,
    /// records it and sends it to the server (T-093: 5 s).
    pub latency_every: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(1),
            heartbeat_timeout: Duration::from_secs(3),
            resync_idle: Duration::from_millis(50),
            resync_every: 16,
            latency_every: Duration::from_secs(5),
        }
    }
}
