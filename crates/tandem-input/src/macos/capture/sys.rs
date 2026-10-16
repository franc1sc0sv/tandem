//! La parte que habla con macOS: el tap, su hilo y el cursor.

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
use super::translate::{INJECTED_MARKER, RawEvent, TapAction, Translator};
use super::{CaptureError, Permissions};
use crate::event::InputEvent;
use crate::geometry::Point;

/// `NSEventTypeSystemDefined`: volumen, brillo y teclas multimedia.
const SYSTEM_DEFINED: CGEventType = CGEventType(14);

/// Cada cuánto el hilo del tap mira si tiene que terminar, por si el aviso de
/// parar llegó antes de que entrara al run loop.
const RUN_SLICE_SECONDS: f64 = 0.5;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: Option<&CFDictionary>) -> u8;
    static kAXTrustedCheckOptionPrompt: &'static CFString;
    // API privada de CoreGraphics (SkyLight): sin `SetsCursorInBackground`,
    // `CGDisplayHideCursor` no oculta el cursor si Tandem no es la app activa,
    // que es siempre (es un agente sin ventana).
    fn _CGSDefaultConnection() -> i32;
    fn CGSSetConnectionProperty(
        connection: i32,
        target: i32,
        key: &CFString,
        value: &CFBoolean,
    ) -> i32;
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> u8;
}

/// Estado actual de los permisos de TCC, sin preguntar nada al usuario.
pub fn permissions() -> Permissions {
    // SAFETY: sin argumentos; solo consulta TCC.
    let accessibility = unsafe { AXIsProcessTrusted() } != 0;
    Permissions {
        accessibility,
        input_monitoring: CGPreflightListenEventAccess(),
    }
}

/// Pide los permisos que falten: macOS muestra su diálogo (una sola vez por
/// binario) y lleva al usuario a Ajustes. Devuelve el estado después de pedir;
/// lo concedido suele valer recién al reiniciar el proceso.
pub fn request_permissions() -> Permissions {
    let before = permissions();
    if !before.accessibility {
        // SAFETY: `kAXTrustedCheckOptionPrompt` es una constante de
        // ApplicationServices, válida durante todo el proceso.
        let key: &CFString = unsafe { kAXTrustedCheckOptionPrompt };
        // SAFETY: `kCFBooleanTrue` es una constante de CoreFoundation.
        if let Some(yes) = unsafe { kCFBooleanTrue } {
            let options = CFDictionary::<CFString, CFBoolean>::from_slices(&[key], &[yes]);
            let options: &CFDictionary = options.as_ref();
            // SAFETY: diccionario válido con la clave y el valor documentados.
            unsafe { AXIsProcessTrustedWithOptions(Some(options)) };
        }
    }
    if !before.input_monitoring {
        CGRequestListenEventAccess();
    }
    permissions()
}

/// Si algún proceso activó Secure Event Input (un campo de contraseña).
pub fn secure_input_active() -> bool {
    // SAFETY: sin argumentos; solo consulta el estado global.
    unsafe { IsSecureEventInputEnabled() != 0 }
}

/// "Scroll natural" de macOS (`com.apple.swipescrolldirection`). Si no está
/// definido, macOS lo trata como activado.
fn natural_scroll() -> bool {
    let key = CFString::from_static_str("com.apple.swipescrolldirection");
    let mut valid = 0u8;
    // SAFETY: claves válidas y puntero a un `Boolean` local.
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

/// El cursor real, con CoreGraphics.
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
        // SAFETY: `kCFBooleanTrue` es una constante de CoreFoundation.
        let Some(yes) = (unsafe { kCFBooleanTrue }) else {
            return;
        };
        // SAFETY: la conexión por defecto del proceso, una clave CFString y un
        // CFBoolean válidos. Si la API privada falla, el cursor solo no se oculta.
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

/// Lo que ve el callback del tap. Vive en el hilo del tap mientras corre su run
/// loop; solo ese hilo lo toca.
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
        if !matches!(raw, RawEvent::TapDisabled | RawEvent::Other)
            && CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUserData)
                == INJECTED_MARKER
        {
            return TapAction::Pass;
        }
        let grabbed = self.grab.is_grabbed();
        let natural = self.natural_scroll.load(Ordering::Relaxed);
        let mut alive = true;
        let sink = &mut self.sink;
        let action = self
            .translator
            .translate(&raw, grabbed, natural, &mut |e| alive &= sink(e));
        if !alive && grabbed {
            // Nadie recibe lo que tragamos: devolver la Mac a su dueño.
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
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
            RawEvent::TapDisabled
        }
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

/// Callback del tap. Corre en el hilo del tap; tiene que ser rápido o macOS lo
/// apaga (y entonces llega `TapDisabledByTimeout`).
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
    // SAFETY: `user_info` es el `TapContext` que creó `tap_thread`, que lo libera
    // recién después de invalidar el tap y salir del run loop; el callback solo
    // corre en ese hilo, así que no hay otra referencia viva.
    let ctx = unsafe { &mut *user_info.cast::<TapContext>() };
    // SAFETY: CoreGraphics garantiza que el evento vale durante el callback.
    let event = unsafe { event.as_ref() };
    // Un pánico no puede cruzar el código de CoreGraphics: ante cualquier
    // problema, el evento sigue su camino.
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

