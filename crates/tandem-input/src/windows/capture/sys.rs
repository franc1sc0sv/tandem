//! Lo que toca Win32: hilo de hooks, Raw Input, sondas y DPI.

use std::cell::{OnceCell, RefCell};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Sender, SyncSender};
use std::thread::JoinHandle;

use ::windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::System::Threading::{
    GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
};
use ::windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    GetThreadDpiAwarenessContext, SetProcessDpiAwarenessContext,
};
use ::windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, MOUSEEVENTF_MOVE,
    MOUSEINPUT, SendInput, VIRTUAL_KEY,
};
use ::windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT,
    RIDEV_INPUTSINK, RIDEV_REMOVE, RIM_TYPEMOUSE, RegisterRawInputDevices,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetSystemMetrics, HC_ACTION, HHOOK, HWND_MESSAGE, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
    PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN,
    SM_CYVIRTUALSCREEN, SetCursorPos, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_INPUT, WM_QUIT, WM_USER,
};
use ::windows::core::{PCWSTR, w};

use super::consts;
use super::{
    CaptureConfig, CaptureError, HookControl, HookEvent, HookFilter, HookKind, KeyboardHook,
    MouseHook, PROBE_TAG, RawMouse, Verdict, run_pipeline,
};
use crate::event::InputEvent;
use crate::geometry::Point;

// Las constantes portables tienen que ser las de Windows.
const _: () = {
    use ::windows::Win32::UI::Input::KeyboardAndMouse as km;
    use ::windows::Win32::UI::Input::{MOUSE_MOVE_ABSOLUTE, MOUSE_VIRTUAL_DESKTOP};
    use ::windows::Win32::UI::WindowsAndMessaging as wm;
    assert!(consts::LLKHF_EXTENDED == wm::LLKHF_EXTENDED.0);
    assert!(consts::LLKHF_INJECTED == wm::LLKHF_INJECTED.0);
    assert!(consts::LLKHF_UP == wm::LLKHF_UP.0);
    assert!(consts::LLMHF_INJECTED == wm::LLMHF_INJECTED);
    assert!(consts::WM_MOUSEMOVE == wm::WM_MOUSEMOVE);
    assert!(consts::WM_LBUTTONDOWN == wm::WM_LBUTTONDOWN);
    assert!(consts::WM_LBUTTONUP == wm::WM_LBUTTONUP);
    assert!(consts::WM_RBUTTONDOWN == wm::WM_RBUTTONDOWN);
    assert!(consts::WM_RBUTTONUP == wm::WM_RBUTTONUP);
    assert!(consts::WM_MBUTTONDOWN == wm::WM_MBUTTONDOWN);
    assert!(consts::WM_MBUTTONUP == wm::WM_MBUTTONUP);
    assert!(consts::WM_MOUSEWHEEL == wm::WM_MOUSEWHEEL);
    assert!(consts::WM_XBUTTONDOWN == wm::WM_XBUTTONDOWN);
    assert!(consts::WM_XBUTTONUP == wm::WM_XBUTTONUP);
    assert!(consts::WM_MOUSEHWHEEL == wm::WM_MOUSEHWHEEL);
    assert!(consts::XBUTTON1 == wm::XBUTTON1);
    assert!(consts::XBUTTON2 == wm::XBUTTON2);
    assert!(consts::MOUSE_MOVE_ABSOLUTE == MOUSE_MOVE_ABSOLUTE.0);
    assert!(consts::MOUSE_VIRTUAL_DESKTOP == MOUSE_VIRTUAL_DESKTOP.0);
    assert!(consts::VK_CANCEL == km::VK_CANCEL.0 as u32);
    assert!(consts::VK_VOLUME_MUTE == km::VK_VOLUME_MUTE.0 as u32);
    assert!(consts::VK_VOLUME_DOWN == km::VK_VOLUME_DOWN.0 as u32);
    assert!(consts::VK_VOLUME_UP == km::VK_VOLUME_UP.0 as u32);
    assert!(consts::VK_MEDIA_NEXT_TRACK == km::VK_MEDIA_NEXT_TRACK.0 as u32);
    assert!(consts::VK_MEDIA_PREV_TRACK == km::VK_MEDIA_PREV_TRACK.0 as u32);
    assert!(consts::VK_MEDIA_STOP == km::VK_MEDIA_STOP.0 as u32);
    assert!(consts::VK_MEDIA_PLAY_PAUSE == km::VK_MEDIA_PLAY_PAUSE.0 as u32);
};

