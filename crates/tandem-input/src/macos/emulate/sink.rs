//! Test double of [`MacSink`]: stores what was published and moves a simulated cursor.

use std::time::Duration;

use super::{EmulateError, MacEvent, MacSink};
use crate::geometry::{Point, Rect};

/// Sink that does not touch the system. The cursor follows the published
/// mouse events, as macOS would.
#[derive(Debug, Clone)]
pub struct RecordingSink {
    pub events: Vec<MacEvent>,
    pub cursor: Point,
    pub screens: Vec<Rect>,
    pub double_click: Duration,
    pub natural_scroll: bool,
    /// If `Some`, `post` fails with this message (without storing the event).
    pub fail_post: Option<String>,
}

impl RecordingSink {
    /// A 1920×1080 screen with the cursor in the center, a 0.5 s double click
    /// and no natural scrolling.
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

    /// Empties and returns what was published.
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
