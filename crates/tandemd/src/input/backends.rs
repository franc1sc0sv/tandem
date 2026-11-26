//! What the input engine needs from this machine, behind a trait
//! ([`InputBackends`]): the real platform ([`PlatformInput`]) or the doubles of
//! [`super::fake`].
//!
//! Each backend's types differ on each system; here all are seen with the
//! error as text ([`Stringly`]) and boxed, so the rest of the agent is just one.

use std::sync::Arc;

use std::sync::Mutex;
use tandem_clipboard::{ClipContent, Clipboard};
use tandem_core::{DisplayKey, Os};

use tandem_input::event::{Direction, ModifierRemap, RemapPolicy, ShortcutTranslator};
use tandem_input::session::{Capture, Captured, Injector};
use tandem_input::{HidUsage, InputEvent, Point, Screen};
use tandem_peripherals::{DeviceScanner, UsbId};
use tandem_proto::InputPermission;
use tokio::sync::mpsc;

use super::screens::{detected, detected_keys};

/// Adapts a backend's error to text.
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

    fn set_focus(&mut self, here: bool) -> Result<(), String> {
        self.0.set_focus(here).map_err(|e| e.to_string())
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

/// Capture of any platform.
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

/// Injection of any platform.
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

    fn set_focus(&mut self, here: bool) -> Result<(), String> {
        self.0.set_focus(here)
    }
}

/// Clipboard of any platform.
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

/// USB scanner of any platform.
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

/// Where the capture delivers what it captured. It remaps the modifiers before
/// sending them (DESIGN §7: the server remaps, the client injects as is), so what
/// the server remembers as pressed is already in the other system's form.
#[derive(Clone)]
pub struct CaptureSink {
    tx: mpsc::UnboundedSender<Captured>,
    remap: ModifierRemap,
    direction: Option<Direction>,
    policy: RemapPolicy,
    /// The scene shortcuts look at the physical keys, before remapping them.
    scenes: Option<crate::agent::SceneShortcuts>,
    /// Chords translated by intent (Ctrl+Left → Option+Left). Shared by the clones of the
    /// sink: it is one keyboard.
    shortcuts: Option<Arc<Mutex<ShortcutTranslator>>>,
}

impl std::fmt::Debug for CaptureSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureSink")
            .field("remap", &self.remap)
            .finish_non_exhaustive()
    }
}

/// A [`CaptureSink`] that does not keep the capture channel open: whoever holds it (the agent,
/// for `UiCommand::SyntheticMotion`) must not stop the channel from closing when the real
/// capture dies, or the engine would never notice and rebuild it.
#[derive(Clone)]
pub struct WeakCaptureSink {
    tx: mpsc::WeakUnboundedSender<Captured>,
    remap: ModifierRemap,
    direction: Option<Direction>,
    policy: RemapPolicy,
    scenes: Option<crate::agent::SceneShortcuts>,
    shortcuts: Option<Arc<Mutex<ShortcutTranslator>>>,
}

impl std::fmt::Debug for WeakCaptureSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WeakCaptureSink").finish_non_exhaustive()
    }
}

impl WeakCaptureSink {
    /// `None` once every strong sink is gone (the capture stopped or died).
    pub fn upgrade(&self) -> Option<CaptureSink> {
        Some(CaptureSink {
            tx: self.tx.upgrade()?,
            remap: self.remap,
            direction: self.direction,
            policy: self.policy,
            scenes: self.scenes.clone(),
            shortcuts: self.shortcuts.clone(),
        })
    }
}

/// The modifier remapping between two systems: Cmd<->Ctrl between macOS and Windows;
/// between two identical ones (or without yet knowing the other machine's), none.
pub fn direction(from: Os, to: Option<Os>) -> Option<Direction> {
    match (from, to?) {
        (Os::MacOs, Os::Windows) => Some(Direction::MacToWin),
        (Os::Windows, Os::MacOs) => Some(Direction::WinToMac),
        (Os::MacOs, Os::MacOs) | (Os::Windows, Os::Windows) => None,
    }
}