/// Mensaje al hilo de hooks: reinstalarlos.
const WM_REINSTALL: u32 = WM_APP + 1;

fn os_error(op: &'static str, e: ::windows::core::Error) -> CaptureError {
    CaptureError::Os {
        op,
        code: e.code().0 as u32,
        message: e.message(),
    }
}

/// Declara el proceso Per-Monitor-V2 DPI aware: así las posiciones del hook,
/// `SetCursorPos` y la inyección absoluta usan píxeles físicos en todos los
/// monitores. Si ya lo estaba (por manifiesto o una llamada anterior), no hace nada.
pub fn ensure_per_monitor_v2() -> Result<(), CaptureError> {
    // SAFETY: solo cambia el modo de DPI del proceso; falla si ya estaba fijado.
    let set = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    // SAFETY: consultas sin efectos sobre el contexto del hilo actual.
    let is_v2 = unsafe {
        AreDpiAwarenessContextsEqual(
            GetThreadDpiAwarenessContext(),
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
    }
    .as_bool();
    match set {
        _ if is_v2 => Ok(()),
        Err(e) => Err(os_error("SetProcessDpiAwarenessContext", e)),
        Ok(()) => Err(CaptureError::DpiAwareness),
    }
}

/// Estado del hilo de hooks, accesible desde los callbacks.
struct HookCtx {
    tx: Sender<HookEvent>,
    grab: Arc<AtomicBool>,
    filter: RefCell<HookFilter>,
}

impl HookCtx {
    /// Decide y encola. Devuelve `true` si hay que tragar el evento.
    fn handle(&self, decide: impl FnOnce(&mut HookFilter, bool) -> (Verdict, HookEvent)) -> bool {
        let grab = self.grab.load(Ordering::Relaxed);
        // Los callbacks no se reentran (no bombeamos mensajes dentro), pero un
        // pánico aquí abortaría el proceso: ante la duda, dejar pasar.
        let Ok(mut filter) = self.filter.try_borrow_mut() else {
            return false;
        };
        let (verdict, event) = decide(&mut filter, grab);
        // `send` en un canal sin límite no bloquea. Si el hilo de proceso ya
        // terminó, no hay a quién avisar.
        let _ = self.tx.send(event);
        verdict != Verdict::Pass
    }
}

thread_local! {
    static CTX: OnceCell<HookCtx> = const { OnceCell::new() };
}

fn with_ctx(f: impl FnOnce(&HookCtx) -> bool) -> bool {
    CTX.try_with(|c| c.get().is_some_and(f)).unwrap_or(false)
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: con HC_ACTION, `lparam` apunta a un KBDLLHOOKSTRUCT válido
        // durante la llamada; se copia enseguida.
        let k = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let hook = KeyboardHook {
            vk: k.vkCode,
            scan_code: k.scanCode,
            flags: k.flags.0,
            time: k.time,
            extra_info: k.dwExtraInfo,
        };
        let swallow = with_ctx(|ctx| {
            ctx.handle(|filter, grab| {
                let verdict = filter.keyboard(grab, &hook);
                let event = match verdict {
                    Verdict::Probe => HookEvent::Probe(HookKind::Keyboard),
                    v => HookEvent::Keyboard {
                        hook,
                        swallowed: v == Verdict::Swallow,
                    },
                };
                (verdict, event)
            })
        });
        if swallow {
            return LRESULT(1);
        }
    }
    // SAFETY: se pasan los parámetros recibidos tal cual al siguiente hook.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: con HC_ACTION, `lparam` apunta a un MSLLHOOKSTRUCT válido
        // durante la llamada; se copia enseguida.
        let m = unsafe { *(lparam.0 as *const MSLLHOOKSTRUCT) };
        let hook = MouseHook {
            message: wparam.0 as u32,
            x: m.pt.x,
            y: m.pt.y,
            mouse_data: m.mouseData,
            flags: m.flags,
            time: m.time,
            extra_info: m.dwExtraInfo,
        };
        let swallow = with_ctx(|ctx| {
            ctx.handle(|filter, grab| {
                let verdict = filter.mouse(grab, &hook);
                let event = match verdict {
                    Verdict::Probe => HookEvent::Probe(HookKind::Mouse),
                    v => HookEvent::Mouse {
                        hook,
                        swallowed: v == Verdict::Swallow,
                    },
                };
                (verdict, event)
            })
        });
        if swallow {
            return LRESULT(1);
        }
    }
    // SAFETY: se pasan los parámetros recibidos tal cual al siguiente hook.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Los dos hooks instalados; se quitan al soltar el valor.
