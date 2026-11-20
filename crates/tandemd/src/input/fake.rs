//! Simulated input engine backends, for tests with two agents: the
//! test "moves the mouse" through [`FakeInputBackends::capture`], sees what was injected in
//! [`FakeInputBackends::injected`], copies to the simulated clipboard and connects or
//! disconnects the hub.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use tandem_clipboard::fake::FakeClipboard;
use tandem_core::DisplayKey;
use tandem_input::session::{Capture, Injector};
use tandem_input::{InputEvent, Point, Screen};
use tandem_peripherals::fake::{FakeDevices, FakeScanner};
use tandem_proto::InputPermission;
use tokio::sync::watch;

use super::backends::{
    CaptureSink, DynCapture, DynClipboard, DynInjector, DynScanner, InputBackends,
};

/// What the simulated capture was asked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CaptureCall {
    Grab(bool),
    Warp(Point),
}

#[derive(Default)]
struct Inner {
    sink: Option<CaptureSink>,
    screens: Vec<Screen>,
    /// The EDID identity of each screen, by its id (T-340).
    screen_keys: Vec<(String, DisplayKey)>,
    missing: Vec<InputPermission>,
    /// How many times the permissions were requested from the system.
    requested: u32,
    /// What macOS grants when they are requested: if `true`, none is missing anymore.
    grant_on_request: bool,
    fail_capture: Option<String>,
}

/// It is cloned to have another view of the same backends.
#[derive(Clone)]
pub struct FakeInputBackends {
    inner: Arc<Mutex<Inner>>,
    calls: Arc<watch::Sender<Vec<CaptureCall>>>,
    injected: Arc<watch::Sender<Vec<InputEvent>>>,
    /// What was injected with the moment it reached the injector (the latency bench).
    timeline: Arc<Mutex<Vec<(Instant, InputEvent)>>>,
    clipboard: FakeClipboard,
    scanner: FakeScanner,
}

impl FakeInputBackends {
    /// With these screens and without any USB device connected.
    pub fn new(screens: Vec<Screen>) -> Self {
        FakeInputBackends {
            inner: Arc::new(Mutex::new(Inner {
                screens,
                ..Inner::default()
            })),
            calls: Arc::new(watch::Sender::new(Vec::new())),
            injected: Arc::new(watch::Sender::new(Vec::new())),
            timeline: Arc::new(Mutex::new(Vec::new())),
            clipboard: FakeClipboard::new(),
            scanner: FakeScanner::new(Vec::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The user moves the mouse or types on this machine. `false` if the capture is not
    /// running (this machine was never a server) or nobody listens to it anymore.
    pub fn capture(&self, event: InputEvent) -> bool {
        self.lock().sink.as_ref().is_some_and(|s| s.send(event))
    }

    /// The capture dies (the hook's thread, the tap): drops the sink the backend held, so the
    /// engine sees the channel close if nobody else keeps it open.
    pub fn close_capture(&self) {
        self.lock().sink = None;
    }

    /// Whether the capture is running.
    pub fn capturing(&self) -> bool {
        self.lock().sink.is_some()
    }

    pub fn capture_calls(&self) -> watch::Receiver<Vec<CaptureCall>> {
        self.calls.subscribe()
    }

    /// Everything injected on this machine, in order.
    pub fn injected(&self) -> watch::Receiver<Vec<InputEvent>> {
        self.injected.subscribe()
    }

    /// Everything injected with the instant at which the injector received it.
    pub fn timeline(&self) -> Vec<(Instant, InputEvent)> {
        self.timeline
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn clipboard(&self) -> &FakeClipboard {
        &self.clipboard
    }

    /// The USB devices connected to this machine.
    pub fn devices(&self) -> FakeDevices {
        self.scanner.handle()
    }

    pub fn set_screens(&self, screens: Vec<Screen>) {
        self.lock().screens = screens;
    }

    pub fn set_screen_keys(&self, keys: Vec<(String, DisplayKey)>) {
        self.lock().screen_keys = keys;
    }

    pub fn set_missing(&self, missing: Vec<InputPermission>) {
        self.lock().missing = missing;
    }

    /// How many times the agent asked the system for the permissions.
    pub fn permission_requests(&self) -> u32 {
        self.lock().requested
    }

    /// When asking for the permissions, the "user" grants them.
    pub fn grant_on_request(&self, grant: bool) {
        self.lock().grant_on_request = grant;
    }

    /// While it has an error, starting the capture fails with it.
    pub fn fail_capture(&self, error: Option<String>) {
        self.lock().fail_capture = error;
    }
}

struct RecordingCapture(Arc<watch::Sender<Vec<CaptureCall>>>);

impl Capture for RecordingCapture {
    type Error = std::convert::Infallible;

    fn set_grab(&mut self, grab: bool) -> Result<(), Self::Error> {
        self.0.send_modify(|c| c.push(CaptureCall::Grab(grab)));
        Ok(())
    }

    fn warp(&mut self, pos: Point) -> Result<(), Self::Error> {
        self.0.send_modify(|c| c.push(CaptureCall::Warp(pos)));
        Ok(())
    }
}

struct RecordingInjector {
    injected: Arc<watch::Sender<Vec<InputEvent>>>,
    timeline: Arc<Mutex<Vec<(Instant, InputEvent)>>>,
}

impl Injector for RecordingInjector {
    type Error = std::convert::Infallible;

    fn inject(&mut self, event: &InputEvent) -> Result<(), Self::Error> {
        let at = Instant::now();
        self.timeline
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((at, *event));
        self.injected.send_modify(|e| e.push(*event));
        Ok(())
    }
}

impl InputBackends for FakeInputBackends {
    fn capture(&self, sink: CaptureSink) -> Result<DynCapture, String> {
        let mut inner = self.lock();
        if let Some(e) = inner.fail_capture.clone() {
            return Err(e);
        }
        inner.sink = Some(sink);
        Ok(DynCapture::new(RecordingCapture(self.calls.clone())))
    }

    fn injector(&self) -> Result<DynInjector, String> {
        Ok(DynInjector::new(RecordingInjector {
            injected: self.injected.clone(),
            timeline: self.timeline.clone(),
        }))
    }

    fn clipboard(&self) -> Result<DynClipboard, String> {
        Ok(DynClipboard::new(self.clipboard.clone()))
    }

    fn scanner(&self) -> Option<DynScanner> {
        Some(DynScanner::new(self.scanner.clone()))
    }

    fn screens(&self) -> Result<Vec<Screen>, String> {
        Ok(self.lock().screens.clone())
    }

    fn screen_keys(&self) -> Result<Vec<(String, DisplayKey)>, String> {
        Ok(self.lock().screen_keys.clone())
    }

    fn missing_permissions(&self) -> Vec<InputPermission> {
        self.lock().missing.clone()
    }

    fn request_permissions(&self) -> Vec<InputPermission> {
        let mut inner = self.lock();
        inner.requested += 1;
        if inner.grant_on_request {
            inner.missing.clear();
        }
        inner.missing.clone()
    }
}
