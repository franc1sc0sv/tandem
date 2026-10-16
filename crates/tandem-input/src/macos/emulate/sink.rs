//! Doble de prueba de [`MacSink`]: guarda lo publicado y mueve un cursor simulado.

use std::time::Duration;

use super::{EmulateError, MacEvent, MacSink};
use crate::geometry::{Point, Rect};

/// Sink que no toca el sistema. El cursor sigue a los eventos de mouse
/// publicados, como lo haría macOS.
#[derive(Debug, Clone)]
pub struct RecordingSink {
    pub events: Vec<MacEvent>,
    pub cursor: Point,
    pub screens: Vec<Rect>,
    pub double_click: Duration,
    pub natural_scroll: bool,
    /// Si es `Some`, `post` falla con este mensaje (sin guardar el evento).
    pub fail_post: Option<String>,
}

impl RecordingSink {
    /// Una pantalla de 1920×1080 con el cursor en el centro, doble clic de 0,5 s
    /// y sin scroll natural.
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            cursor: Point::new(960.0, 540.0),
            screens: vec![Rect::new(0, 0, 1920, 1080)],
            double_click: Duration::from_millis(500),
            natural_scroll: false,
            fail_post: None,
        }
    }

    /// Vacía y devuelve lo publicado.
    pub fn take(&mut self) -> Vec<MacEvent> {
        std::mem::take(&mut self.events)
    }
}

impl Default for RecordingSink {
    fn default() -> Self {
        Self::new()
    }
}

impl MacSink for RecordingSink {
    fn post(&mut self, event: &MacEvent) -> Result<(), EmulateError> {
        if let Some(msg) = &self.fail_post {
            return Err(EmulateError::Platform(msg.clone()));
        }
        if let MacEvent::Mouse { pos, .. } = event {
            self.cursor = *pos;
        }
        self.events.push(*event);
        Ok(())
    }

    fn cursor(&mut self) -> Result<Point, EmulateError> {
        Ok(self.cursor)
    }

    fn screens(&mut self) -> Result<Vec<Rect>, EmulateError> {
        Ok(self.screens.clone())
    }

    fn double_click_interval(&mut self) -> Duration {
        self.double_click
    }

    fn natural_scroll(&mut self) -> bool {
        self.natural_scroll
    }
}
