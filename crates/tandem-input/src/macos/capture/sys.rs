//! The part that talks to macOS: the tap, its thread and the cursor.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use objc2_app_kit::NSEvent;
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFMachPort, CFPreferencesGetAppBooleanValue, CFRetained, CFRunLoop,
    CFString, CGPoint, kCFBooleanTrue, kCFPreferencesAnyApplication, kCFRunLoopCommonModes,
    kCFRunLoopDefaultMode,
};
use objc2_core_graphics::{
    CGAssociateMouseAndMouseCursorPosition, CGDisplayHideCursor, CGDisplayShowCursor, CGError,
    CGEvent, CGEventField, CGEventMask, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, CGPreflightListenEventAccess, CGRequestListenEventAccess,
    CGWarpMouseCursorPosition, kCGNullDirectDisplay,
};

use super::grab::{CursorBackend, Grab};
use super::translate::{RawEvent, TapAction, TapDisabledBy, Translator};
use super::{CaptureError, Permissions};
use crate::event::InputEvent;
use crate::geometry::Point;

/// `NSEventTypeSystemDefined`: volume, brightness and media keys.
const SYSTEM_DEFINED: CGEventType = CGEventType(14);

/// How often the tap thread checks whether it has to end, in case the stop
/// signal arrived before it entered the run loop.
const RUN_SLICE_SECONDS: f64 = 0.5;

#[link(name = "ApplicationServices", kind = "framework")]
// SAFETY: signatures verified against the SDK (ApplicationServices); the functions only
// read system state and the pointers passed do not outlive the call.
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: Option<&CFDictionary>) -> u8;
    static kAXTrustedCheckOptionPrompt: &'static CFString;
    // Private CoreGraphics API (SkyLight): without `SetsCursorInBackground`,
    // `CGDisplayHideCursor` does not hide the cursor if Tandem is not the active app,
    // which is always the case (it is an agent without a window).
    fn _CGSDefaultConnection() -> i32;
    fn CGSSetConnectionProperty(
        connection: i32,
        target: i32,
        key: &CFString,
        value: &CFBoolean,
    ) -> i32;
}

#[link(name = "Carbon", kind = "framework")]
// SAFETY: signature verified against the SDK (Carbon); it takes no pointers and modifies no state.
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> u8;
}

/// Current state of the TCC permissions, without asking the user anything.
pub fn permissions() -> Permissions {
    // SAFETY: no arguments; it only queries TCC.
    let accessibility = unsafe { AXIsProcessTrusted() } != 0;
    Permissions {
        accessibility,
        input_monitoring: CGPreflightListenEventAccess(),
    }
}

/// Asks for the missing permissions: macOS shows its dialog (only once per
/// binary) and takes the user to Settings. Returns the state after asking;
/// what was granted usually only takes effect on restarting the process.
pub fn request_permissions() -> Permissions {
    let before = permissions();
    if !before.accessibility {
        // SAFETY: `kAXTrustedCheckOptionPrompt` is a constant of
        // ApplicationServices, valid for the whole process.
        let key: &CFString = unsafe { kAXTrustedCheckOptionPrompt };
        // SAFETY: `kCFBooleanTrue` is a CoreFoundation constant.
        if let Some(yes) = unsafe { kCFBooleanTrue } {
            let options = CFDictionary::<CFString, CFBoolean>::from_slices(&[key], &[yes]);
            let options: &CFDictionary = options.as_ref();
            // SAFETY: a valid dictionary with the documented key and value.
            unsafe { AXIsProcessTrustedWithOptions(Some(options)) };
        }
    }
    if !before.input_monitoring {
        CGRequestListenEventAccess();
    }
    permissions()
}

/// Whether some process enabled Secure Event Input (a password field).
pub fn secure_input_active() -> bool {
    // SAFETY: no arguments; it only queries the global state.
    unsafe { IsSecureEventInputEnabled() != 0 }
}

/// macOS "natural scrolling" (`com.apple.swipescrolldirection`). If it is not
/// defined, macOS treats it as enabled.
fn natural_scroll() -> bool {
    let key = CFString::from_static_str("com.apple.swipescrolldirection");
    let mut valid = 0u8;
    // SAFETY: valid keys and a pointer to a local `Boolean`.
    let value =
        unsafe { CFPreferencesGetAppBooleanValue(&key, kCFPreferencesAnyApplication, &mut valid) };
    if valid != 0 { value } else { true }
}

fn check(op: &'static str, err: CGError) -> Result<(), CaptureError> {
    if err == CGError::Success {
        Ok(())
    } else {
        Err(CaptureError::Cursor { op, code: err.0 })
    }
}

/// The real cursor, with CoreGraphics.
#[derive(Debug)]
pub struct SysCursor {
    background_set: bool,
}