struct Hooks {
    keyboard: HHOOK,
    mouse: HHOOK,
}

fn unhook(hook: HHOOK) {
    // SAFETY: handle devuelto por SetWindowsHookExW en este hilo. Si Windows ya
    // lo quitó, la llamada falla sin efecto.
    let _ = unsafe { UnhookWindowsHookEx(hook) };
}

impl Hooks {
    fn install() -> Result<Self, CaptureError> {
        // SAFETY: consulta el módulo del ejecutable, sin efectos.
        let module = unsafe { GetModuleHandleW(PCWSTR::null()) }
            .map_err(|e| os_error("GetModuleHandleW", e))?;
        let instance = HINSTANCE::from(module);
        // SAFETY: `keyboard_proc` y `mouse_proc` tienen la firma de HOOKPROC y
        // existen mientras viva el programa.
        let keyboard =
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(instance), 0) }
                .map_err(|e| os_error("SetWindowsHookExW(WH_KEYBOARD_LL)", e))?;
        // SAFETY: igual que el de teclado.
        match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(instance), 0) } {
            Ok(mouse) => Ok(Self { keyboard, mouse }),
            Err(e) => {
                unhook(keyboard);
                Err(os_error("SetWindowsHookExW(WH_MOUSE_LL)", e))
            }
        }
    }
}

impl Drop for Hooks {
    fn drop(&mut self) {
        unhook(self.keyboard);
        unhook(self.mouse);
    }
}

fn raw_mouse_device(
    flags: ::windows::Win32::UI::Input::RAWINPUTDEVICE_FLAGS,
    hwnd: HWND,
) -> RAWINPUTDEVICE {
    // Generic Desktop / Mouse.
    RAWINPUTDEVICE {
        usUsagePage: 0x01,
        usUsage: 0x02,
        dwFlags: flags,
        hwndTarget: hwnd,
    }
}

/// Ventana solo de mensajes que recibe Raw Input del mouse aunque no tenga el foco.
struct SinkWindow(HWND);

impl SinkWindow {
    fn create() -> Result<Self, CaptureError> {
        // SAFETY: "STATIC" es una clase del sistema; con padre HWND_MESSAGE la
        // ventana no se muestra ni recibe difusiones.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("tandem-capture"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .map_err(|e| os_error("CreateWindowExW", e))?;
        let window = SinkWindow(hwnd);
        let device = raw_mouse_device(RIDEV_INPUTSINK, hwnd);
        // SAFETY: arreglo de un elemento con el tamaño de su estructura.
        unsafe { RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32) }
            .map_err(|e| os_error("RegisterRawInputDevices", e))?;
        Ok(window)
    }
}

