//! Lo que el motor de entrada necesita de esta máquina, detrás de un trait
//! ([`InputBackends`]): la plataforma real ([`PlatformInput`]) o los dobles de
//! [`super::fake`].
//!
//! Los tipos de cada backend son distintos en cada sistema; aquí todos se ven con el
//! error como texto ([`Stringly`]) y en caja, así el resto del agente es uno solo.

use std::sync::Arc;

use tandem_clipboard::{ClipContent, Clipboard};
use tandem_core::{Config, Machine};
use tandem_display::desktop::{self, DesktopControl};
use tandem_input::event::{Direction, ModifierRemap, RemapPolicy};
use tandem_input::session::{Capture, Injector};
use tandem_input::{InputEvent, Point, Screen};
use tandem_peripherals::{DeviceScanner, UsbId};
use tandem_proto::InputPermission;
use tokio::sync::mpsc;

use super::screens::from_placed;

/// Adapta el error de un backend a texto.
pub struct Stringly<T>(pub T);

impl<C: Capture> Capture for Stringly<C> {
    type Error = String;

    fn set_grab(&mut self, grab: bool) -> Result<(), String> {
        self.0.set_grab(grab).map_err(|e| e.to_string())
    }

    fn warp(&mut self, pos: Point) -> Result<(), String> {
        self.0.warp(pos).map_err(|e| e.to_string())
    }
}

impl<I: Injector> Injector for Stringly<I> {
    type Error = String;

    fn inject(&mut self, event: &InputEvent) -> Result<(), String> {
        self.0.inject(event).map_err(|e| e.to_string())
    }

    fn refresh(&mut self) -> Result<(), String> {
        self.0.refresh().map_err(|e| e.to_string())
    }
}

impl<C: Clipboard> Clipboard for Stringly<C> {
    type Error = String;

    fn change_count(&mut self) -> Result<u64, String> {
        self.0.change_count().map_err(|e| e.to_string())
    }

    fn read(&mut self) -> Result<Option<ClipContent>, String> {
        self.0.read().map_err(|e| e.to_string())
    }

    fn write(&mut self, content: &ClipContent) -> Result<(), String> {
        self.0.write(content).map_err(|e| e.to_string())
    }
}

impl<S: DeviceScanner> DeviceScanner for Stringly<S> {
    type Error = String;

    fn scan(&mut self) -> Result<Vec<UsbId>, String> {
        self.0.scan().map_err(|e| e.to_string())
    }
}

/// Captura de cualquier plataforma.
pub struct DynCapture(pub Box<dyn Capture<Error = String>>);

impl DynCapture {
    pub fn new(c: impl Capture + 'static) -> Self {
        DynCapture(Box::new(Stringly(c)))
    }
}

impl Capture for DynCapture {
    type Error = String;

    fn set_grab(&mut self, grab: bool) -> Result<(), String> {
        self.0.set_grab(grab)
    }

    fn warp(&mut self, pos: Point) -> Result<(), String> {
        self.0.warp(pos)
    }
}

/// Inyección de cualquier plataforma.
pub struct DynInjector(pub Box<dyn Injector<Error = String>>);

impl DynInjector {
    pub fn new(i: impl Injector + 'static) -> Self {
        DynInjector(Box::new(Stringly(i)))
    }
}

impl Injector for DynInjector {
    type Error = String;

    fn inject(&mut self, event: &InputEvent) -> Result<(), String> {
        self.0.inject(event)
    }

    fn refresh(&mut self) -> Result<(), String> {
        self.0.refresh()
    }
}

/// Portapapeles de cualquier plataforma.
pub struct DynClipboard(pub Box<dyn Clipboard<Error = String>>);

impl DynClipboard {
    pub fn new(c: impl Clipboard + 'static) -> Self {
        DynClipboard(Box::new(Stringly(c)))
    }
}

impl Clipboard for DynClipboard {
    type Error = String;

    fn change_count(&mut self) -> Result<u64, String> {
        self.0.change_count()
    }

    fn read(&mut self) -> Result<Option<ClipContent>, String> {
        self.0.read()
    }

    fn write(&mut self, content: &ClipContent) -> Result<(), String> {
        self.0.write(content)
    }
}

/// Escáner USB de cualquier plataforma.
pub struct DynScanner(pub Box<dyn DeviceScanner<Error = String>>);

impl DynScanner {
    pub fn new(s: impl DeviceScanner + 'static) -> Self {
        DynScanner(Box::new(Stringly(s)))
    }
}

