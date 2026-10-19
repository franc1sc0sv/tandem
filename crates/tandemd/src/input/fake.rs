//! Backends simulados del motor de entrada, para las pruebas con dos agentes: la
//! prueba "mueve el mouse" por [`FakeInputBackends::capture`], ve lo inyectado en
//! [`FakeInputBackends::injected`], copia en el portapapeles simulado y conecta o
//! desconecta el hub.

use std::sync::{Arc, Mutex, PoisonError};

use tandem_clipboard::fake::FakeClipboard;
use tandem_core::Config;
use tandem_input::session::{Capture, Injector};
use tandem_input::{InputEvent, Point, Screen};
use tandem_peripherals::fake::{FakeDevices, FakeScanner};
use tandem_proto::InputPermission;
use tokio::sync::watch;

use super::backends::{
    CaptureSink, DynCapture, DynClipboard, DynInjector, DynScanner, InputBackends,
};

/// Lo que se le pidió a la captura simulada.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CaptureCall {
    Grab(bool),
    Warp(Point),
}

#[derive(Default)]
struct Inner {
    sink: Option<CaptureSink>,
    screens: Vec<Screen>,
    missing: Vec<InputPermission>,
    /// Cuántas veces se pidieron los permisos al sistema.
    requested: u32,
    /// Lo que macOS concede al pedirlos: si `true`, ya no falta ninguno.
    grant_on_request: bool,
    fail_capture: Option<String>,
}

/// Se clona para tener otra vista de los mismos backends.
#[derive(Clone)]
pub struct FakeInputBackends {
    inner: Arc<Mutex<Inner>>,
    calls: Arc<watch::Sender<Vec<CaptureCall>>>,
    injected: Arc<watch::Sender<Vec<InputEvent>>>,
    clipboard: FakeClipboard,
    scanner: FakeScanner,
}

impl FakeInputBackends {
    /// Con estas pantallas y sin ningún dispositivo USB conectado.
    pub fn new(screens: Vec<Screen>) -> Self {
        FakeInputBackends {
            inner: Arc::new(Mutex::new(Inner {
                screens,
                ..Inner::default()
            })),
            calls: Arc::new(watch::Sender::new(Vec::new())),
            injected: Arc::new(watch::Sender::new(Vec::new())),
            clipboard: FakeClipboard::new(),
            scanner: FakeScanner::new(Vec::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// El usuario mueve el mouse o escribe en esta máquina. `false` si la captura no
    /// está corriendo (esta máquina nunca fue servidor) o ya nadie la escucha.
    pub fn capture(&self, event: InputEvent) -> bool {
        self.lock().sink.as_ref().is_some_and(|s| s.send(event))
    }

    /// Si la captura está corriendo.
    pub fn capturing(&self) -> bool {
        self.lock().sink.is_some()
    }

    pub fn capture_calls(&self) -> watch::Receiver<Vec<CaptureCall>> {
        self.calls.subscribe()
    }

    /// Todo lo inyectado en esta máquina, en orden.
    pub fn injected(&self) -> watch::Receiver<Vec<InputEvent>> {
        self.injected.subscribe()
    }

    pub fn clipboard(&self) -> &FakeClipboard {
        &self.clipboard
    }

    /// Los dispositivos USB conectados a esta máquina.
    pub fn devices(&self) -> FakeDevices {
        self.scanner.handle()
    }

    pub fn set_screens(&self, screens: Vec<Screen>) {
        self.lock().screens = screens;
    }

    pub fn set_missing(&self, missing: Vec<InputPermission>) {
        self.lock().missing = missing;
    }

    /// Cuántas veces el agente pidió los permisos al sistema.
    pub fn permission_requests(&self) -> u32 {
        self.lock().requested
    }

    /// Al pedir los permisos, el «usuario» los concede.
    pub fn grant_on_request(&self, grant: bool) {
        self.lock().grant_on_request = grant;
    }

    /// Mientras tenga un error, arrancar la captura falla con él (un permiso que falta).
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

struct RecordingInjector(Arc<watch::Sender<Vec<InputEvent>>>);

impl Injector for RecordingInjector {
    type Error = std::convert::Infallible;

    fn inject(&mut self, event: &InputEvent) -> Result<(), Self::Error> {
        self.0.send_modify(|e| e.push(*event));
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
        Ok(DynInjector::new(RecordingInjector(self.injected.clone())))
    }

    fn clipboard(&self) -> Result<DynClipboard, String> {
        Ok(DynClipboard::new(self.clipboard.clone()))
    }

    fn scanner(&self) -> Option<DynScanner> {
        Some(DynScanner::new(self.scanner.clone()))
    }

    fn screens(&self, _desk: &Config) -> Result<Vec<Screen>, String> {
        Ok(self.lock().screens.clone())
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