/// El run loop del hilo del tap, para pararlo desde otro hilo.
struct RunLoopHandle(CFRetained<CFRunLoop>);

// SAFETY: solo se usa para `CFRunLoopStop`, que CoreFoundation documenta como
// seguro desde cualquier hilo.
unsafe impl Send for RunLoopHandle {}

fn tap_thread(
    grab: Arc<Grab<SysCursor>>,
    natural_scroll: Arc<AtomicBool>,
    sink: Sink,
    stop: Arc<AtomicBool>,
    ready: mpsc::SyncSender<Result<RunLoopHandle, CaptureError>>,
) {
    // Se arma aquí porque el puerto del tap no puede cambiar de hilo.
    let user_info = Box::into_raw(Box::new(TapContext {
        grab,
        natural_scroll,
        translator: Translator::new(),
        sink,
        port: None,
    }));
    // SAFETY: lo devuelve `Box::into_raw` y se libera una sola vez, cuando el tap
    // ya no puede llamar al callback.
    let free = move || drop(unsafe { Box::from_raw(user_info) });
    // SAFETY: el callback tiene la firma de `CGEventTapCallBack`; `user_info`
    // apunta al contexto, que vive hasta después de invalidar el tap.
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
    // SAFETY: el run loop todavía no corre, así que el callback no tiene una
    // referencia al contexto; escribir a través del puntero no crea alias.
    unsafe { (*user_info).port = Some(port.clone()) };
    // SAFETY: constantes de CoreFoundation, válidas durante todo el proceso.
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
    // Recién ahora el contexto puede liberarse: el tap ya no llama al callback.
    free();
}

/// Captura de entrada en macOS.
///
/// Mientras existe, un CGEventTap ve toda la entrada de la Mac y la entrega a la
/// función `sink` (en el hilo del tap: tiene que volver enseguida, p. ej.
/// mandando a un canal sin límite). `sink` devuelve `false` cuando ya nadie
/// recibe eventos; si la entrada estaba tomada, se devuelve.
///
/// Con el foco local salen `MotionRelative` (unidades nativas, con la
/// aceleración de macOS) seguidos de `MotionAbsolute` con la posición real del
/// cursor; con el foco remoto, solo los deltas.
pub struct MacCapture {
    grab: Arc<Grab<SysCursor>>,
    natural_scroll: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    run_loop: RunLoopHandle,
    thread: Option<JoinHandle<()>>,
}

impl MacCapture {
    /// Arranca el tap en su hilo. Falla si falta Accesibilidad o si macOS no deja
    /// crear el tap.
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
                Err(CaptureError::Thread("terminó sin avisar".into()))
            }
        }
    }

    /// `true`: tragar la entrada local y congelar y ocultar el cursor, porque el
    /// foco pasa a la otra máquina. Falla con Secure Event Input activo.
    /// `false`: devolverla.
    pub fn set_grab(&mut self, grab: bool) -> Result<(), CaptureError> {
        if grab {
            // La preferencia puede cambiar mientras corre; se relee al cruzar.
            self.natural_scroll
                .store(natural_scroll(), Ordering::Relaxed);
        }
        self.grab.set(grab)
    }

    /// Pone el cursor local en `pos` (coordenadas globales en puntos, origen
    /// arriba a la izquierda de la pantalla principal).
    pub fn warp(&mut self, pos: Point) -> Result<(), CaptureError> {
        self.grab.warp(pos)
    }

    pub fn is_grabbed(&self) -> bool {
        self.grab.is_grabbed()
    }
}

impl Drop for MacCapture {
    fn drop(&mut self) {
        // Primero devolver teclado y cursor: pase lo que pase después, la Mac
        // queda usable.
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
    fn la_mascara_cubre_la_entrada() {
        let m = event_mask();
        for t in [1u32, 2, 3, 4, 5, 6, 7, 10, 11, 12, 14, 22, 25, 26, 27] {
            assert_ne!(m & (1 << t), 0, "falta el tipo {t}");
        }
        assert_eq!(m.count_ones(), 15);
    }

    #[test]
    fn consultas_sin_dispositivo() {
        // En CI no hay permisos ni campo de contraseña; solo tiene que responder.
        let _ = permissions();
        let _ = secure_input_active();
        let _ = natural_scroll();
    }

    /// Captura real: requiere Accesibilidad. Mueve el mouse y escribe durante 5 s.
    #[test]
    #[ignore = "hardware"]
    fn captura_real_cinco_segundos() {
        let (tx, rx) = mpsc::channel();
        let capture = MacCapture::start(move |e| tx.send(e).is_ok()).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(5));
        drop(capture);
        let events: Vec<_> = rx.try_iter().collect();
        assert!(!events.is_empty(), "no llegó ningún evento");
    }
}