impl DeviceScanner for DynScanner {
    type Error = String;

    fn scan(&mut self) -> Result<Vec<UsbId>, String> {
        self.0.scan()
    }
}

/// Por donde la captura entrega lo capturado. Reasigna los modificadores antes de
/// mandarlos (DESIGN §7: el servidor reasigna, el cliente inyecta tal cual), así lo
/// que el servidor recuerda presionado ya está en la forma del otro sistema.
#[derive(Clone)]
pub struct CaptureSink {
    tx: mpsc::UnboundedSender<InputEvent>,
    remap: ModifierRemap,
}

impl CaptureSink {
    /// La dirección es la de la máquina que captura hacia la otra.
    pub fn new(tx: mpsc::UnboundedSender<InputEvent>, policy: &RemapPolicy, me: Machine) -> Self {
        let direction = match me {
            Machine::Mac => Direction::MacToWin,
            Machine::Pc => Direction::WinToMac,
        };
        CaptureSink {
            tx,
            remap: *policy.for_direction(direction),
        }
    }

    /// `false` si ya nadie recibe lo capturado: la captura tiene que soltar la entrada.
    pub fn send(&self, event: InputEvent) -> bool {
        self.tx.send(self.remap.apply(event)).is_ok()
    }
}

/// Lo que cada máquina aporta al teclado, el mouse y el portapapeles compartidos.
/// Las llamadas pueden bloquear (crear un tap, leer la topología): el agente las hace
/// fuera del runtime.
pub trait InputBackends: Send + Sync + 'static {
    /// Arranca la captura; lo capturado va a `sink`.
    fn capture(&self, sink: CaptureSink) -> Result<DynCapture, String>;

    fn injector(&self) -> Result<DynInjector, String>;

    fn clipboard(&self) -> Result<DynClipboard, String>;

    /// `None` si este sistema no sabe listar sus dispositivos USB.
    fn scanner(&self) -> Option<DynScanner>;

    /// Las pantallas del escritorio, en las coordenadas nativas del cursor.
    fn screens(&self, desk: &Config) -> Result<Vec<Screen>, String>;

    /// Permisos del sistema que faltan para capturar o inyectar.
    fn missing_permissions(&self) -> Vec<InputPermission>;
}

/// Los backends reales de este sistema: CGEventTap/CGEventPost en macOS, hooks y
/// `SendInput` en Windows, `arboard` para el portapapeles, IOKit/SetupAPI para el hub y
/// la topología de `tandem-display` para las pantallas. En otros sistemas no hay
/// captura ni inyección, y el agente lo informa.
pub struct PlatformInput {
    desktop: Option<Arc<dyn DesktopControl>>,
}

impl PlatformInput {
    pub fn new(me: Machine) -> Self {
        PlatformInput {
            desktop: desktop::native(me),
        }
    }
}

impl InputBackends for PlatformInput {
    fn capture(&self, sink: CaptureSink) -> Result<DynCapture, String> {
        platform::capture(sink)
    }

    fn injector(&self) -> Result<DynInjector, String> {
        platform::injector()
    }

    fn clipboard(&self) -> Result<DynClipboard, String> {
        tandem_clipboard::SystemClipboard::new()
            .map(DynClipboard::new)
            .map_err(|e| e.to_string())
    }

    fn scanner(&self) -> Option<DynScanner> {
        platform::scanner()
    }

    fn screens(&self, desk: &Config) -> Result<Vec<Screen>, String> {
        let desktop = self
            .desktop
            .as_ref()
            .ok_or("este sistema no informa sus pantallas")?;
        let placed = desktop.screens(&desk.monitors).map_err(|e| e.0)?;
        Ok(placed.into_iter().filter_map(from_placed).collect())
    }