impl CaptureSink {
    /// The direction is that of the capturing system (`from`) toward that of the other machine
    /// (`to`, if already known).
    pub fn new(
        tx: mpsc::UnboundedSender<Captured>,
        policy: &RemapPolicy,
        from: Os,
        to: Option<Os>,
    ) -> Self {
        let direction = direction(from, to);
        let remap = direction
            .map(|d| *policy.for_direction(d))
            .unwrap_or_default();
        CaptureSink {
            tx,
            remap,
            direction,
            policy: *policy,
            scenes: None,
            shortcuts: None,
        }
    }

    /// `false` if nobody receives what is captured anymore: the capture has to release the input.
    pub fn send(&self, event: InputEvent) -> bool {
        if let Some(scenes) = &self.scenes {
            scenes.observe(&event);
        }
        // The mark is set here, on the capture thread (the hook's or the tap's).
        let Some(shortcuts) = &self.shortcuts else {
            return self.tx.send(Captured::now(self.remap.apply(event))).is_ok();
        };
        let translated = shortcuts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .process(event);
        translated
            .into_iter()
            .all(|e| self.tx.send(Captured::now(e)).is_ok())
    }

    /// A handle that does not keep the channel open.
    pub fn downgrade(&self) -> WeakCaptureSink {
        WeakCaptureSink {
            tx: self.tx.downgrade(),
            remap: self.remap,
            direction: self.direction,
            policy: self.policy,
            scenes: self.scenes.clone(),
            shortcuts: self.shortcuts.clone(),
        }
    }

    /// With the scenes' shortcuts (they work on any machine of the desk: the keyboard
    /// is captured here even if the cursor is on the other).
    pub fn with_scenes(mut self, scenes: crate::agent::SceneShortcuts) -> Self {
        self.scenes = Some(scenes);
        self
    }

    /// Translates the source system's shortcuts into the target's (`event::shortcuts`) on top
    /// of the modifier remap. `exempt` are the physical keys of the Tandem's own shortcuts:
    /// they keep matching the chord as typed. Without a direction (same systems, or
    /// `cmd_ctrl` off) there is nothing to translate.
    pub fn with_shortcuts(mut self, exempt: Vec<HidUsage>) -> Self {
        self.shortcuts = self.direction.map(|d| {
            Arc::new(Mutex::new(
                ShortcutTranslator::new(d, &self.policy).with_exempt(exempt),
            ))
        });
        self
    }

    /// A physical key as seen by whoever receives what is captured. The lock shortcut is
    /// written with this machine's keys, but the session compares them with what is already
    /// remapped.
    pub fn remap_key(&self, usage: HidUsage) -> HidUsage {
        usage
            .modifier()
            .map_or(usage, |m| self.remap.map(m).usage())
    }
}

/// What each machine contributes to the shared keyboard, mouse and clipboard.
/// Calls can block (creating a tap, reading the screens): the agent makes them
/// outside the runtime.
pub trait InputBackends: Send + Sync + 'static {
    /// Starts the capture; what is captured goes to `sink`.
    fn capture(&self, sink: CaptureSink) -> Result<DynCapture, String>;

    fn injector(&self) -> Result<DynInjector, String>;

    fn clipboard(&self) -> Result<DynClipboard, String>;

    /// `None` if this system does not know how to list its USB devices.
    fn scanner(&self) -> Option<DynScanner>;

    /// This machine's screens, in the cursor's native coordinates: the default arrangement
    /// when the desk does not declare its own (DESIGN §7).
    fn screens(&self) -> Result<Vec<Screen>, String>;

    /// The EDID identity of each screen of [`Self::screens`], under its id (T-340): with it
    /// each monitor is linked to its screen. Empty where the system does not say.
    fn screen_keys(&self) -> Result<Vec<(String, DisplayKey)>, String> {
        Ok(Vec::new())
    }

    /// System permissions missing to capture or inject.
    fn missing_permissions(&self) -> Vec<InputPermission>;

    /// Asks the system for the missing permissions (on macOS, its dialog) and returns the ones
    /// still missing. What was granted usually holds only after restarting the process. Without a
    /// possible dialog, it is the same as [`Self::missing_permissions`].
    fn request_permissions(&self) -> Vec<InputPermission> {
        self.missing_permissions()
    }
}