impl SysCursor {
    pub fn new() -> Self {
        Self {
            background_set: false,
        }
    }

    fn allow_hide_in_background(&mut self) {
        if self.background_set {
            return;
        }
        let key = CFString::from_static_str("SetsCursorInBackground");
        // SAFETY: `kCFBooleanTrue` is a CoreFoundation constant.
        let Some(yes) = (unsafe { kCFBooleanTrue }) else {
            return;
        };
        // SAFETY: the process's default connection, a CFString key and a valid
        // CFBoolean. If the private API fails, the cursor is just not hidden.
        let err = unsafe {
            let cid = _CGSDefaultConnection();
            CGSSetConnectionProperty(cid, cid, &key, yes)
        };
        self.background_set = err == 0;
    }
}

impl Default for SysCursor {
    fn default() -> Self {
        Self::new()
    }
}

impl CursorBackend for SysCursor {
    fn set_frozen(&mut self, frozen: bool) -> Result<(), CaptureError> {
        check(
            "CGAssociateMouseAndMouseCursorPosition",
            CGAssociateMouseAndMouseCursorPosition(!frozen),
        )
    }

    fn set_hidden(&mut self, hidden: bool) -> Result<(), CaptureError> {
        if hidden {
            self.allow_hide_in_background();
            check(
                "CGDisplayHideCursor",
                CGDisplayHideCursor(kCGNullDirectDisplay),
            )
        } else {
            check(
                "CGDisplayShowCursor",
                CGDisplayShowCursor(kCGNullDirectDisplay),
            )
        }
    }

    fn warp(&mut self, pos: Point) -> Result<(), CaptureError> {
        check(
            "CGWarpMouseCursorPosition",
            CGWarpMouseCursorPosition(CGPoint { x: pos.x, y: pos.y }),
        )
    }

    fn secure_input_active(&self) -> bool {
        secure_input_active()
    }
}

type Sink = Box<dyn FnMut(InputEvent) -> bool + Send>;

/// What the tap callback sees. It lives on the tap thread while its run
/// loop runs; only that thread touches it.
struct TapContext {
    grab: Arc<Grab<SysCursor>>,
    natural_scroll: Arc<AtomicBool>,
    translator: Translator,
    sink: Sink,
    port: Option<CFRetained<CFMachPort>>,
}

impl TapContext {
    fn handle(&mut self, ty: CGEventType, event: &CGEvent) -> TapAction {
        let raw = read_event(ty, event);
        let source_user_data =
            CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUserData);
        let grabbed = self.grab.is_grabbed();
        let natural = self.natural_scroll.load(Ordering::Relaxed);
        let mut alive = true;
        let sink = &mut self.sink;
        let action = self
            .translator
            .tap(&raw, source_user_data, grabbed, natural, &mut |e| {
                alive &= sink(e)
            });
        if !alive && grabbed {
            // Nobody receives what we swallow: give the Mac back to its owner.
            self.grab.release();
            return TapAction::Pass;
        }
        if action == TapAction::Reenable
            && let Some(port) = &self.port
        {
            CGEvent::tap_enable(port, true);
        }
        action
    }
}

fn read_event(ty: CGEventType, event: &CGEvent) -> RawEvent {
    let ev = Some(event);
    let int = |f| CGEvent::integer_value_field(ev, f);
    let float = |f| CGEvent::double_value_field(ev, f);
    let location = || {
        let p = CGEvent::location(ev);
        Point::new(p.x, p.y)
    };
    let keycode = || u16::try_from(int(CGEventField::KeyboardEventKeycode)).unwrap_or(u16::MAX);
    match ty {
        CGEventType::TapDisabledByTimeout => RawEvent::TapDisabled(TapDisabledBy::Timeout),
        CGEventType::TapDisabledByUserInput => RawEvent::TapDisabled(TapDisabledBy::UserInput),
        CGEventType::KeyDown => RawEvent::KeyDown { keycode: keycode() },
        CGEventType::KeyUp => RawEvent::KeyUp { keycode: keycode() },
        CGEventType::FlagsChanged => RawEvent::FlagsChanged {
            keycode: keycode(),
            flags: CGEvent::flags(ev).0,
        },
        CGEventType::LeftMouseDown | CGEventType::RightMouseDown | CGEventType::OtherMouseDown => {
            RawEvent::MouseDown {
                button: int(CGEventField::MouseEventButtonNumber),
                location: location(),
            }
        }
        CGEventType::LeftMouseUp | CGEventType::RightMouseUp | CGEventType::OtherMouseUp => {
            RawEvent::MouseUp {
                button: int(CGEventField::MouseEventButtonNumber),
                location: location(),
            }
        }
        CGEventType::MouseMoved
        | CGEventType::LeftMouseDragged
        | CGEventType::RightMouseDragged
        | CGEventType::OtherMouseDragged => RawEvent::Moved {
            dx: float(CGEventField::MouseEventDeltaX),
            dy: float(CGEventField::MouseEventDeltaY),
            location: location(),
        },
        CGEventType::ScrollWheel => RawEvent::Scroll {
            continuous: int(CGEventField::ScrollWheelEventIsContinuous) != 0,
            lines_axis1: float(CGEventField::ScrollWheelEventFixedPtDeltaAxis1),
            lines_axis2: float(CGEventField::ScrollWheelEventFixedPtDeltaAxis2),
            points_axis1: int(CGEventField::ScrollWheelEventPointDeltaAxis1) as f64,
            points_axis2: int(CGEventField::ScrollWheelEventPointDeltaAxis2) as f64,
        },
        SYSTEM_DEFINED => objc2::rc::autoreleasepool(|_| match NSEvent::eventWithCGEvent(event) {
            Some(ns) => RawEvent::SystemDefined {
                subtype: ns.subtype().0,
                data1: ns.data1() as i64,
            },
            None => RawEvent::Other,
        }),
        _ => RawEvent::Other,
    }
}

