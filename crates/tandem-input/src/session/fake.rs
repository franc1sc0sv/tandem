//! Dobles de prueba de [`Capture`] e [`Injector`]: registran lo que se les pide
//! y dejan esperar a que llegue algo.

use std::convert::Infallible;

use tokio::sync::watch;

use super::{Capture, Injector};
use crate::event::InputEvent;
use crate::geometry::Point;

/// Lo que se le pidió a la captura.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CaptureCall {
    Grab(bool),
    Warp(Point),
}

/// Captura simulada. Los eventos "capturados" los manda la prueba por el canal
/// del driver; esto solo registra los pedidos de control.
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

    /// Si la entrada local está tragada ahora.
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

/// Inyección simulada: guarda todo lo inyectado, en orden.
#[derive(Debug)]
pub struct FakeInjector {
    events: watch::Sender<Vec<InputEvent>>,
}

impl FakeInjector {
    pub fn new() -> Self {
        Self {
            events: watch::Sender::new(Vec::new()),
        }
    }

    pub fn events(&self) -> watch::Receiver<Vec<InputEvent>> {
        self.events.subscribe()
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
}