impl Drop for SinkWindow {
    fn drop(&mut self) {
        let device = raw_mouse_device(RIDEV_REMOVE, HWND::default());
        // SAFETY: igual que al registrar; RIDEV_REMOVE exige hwndTarget nulo.
        let _ = unsafe { RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32) };
        // SAFETY: ventana creada por este hilo.
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

fn metric_span(
    cx: ::windows::Win32::UI::WindowsAndMessaging::SYSTEM_METRICS_INDEX,
    cy: ::windows::Win32::UI::WindowsAndMessaging::SYSTEM_METRICS_INDEX,
) -> (f64, f64) {
    // SAFETY: consultas sin efectos.
    let (w, h) = unsafe { (GetSystemMetrics(cx), GetSystemMetrics(cy)) };
    (f64::from(w), f64::from(h))
}

/// Lee un WM_INPUT. Solo interesa el mouse.
fn read_raw(lparam: LPARAM) -> Option<RawMouse> {
    let mut raw = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    // SAFETY: el búfer es un RAWINPUT completo y `size` dice su tamaño; un
    // RAWINPUT de mouse siempre cabe.
    let n = unsafe {
        GetRawInputData(
            HRAWINPUT(lparam.0 as *mut _),
            RID_INPUT,
            Some(&mut raw as *mut RAWINPUT as *mut _),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if n == 0 || n == u32::MAX || raw.header.dwType != RIM_TYPEMOUSE.0 {
        return None;
    }
    // SAFETY: dwType == RIM_TYPEMOUSE: la variante válida de la unión es `mouse`.
    let mouse = unsafe { raw.data.mouse };
    let flags = mouse.usFlags.0;
    let span = if flags & consts::MOUSE_MOVE_ABSOLUTE == 0 {
        (0.0, 0.0)
    } else if flags & consts::MOUSE_VIRTUAL_DESKTOP != 0 {
        metric_span(SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN)
    } else {
        metric_span(SM_CXSCREEN, SM_CYSCREEN)
    };
    Some(RawMouse {
        device: raw.header.hDevice.0 as usize,
        flags,
        last_x: mouse.lLastX,
        last_y: mouse.lLastY,
        span,
    })
}

fn hook_thread(
    tx: Sender<HookEvent>,
    grab: Arc<AtomicBool>,
    reinstalls: Arc<AtomicU32>,
    ready: SyncSender<Result<u32, CaptureError>>,
) {
    let mut msg = MSG::default();
    // SAFETY: crea la cola de mensajes del hilo para que PostThreadMessageW no
    // falle antes del primer GetMessageW.
    let _ = unsafe { PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE) };
    // Este hilo casi siempre espera; cuando despierta, tiene que contestar ya.
    // SAFETY: seudo-handle del hilo actual.
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) };
    CTX.with(|c| {
        let _ = c.set(HookCtx {
            tx,
            grab,
            filter: RefCell::new(HookFilter::new()),
        });
    });

    let setup = SinkWindow::create().and_then(|w| Ok((w, Hooks::install()?)));
    let (window, hooks) = match setup {
        Ok(v) => v,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let mut hooks = Some(hooks);
    // SAFETY: sin parámetros.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));

    loop {
        // SAFETY: `msg` es un MSG propio y válido.
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 == 0 || r.0 == -1 {
            break;
        }
        if msg.hwnd.is_invalid() && msg.message == WM_REINSTALL {
            // Primero se quitan los viejos para no recibir cada evento dos veces.
            drop(hooks.take());
            hooks = Hooks::install().ok();
            reinstalls.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        if msg.message == WM_INPUT
            && let Some(raw) = read_raw(msg.lParam)
        {
            with_ctx(|ctx| {
                let _ = ctx.tx.send(HookEvent::Raw(raw));
                false
            });
        }
        // WM_INPUT también pasa por DefWindowProc, que libera sus datos.
        // SAFETY: mensaje recién sacado de la cola de este hilo.
        unsafe { DispatchMessageW(&msg) };
    }
    drop(hooks);
    drop(window);
    // El Sender de CTX se suelta al terminar el hilo y el de proceso sale.
}

/// Sondas y reinstalación, desde el hilo de proceso.
struct SysControl {
    hook_thread: u32,
}

impl HookControl for SysControl {
    fn probe(&mut self, kind: HookKind) {
        let input = match kind {
            // Soltar una tecla sin asignar: si nuestro hook ya no está, nadie la nota.
            HookKind::Keyboard => INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(consts::VK_PROBE),
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: PROBE_TAG,
                    },
                },
            },
            // Movimiento nulo: no mueve el cursor.
            HookKind::Mouse => INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_MOVE,
                        time: 0,
                        dwExtraInfo: PROBE_TAG,
                    },
                },
            },
        };
        // SAFETY: un INPUT bien formado y el tamaño de su estructura.
        unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    }

    fn reinstall(&mut self) {
        // SAFETY: mensaje sin punteros a un hilo con cola.
        let _ = unsafe { PostThreadMessageW(self.hook_thread, WM_REINSTALL, WPARAM(0), LPARAM(0)) };
    }
}

