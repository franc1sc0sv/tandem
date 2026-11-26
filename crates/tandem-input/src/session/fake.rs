//! Test doubles of [`Capture`] and [`Injector`]: they record what they are asked
//! and let a test wait for something to arrive.

use std::convert::Infallible;

use tokio::sync::watch;

use super::{Capture, Injector};
use crate::event::InputEvent;
use crate::geometry::Point;

/// What the capture was asked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CaptureCall {
    Grab(bool),
    Warp(Point),
}

/// Simulated capture. The "captured" events are sent by the test over the driver's
/// channel; this only records the control requests.
#[derive(Debug)]
pub struct FakeCapture {
    calls: watch::Sender<Vec<CaptureCall>>,
    refuse_grab: bool,
}

/// The double refused to take input (Secure Event Input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrabRefused;

impl std::fmt::Display for GrabRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("grab refused")
    }
}

impl FakeCapture {
    pub fn new() -> Self {
        Self {
            calls: watch::Sender::new(Vec::new()),
            refuse_grab: false,
        }
    }

    /// A capture that refuses to take input, like macOS with a password field active.
    pub fn refusing_grab() -> Self {
        Self {
            refuse_grab: true,
            ..Self::new()
        }
    }

    pub fn calls(&self) -> watch::Receiver<Vec<CaptureCall>> {
        self.calls.subscribe()
    }

    /// Whether local input is currently swallowed.
    pub fn grabbed(&self) -> bool {
        self.calls
            .borrow()
            .iter()
            .rev()
            .find_map(|c| match c {
                CaptureCall::Grab(g) => Some(*g),
                CaptureCall::Warp(_) => None,
            })
            .unwrap_or(false)
    }
}

impl Default for FakeCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl Capture for FakeCapture {
    type Error = GrabRefused;

    fn set_grab(&mut self, grab: bool) -> Result<(), GrabRefused> {
        self.calls.send_modify(|c| c.push(CaptureCall::Grab(grab)));
        if grab && self.refuse_grab {
            return Err(GrabRefused);
        }
        Ok(())
    }

    fn warp(&mut self, pos: Point) -> Result<(), GrabRefused> {
        self.calls.send_modify(|c| c.push(CaptureCall::Warp(pos)));
        Ok(())
    }
}

/// Simulated injection: stores everything injected, in order.
#[derive(Debug)]
pub struct FakeInjector {
    events: watch::Sender<Vec<InputEvent>>,
    focus: watch::Sender<Vec<bool>>,
    refreshes: watch::Sender<u32>,
}

impl FakeInjector {
    pub fn new() -> Self {
        Self {
            events: watch::Sender::new(Vec::new()),
            focus: watch::Sender::new(Vec::new()),
            refreshes: watch::Sender::new(0),
        }
    }

    pub fn events(&self) -> watch::Receiver<Vec<InputEvent>> {
        self.events.subscribe()
    }

    /// How many times the screens were read again ([`Injector::refresh`]).
    pub fn refreshes(&self) -> watch::Receiver<u32> {
        self.refreshes.subscribe()
    }

    /// Each [`Injector::set_focus`], in order.
    pub fn focus(&self) -> watch::Receiver<Vec<bool>> {
        self.focus.subscribe()
    }
}

impl Default for FakeInjector {
    fn default() -> Self {
        Self::new()
    }
}

impl Injector for FakeInjector {
    type Error = Infallible;

    fn inject(&mut self, event: &InputEvent) -> Result<(), Infallible> {
        self.events.send_modify(|e| e.push(*event));
        Ok(())
    }

    fn refresh(&mut self) -> Result<(), Infallible> {
        self.refreshes.send_modify(|n| *n += 1);
        Ok(())
    }

    fn set_focus(&mut self, here: bool) -> Result<(), Infallible> {
        self.focus.send_modify(|f| f.push(here));
        Ok(())
    }
}
