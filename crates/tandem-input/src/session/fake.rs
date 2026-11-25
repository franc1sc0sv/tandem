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
}

impl FakeCapture {
    pub fn new() -> Self {
        Self {
            calls: watch::Sender::new(Vec::new()),
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
    type Error = Infallible;

    fn set_grab(&mut self, grab: bool) -> Result<(), Infallible> {
        self.calls.send_modify(|c| c.push(CaptureCall::Grab(grab)));
        Ok(())
    }

    fn warp(&mut self, pos: Point) -> Result<(), Infallible> {
        self.calls.send_modify(|c| c.push(CaptureCall::Warp(pos)));
        Ok(())
    }
}

/// Simulated injection: stores everything injected, in order.
#[derive(Debug)]
pub struct FakeInjector {
    events: watch::Sender<Vec<InputEvent>>,
    focus: watch::Sender<Vec<bool>>,
}

impl FakeInjector {
    pub fn new() -> Self {
        Self {
            events: watch::Sender::new(Vec::new()),
            focus: watch::Sender::new(Vec::new()),
        }
    }

    pub fn events(&self) -> watch::Receiver<Vec<InputEvent>> {
        self.events.subscribe()
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

    fn set_focus(&mut self, here: bool) -> Result<(), Infallible> {
        self.focus.send_modify(|f| f.push(here));
        Ok(())
    }
}