/// Captura de teclado y mouse de Windows.
///
/// Mientras no hay grab, todo pasa al sistema local y se informa igual (para
/// atajos, lo presionado y la posición del cursor). Con grab, la entrada local se
/// traga y el cursor queda quieto; los eventos siguen llegando a `sink`.
pub struct WindowsCapture {
    grab: Arc<AtomicBool>,
    reinstalls: Arc<AtomicU32>,
    hook_thread_id: u32,
    hook_thread: Option<JoinHandle<()>>,
    pipeline: Option<JoinHandle<()>>,
}

impl WindowsCapture {
    /// Arranca los dos hilos. `sink` corre en el hilo de proceso, nunca en el de
    /// los hooks: puede tardar sin poner en riesgo los hooks.
    pub fn start<F>(config: CaptureConfig, mut sink: F) -> Result<Self, CaptureError>
    where
        F: FnMut(InputEvent) + Send + 'static,
    {
        ensure_per_monitor_v2()?;
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let grab = Arc::new(AtomicBool::new(false));
        let reinstalls = Arc::new(AtomicU32::new(0));
        let hook_thread = std::thread::Builder::new()
            .name("tandem-hooks".into())
            .spawn({
                let grab = Arc::clone(&grab);
                let reinstalls = Arc::clone(&reinstalls);
                move || hook_thread(tx, grab, reinstalls, ready_tx)
            })?;
        let hook_thread_id = match ready_rx.recv() {
            Ok(Ok(id)) => id,
            Ok(Err(e)) => {
                let _ = hook_thread.join();
                return Err(e);
            }
            Err(_) => {
                let _ = hook_thread.join();
                return Err(CaptureError::ThreadDied);
            }
        };
        let mut capture = Self {
            grab,
            reinstalls,
            hook_thread_id,
            hook_thread: Some(hook_thread),
            pipeline: None,
        };
        // Si esto falla, el Drop de `capture` detiene el hilo de hooks.
        let pipeline = std::thread::Builder::new()
            .name("tandem-capture".into())
            .spawn(move || {
                let mut control = SysControl {
                    hook_thread: hook_thread_id,
                };
                run_pipeline(&rx, &mut control, &mut sink, &config);
            })?;
        capture.pipeline = Some(pipeline);
        Ok(capture)
    }

    /// `true`: tragar la entrada local (el foco está en la otra máquina).
    pub fn set_grab(&self, grab: bool) {
        self.grab.store(grab, Ordering::Relaxed);
    }

    pub fn is_grabbed(&self) -> bool {
        self.grab.load(Ordering::Relaxed)
    }

    /// Pone el cursor en `pos`, en píxeles físicos del escritorio virtual.
    pub fn warp(&self, pos: Point) -> Result<(), CaptureError> {
        // SAFETY: sin punteros; las coordenadas fuera de pantalla se recortan.
        unsafe { SetCursorPos(pos.x.round() as i32, pos.y.round() as i32) }
            .map_err(|e| os_error("SetCursorPos", e))
    }

    /// Cuántas veces el watchdog tuvo que reinstalar los hooks.
    pub fn reinstalls(&self) -> u32 {
        self.reinstalls.load(Ordering::Relaxed)
    }
}

impl Drop for WindowsCapture {
    fn drop(&mut self) {
        self.grab.store(false, Ordering::Relaxed);
        // SAFETY: mensaje sin punteros a un hilo con cola.
        let _ = unsafe { PostThreadMessageW(self.hook_thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(t) = self.hook_thread.take() {
            let _ = t.join();
        }
        // Sin el hilo de hooks el canal se cierra y el de proceso termina.
        if let Some(t) = self.pipeline.take() {
            let _ = t.join();
        }
    }
}