/// Tap callback. It runs on the tap thread; it has to be fast or macOS
/// turns it off (and then `TapDisabledByTimeout` arrives).
// SAFETY: the signature matches `CGEventTapCallBack`. It is only invoked by CoreGraphics on the
// tap thread, with `user_info` null or the current `TapContext` (see below) and a valid event.
unsafe extern "C-unwind" fn tap_callback(
    _proxy: CGEventTapProxy,
    ty: CGEventType,
    event: NonNull<CGEvent>,
    user_info: *mut c_void,
) -> *mut CGEvent {
    let pass = event.as_ptr();
    if user_info.is_null() {
        return pass;
    }
    // SAFETY: `user_info` is the `TapContext` that `tap_thread` created, which frees it
    // only after invalidating the tap and leaving the run loop; the callback only
    // runs on that thread, so there is no other live reference.
    let ctx = unsafe { &mut *user_info.cast::<TapContext>() };
    // SAFETY: CoreGraphics guarantees the event is valid during the callback.
    let event = unsafe { event.as_ref() };
    // A panic cannot cross CoreGraphics code: on any
    // problem, the event goes on its way.
    match catch_unwind(AssertUnwindSafe(|| ctx.handle(ty, event))) {
        Ok(TapAction::Swallow) => ptr::null_mut(),
        _ => pass,
    }
}

fn event_mask() -> CGEventMask {
    [
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::OtherMouseDown,
        CGEventType::OtherMouseUp,
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDragged,
        CGEventType::OtherMouseDragged,
        CGEventType::ScrollWheel,
        SYSTEM_DEFINED,
    ]
    .iter()
    .fold(0, |m, t| m | (1 << t.0))
}

/// The tap thread's run loop, to stop it from another thread.
struct RunLoopHandle(CFRetained<CFRunLoop>);

// SAFETY: it is only used for `CFRunLoopStop`, which CoreFoundation documents as
// safe from any thread.
unsafe impl Send for RunLoopHandle {}

fn tap_thread(
    grab: Arc<Grab<SysCursor>>,
    natural_scroll: Arc<AtomicBool>,
    sink: Sink,
    stop: Arc<AtomicBool>,
    ready: mpsc::SyncSender<Result<RunLoopHandle, CaptureError>>,
) {
    // It is built here because the tap port cannot change threads.
    let user_info = Box::into_raw(Box::new(TapContext {
        grab,
        natural_scroll,
        translator: Translator::new(),
        sink,
        port: None,
    }));
    // SAFETY: returned by `Box::into_raw` and freed only once, when the tap
    // can no longer call the callback.
    let free = move || drop(unsafe { Box::from_raw(user_info) });
    // SAFETY: the callback has the signature of `CGEventTapCallBack`; `user_info`
    // points to the context, which lives until after the tap is invalidated.
    let port = unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::HIDEventTap,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default,
            event_mask(),
            Some(tap_callback),
            user_info.cast(),
        )
    };
    let Some(port) = port else {
        free();
        let _ = ready.send(Err(CaptureError::TapCreate));
        return;
    };
    let (Some(source), Some(run_loop)) = (
        CFMachPort::new_run_loop_source(None, Some(&port), 0),
        CFRunLoop::current(),
    ) else {
        port.invalidate();
        free();
        let _ = ready.send(Err(CaptureError::TapCreate));
        return;
    };
    // SAFETY: the run loop is not running yet, so the callback holds no
    // reference to the context; writing through the pointer creates no alias.
    unsafe { (*user_info).port = Some(port.clone()) };
    // SAFETY: CoreFoundation constants, valid for the whole process.
    let (common, default) = unsafe { (kCFRunLoopCommonModes, kCFRunLoopDefaultMode) };
    run_loop.add_source(Some(&source), common);
    CGEvent::tap_enable(&port, true);
    if ready.send(Ok(RunLoopHandle(run_loop.clone()))).is_err() {
        stop.store(true, Ordering::Release);
    }
    while !stop.load(Ordering::Acquire) {
        CFRunLoop::run_in_mode(default, RUN_SLICE_SECONDS, false);
    }
    CGEvent::tap_enable(&port, false);
    run_loop.remove_source(Some(&source), common);
    port.invalidate();
    // Only now can the context be freed: the tap no longer calls the callback.
    free();
}