/// This system's real backends: CGEventTap/CGEventPost on macOS, hooks and
/// `SendInput` on Windows, `arboard` for the clipboard and IOKit/SetupAPI for the hub.
/// The screens are the bounds injection already reads (`CGDisplayBounds` on macOS, the
/// virtual desktop's monitors on Windows). On other systems there is no capture or
/// injection, and the agent reports it.
#[derive(Debug, Default)]
pub struct PlatformInput;

impl PlatformInput {
    pub fn new() -> Self {
        PlatformInput
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

    fn screens(&self) -> Result<Vec<Screen>, String> {
        Ok(detected(&platform::screens()?))
    }

    fn screen_keys(&self) -> Result<Vec<(String, DisplayKey)>, String> {
        Ok(detected_keys(&platform::displays()?))
    }

    fn missing_permissions(&self) -> Vec<InputPermission> {
        platform::missing_permissions()
    }

    fn request_permissions(&self) -> Vec<InputPermission> {
        platform::request_permissions()
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use tandem_input::Rect;
    use tandem_input::displays::Display;
    use tandem_input::macos::capture::{
        MacCapture, Permission, Permissions, permissions, request_permissions as ask,
    };
    use tandem_input::macos::emulate::{CgSink, MacEmulator, MacSink};
    use tandem_proto::InputPermission;

    use super::{CaptureSink, DynCapture, DynInjector, DynScanner};

    /// The bounds of each active screen (`CGDisplayBounds`), in points.
    pub fn screens() -> Result<Vec<Rect>, String> {
        let mut sink = CgSink::new().map_err(|e| e.to_string())?;
        sink.screens().map_err(|e| e.to_string())
    }

    /// The same screens with the EDID identity CoreGraphics gives for each one.
    pub fn displays() -> Result<Vec<Display>, String> {
        tandem_input::macos::emulate::displays().map_err(|e| e.to_string())
    }

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

    fn wire(p: Permissions) -> Vec<InputPermission> {
        p.missing()
            .into_iter()
            .map(|p| match p {
                Permission::Accessibility => InputPermission::Accessibility,
                Permission::InputMonitoring => InputPermission::InputMonitoring,
            })
            .collect()
    }

    pub fn missing_permissions() -> Vec<InputPermission> {
        wire(permissions())
    }

    /// macOS dialog (Accessibility) and registration in Input Monitoring.
    pub fn request_permissions() -> Vec<InputPermission> {
        wire(ask())
    }
}

#[cfg(windows)]
mod platform {
    use tandem_input::Rect;
    use tandem_input::displays::Display;
    use tandem_input::windows::capture::{CaptureConfig, WindowsCapture};
    use tandem_input::windows::emulate::{Emulator, InputSink, SendInputSink};
    use tandem_proto::InputPermission;

    use super::{CaptureSink, DynCapture, DynInjector, DynScanner};

    /// Each monitor of the virtual desktop, in physical pixels (the process is
    /// Per-Monitor-V2); without monitors, the whole virtual desktop.
    pub fn screens() -> Result<Vec<Rect>, String> {
        let desktop = SendInputSink::new()
            .and_then(|sink| sink.desktop())
            .map_err(|e| e.to_string())?;
        Ok(if desktop.monitors.is_empty() {
            vec![desktop.virtual_rect]
        } else {
            desktop.monitors
        })
    }

    /// The same monitors with the EDID Windows saved for each one; with only the whole virtual
    /// desktop, none.
    pub fn displays() -> Result<Vec<Display>, String> {
        tandem_input::windows::emulate::displays().map_err(|e| e.to_string())
    }

    pub fn capture(sink: CaptureSink) -> Result<DynCapture, String> {
        WindowsCapture::start(CaptureConfig::default(), move |ev| {
            // Without a receiver the agent is already closing and releases the capture.
            let _ = sink.send(ev);
        })
        .map(DynCapture::new)
        .map_err(|e| e.to_string())
    }

    /// `SendInput` through the secure desktop helper when the service is up (UAC prompts,
    /// sign-in screen, elevated windows; T-027), locally otherwise.
    pub fn injector() -> Result<DynInjector, String> {
        use crate::secure::routed::RoutedSink;
        use crate::secure::windows::PipeConnector;

        let sink = SendInputSink::new().map_err(|e| e.to_string())?;
        let routed = RoutedSink::new(sink, Box::new(PipeConnector::new()))
            .with_foreground(Box::new(crate::secure::windows::WinForeground));
        Emulator::new(routed)
            .map(DynInjector::new)
            .map_err(|e| e.to_string())
    }

    pub fn scanner() -> Option<DynScanner> {
        Some(DynScanner::new(tandem_peripherals::SystemScanner::new()))
    }

    pub fn missing_permissions() -> Vec<InputPermission> {
        Vec::new()
    }

    pub fn request_permissions() -> Vec<InputPermission> {
        missing_permissions()
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use tandem_input::Rect;
    use tandem_proto::InputPermission;

    use super::{CaptureSink, DynCapture, DynInjector, DynScanner};

    const UNSUPPORTED: &str = "this system has no keyboard and mouse capture or injection";

    pub fn screens() -> Result<Vec<Rect>, String> {
        Err("this system does not report its screens".to_owned())
    }

    pub fn displays() -> Result<Vec<tandem_input::displays::Display>, String> {
        screens()?;
        Ok(Vec::new())
    }

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

    pub fn request_permissions() -> Vec<InputPermission> {
        missing_permissions()
    }
}

/// This system's file detector and sender (T-121); `None` where there is none.
#[cfg(windows)]
pub fn platform_file_drag() -> Option<Arc<dyn crate::drag::FileDragBackend>> {
    use tandem_files::drag_target_windows::WindowsDropWatcher;

    // Without the edge window there is no detector; the rest of the engine continues.
    let watcher = match WindowsDropWatcher::new() {
        Ok(w) => Arc::new(w),
        Err(e) => {
            tracing::warn!("no file drag detector: {e}");
            return None;
        }
    };
    struct Shared(Arc<WindowsDropWatcher>);
    impl tandem_files::drag::DragDetector for Shared {
        fn file_drag(&self) -> Option<Vec<std::path::PathBuf>> {
            self.0.file_drag()
        }
    }
    let w = watcher.clone();
    Some(Arc::new(
        crate::drag::SystemFileDrag::new(Shared(watcher)).with_edge(move |edge| {
            w.set_strip(edge.map(|(b, e)| {
                tandem_files::drag_os::strip_rect(b, e, super::file_drag::STRIP_THICKNESS)
            }));
        }),
    ))
}

#[cfg(target_os = "macos")]
pub fn platform_file_drag() -> Option<Arc<dyn crate::drag::FileDragBackend>> {
    Some(Arc::new(crate::drag::SystemFileDrag::new(
        tandem_files::drag_macos::MacDragDetector,
    )))
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn platform_file_drag() -> Option<Arc<dyn crate::drag::FileDragBackend>> {
    None
}

#[cfg(test)]
mod tests {
    use tandem_input::{KeyState, ModifierKey};

    use super::*;

    fn key(m: ModifierKey) -> InputEvent {
        InputEvent::Key {
            usage: m.usage(),
            state: KeyState::Pressed,
        }
    }

    #[test]
    fn a_weak_sink_does_not_keep_the_channel_open() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink = CaptureSink::new(tx, &RemapPolicy::default(), Os::MacOs, None);
        let weak = sink.downgrade();
        assert!(
            weak.upgrade()
                .is_some_and(|s| s.send(key(ModifierKey::MetaLeft)))
        );
        assert!(rx.try_recv().is_ok());
        drop(sink);
        assert!(weak.upgrade().is_none());
        assert!(rx.try_recv().is_err());
        // Closed: `recv` ends instead of waiting for the weak handle.
        assert!(rx.is_closed() || rx.try_recv() == Err(mpsc::error::TryRecvError::Disconnected));
    }

    #[test]
    fn the_sink_remaps_in_the_direction_of_the_capture() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let policy = RemapPolicy::default();
        let mac = CaptureSink::new(tx.clone(), &policy, Os::MacOs, Some(Os::Windows));
        assert!(mac.send(key(ModifierKey::MetaLeft)));
        assert_eq!(
            rx.try_recv().unwrap().event,
            key(ModifierKey::ControlLeft),
            "Cmd→Ctrl"
        );
        let pc = CaptureSink::new(tx, &policy, Os::Windows, Some(Os::MacOs));
        assert!(pc.send(key(ModifierKey::ControlLeft)));
        assert_eq!(
            rx.try_recv().unwrap().event,
            key(ModifierKey::MetaLeft),
            "Ctrl→Cmd"
        );
        let a = InputEvent::Key {
            usage: HidUsage::key(0x04),
            state: KeyState::Pressed,
        };
        assert!(pc.send(a));
        assert_eq!(rx.try_recv().unwrap().event, a);
        drop(rx);
        assert!(!pc.send(a), "without a receiver the capture has to release");
    }

    /// Between two machines with the same system nothing is remapped (DECISIONS D2): the side
    /// does not matter, only the systems.
    #[test]
    fn the_same_system_on_both_sides_keeps_the_modifiers() {
        let policy = RemapPolicy::default();
        for os in Os::ALL {
            assert_eq!(direction(os, Some(os)), None);
            assert_eq!(
                direction(os, None),
                None,
                "without knowing the other, nothing"
            );
            let (tx, mut rx) = mpsc::unbounded_channel();
            let sink = CaptureSink::new(tx, &policy, os, Some(os));
            for m in [ModifierKey::MetaLeft, ModifierKey::ControlLeft] {
                assert!(sink.send(key(m)));
                assert_eq!(rx.try_recv().unwrap().event, key(m), "{os}: {m:?}");
                assert_eq!(sink.remap_key(m.usage()), m.usage());
            }
        }
    }

    #[test]
    fn keys_are_remapped_like_the_captured_events() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let policy = RemapPolicy::default();
        let pc = CaptureSink::new(tx.clone(), &policy, Os::Windows, Some(Os::MacOs));
        assert_eq!(
            pc.remap_key(ModifierKey::ControlLeft.usage()),
            ModifierKey::MetaLeft.usage()
        );
        let mac = CaptureSink::new(tx, &policy, Os::MacOs, Some(Os::Windows));
        assert_eq!(
            mac.remap_key(ModifierKey::MetaLeft.usage()),
            ModifierKey::ControlLeft.usage()
        );
        let l = HidUsage::key(0x0F);
        assert_eq!(pc.remap_key(l), l);
    }

    #[test]
    fn this_system_says_what_it_lacks() {
        let p = PlatformInput::new();
        if cfg!(not(any(windows, target_os = "macos"))) {
            assert!(p.injector().is_err());
            let (tx, _rx) = mpsc::unbounded_channel();
            assert!(
                p.capture(CaptureSink::new(
                    tx,
                    &RemapPolicy::default(),
                    Os::local(),
                    None
                ))
                .is_err()
            );
            assert!(p.scanner().is_none());
            assert!(p.screens().is_err());
        }
        if cfg!(not(target_os = "macos")) {
            assert!(p.missing_permissions().is_empty());
        }
    }
}