    fn missing_permissions(&self) -> Vec<InputPermission> {
        platform::missing_permissions()
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use tandem_input::macos::capture::{MacCapture, Permission, permissions};
    use tandem_input::macos::emulate::{CgSink, MacEmulator};
    use tandem_proto::InputPermission;

    use super::{CaptureSink, DynCapture, DynInjector, DynScanner};

    pub fn capture(sink: CaptureSink) -> Result<DynCapture, String> {
        MacCapture::start(move |ev| sink.send(ev))
            .map(DynCapture::new)
            .map_err(|e| e.to_string())
    }

    pub fn injector() -> Result<DynInjector, String> {
        let sink = CgSink::new().map_err(|e| e.to_string())?;
        MacEmulator::new(sink)
            .map(DynInjector::new)
            .map_err(|e| e.to_string())
    }

    pub fn scanner() -> Option<DynScanner> {
        Some(DynScanner::new(tandem_peripherals::SystemScanner::new()))
    }

    pub fn missing_permissions() -> Vec<InputPermission> {
        permissions()
            .missing()
            .into_iter()
            .map(|p| match p {
                Permission::Accessibility => InputPermission::Accessibility,
                Permission::InputMonitoring => InputPermission::InputMonitoring,
            })
            .collect()
    }
}

#[cfg(windows)]
mod platform {
    use tandem_input::windows::capture::{CaptureConfig, WindowsCapture};
    use tandem_input::windows::emulate::{Emulator, SendInputSink};
    use tandem_proto::InputPermission;

    use super::{CaptureSink, DynCapture, DynInjector, DynScanner};

    pub fn capture(sink: CaptureSink) -> Result<DynCapture, String> {
        WindowsCapture::start(CaptureConfig::default(), move |ev| {
            // Sin receptor el agente ya está cerrando y suelta la captura.
            let _ = sink.send(ev);
        })
        .map(DynCapture::new)
        .map_err(|e| e.to_string())
    }

    pub fn injector() -> Result<DynInjector, String> {
        let sink = SendInputSink::new().map_err(|e| e.to_string())?;
        Emulator::new(sink)
            .map(DynInjector::new)
            .map_err(|e| e.to_string())
    }

    pub fn scanner() -> Option<DynScanner> {
        Some(DynScanner::new(tandem_peripherals::SystemScanner::new()))
    }

    pub fn missing_permissions() -> Vec<InputPermission> {
        Vec::new()
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use tandem_proto::InputPermission;

    use super::{CaptureSink, DynCapture, DynInjector, DynScanner};

    const UNSUPPORTED: &str = "este sistema no tiene captura ni inyección de teclado y mouse";

    pub fn capture(_sink: CaptureSink) -> Result<DynCapture, String> {
        Err(UNSUPPORTED.to_owned())
    }

    pub fn injector() -> Result<DynInjector, String> {
        Err(UNSUPPORTED.to_owned())
    }

    pub fn scanner() -> Option<DynScanner> {
        None
    }

    pub fn missing_permissions() -> Vec<InputPermission> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use tandem_input::{HidUsage, KeyState, ModifierKey};

    use super::*;

    fn key(m: ModifierKey) -> InputEvent {
        InputEvent::Key {
            usage: m.usage(),
            state: KeyState::Pressed,
        }
    }

    #[test]
    fn the_sink_remaps_in_the_direction_of_the_capture() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let policy = RemapPolicy::default();
        let mac = CaptureSink::new(tx.clone(), &policy, Machine::Mac);
        assert!(mac.send(key(ModifierKey::MetaLeft)));
        assert_eq!(
            rx.try_recv().unwrap(),
            key(ModifierKey::ControlLeft),
            "Cmd→Ctrl"
        );
        let pc = CaptureSink::new(tx, &policy, Machine::Pc);
        assert!(pc.send(key(ModifierKey::ControlLeft)));
        assert_eq!(
            rx.try_recv().unwrap(),
            key(ModifierKey::MetaLeft),
            "Ctrl→Cmd"
        );
        let a = InputEvent::Key {
            usage: HidUsage::key(0x04),
            state: KeyState::Pressed,
        };
        assert!(pc.send(a));
        assert_eq!(rx.try_recv().unwrap(), a);
        drop(rx);
        assert!(!pc.send(a), "sin receptor la captura tiene que soltar");
    }

    #[test]
    fn this_system_says_what_it_lacks() {
        let p = PlatformInput::new(Machine::Pc);
        if cfg!(not(any(windows, target_os = "macos"))) {
            assert!(p.injector().is_err());
            let (tx, _rx) = mpsc::unbounded_channel();
            assert!(
                p.capture(CaptureSink::new(tx, &RemapPolicy::default(), Machine::Pc))
                    .is_err()
            );
            assert!(p.scanner().is_none());
            assert!(
                p.screens(&Config::from_toml("monitors = []").unwrap())
                    .is_err()
            );
        }
        if cfg!(not(target_os = "macos")) {
            assert!(p.missing_permissions().is_empty());
        }
    }
}