/// Input capture on macOS.
///
/// While it exists, a CGEventTap sees all of the Mac's input and delivers it to the
/// `sink` function (on the tap thread: it has to return right away, e.g. by
/// sending to an unbounded channel). `sink` returns `false` when nobody
/// receives events anymore; if the input was taken, it is given back.
///
/// With local focus, `MotionRelative` comes out (native units, with macOS
/// acceleration) followed by `MotionAbsolute` with the cursor's real
/// position; with remote focus, only the deltas.
pub struct MacCapture {
    grab: Arc<Grab<SysCursor>>,
    natural_scroll: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    run_loop: RunLoopHandle,
    thread: Option<JoinHandle<()>>,
}

impl MacCapture {
    /// Starts the tap on its thread. Fails if Accessibility is missing or if macOS does not allow
    /// creating the tap.
    pub fn start(
        sink: impl FnMut(InputEvent) -> bool + Send + 'static,
    ) -> Result<Self, CaptureError> {
        if let Some(missing) = permissions().missing_for_active_tap() {
            return Err(CaptureError::Permission(missing));
        }
        let grab = Arc::new(Grab::new(SysCursor::new()));
        let natural = Arc::new(AtomicBool::new(natural_scroll()));
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_grab, thread_natural) = (grab.clone(), natural.clone());
        let sink: Sink = Box::new(sink);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("tandem-macos-tap".into())
            .spawn(move || tap_thread(thread_grab, thread_natural, sink, thread_stop, ready_tx))
            .map_err(|e| CaptureError::Thread(e.to_string()))?;
        match ready_rx.recv() {
            Ok(Ok(run_loop)) => Ok(Self {
                grab,
                natural_scroll: natural,
                stop,
                run_loop,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(CaptureError::Thread("ended without reporting".into()))
            }
        }
    }

    /// `true`: swallow local input and freeze and hide the cursor, because the
    /// focus moves to the other machine. Fails with Secure Event Input active.
    /// `false`: give it back.
    pub fn set_grab(&mut self, grab: bool) -> Result<(), CaptureError> {
        if grab {
            // The preference may change while it runs; it is reread on crossing.
            self.natural_scroll
                .store(natural_scroll(), Ordering::Relaxed);
        }
        self.grab.set(grab)
    }

    /// Puts the local cursor at `pos` (global coordinates in points, origin
    /// at the top left of the main screen).
    pub fn warp(&mut self, pos: Point) -> Result<(), CaptureError> {
        self.grab.warp(pos)
    }

    pub fn is_grabbed(&self) -> bool {
        self.grab.is_grabbed()
    }
}

impl Drop for MacCapture {
    fn drop(&mut self) {
        // First give back keyboard and cursor: whatever happens afterwards, the Mac
        // stays usable.
        self.grab.release();
        self.stop.store(true, Ordering::Release);
        self.run_loop.0.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mask_covers_the_input() {
        let m = event_mask();
        for t in [1u32, 2, 3, 4, 5, 6, 7, 10, 11, 12, 14, 22, 25, 26, 27] {
            assert_ne!(m & (1 << t), 0, "type {t} is missing");
        }
        assert_eq!(m.count_ones(), 15);
    }

    #[test]
    fn queries_without_a_device() {
        // In CI there are no permissions or password field; it only has to answer.
        let _ = permissions();
        let _ = secure_input_active();
        let _ = natural_scroll();
    }

    /// Real capture: requires Accessibility. Moves the mouse and types for 5 s.
    #[test]
    #[ignore = "hardware"]
    fn real_capture_five_seconds() {
        let (tx, rx) = mpsc::channel();
        let capture = MacCapture::start(move |e| tx.send(e).is_ok()).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(5));
        drop(capture);
        let events: Vec<_> = rx.try_iter().collect();
        assert!(!events.is_empty(), "no event arrived");
    }
}
