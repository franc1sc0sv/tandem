//! What touches Win32: hook thread, Raw Input and DPI.

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
use ::windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTDEVICE_FLAGS, RAWINPUTHEADER,
    RID_INPUT, RIDEV_INPUTSINK, RIDEV_REMOVE, RIM_TYPEKEYBOARD, RIM_TYPEMOUSE,
    RegisterRawInputDevices,
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
    CaptureConfig, CaptureError, HookControl, HookEvent, HookFilter, KeyboardHook, MouseHook,
    RawMouse, Verdict, hook_step, run_pipeline,
};
use crate::event::InputEvent;
use crate::geometry::Point;

// The portable constants have to be those of Windows.
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

/// Message to the hook thread: reinstall them.
const WM_REINSTALL: u32 = WM_APP + 1;

fn os_error(op: &'static str, e: ::windows::core::Error) -> CaptureError {
    CaptureError::Os {
        op,
        code: e.code().0 as u32,
        message: e.message(),
    }
}

/// Declares the process Per-Monitor-V2 DPI aware: that way the hook's positions,
/// `SetCursorPos` and absolute injection use physical pixels on all
/// monitors. If it already was (by manifest or an earlier call), it does nothing.
pub fn ensure_per_monitor_v2() -> Result<(), CaptureError> {
    // SAFETY: it only changes the process's DPI mode; it fails if it was already set.
    let set = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    // SAFETY: queries without effects on the current thread's context.
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

/// State of the hook thread, accessible from the callbacks.
struct HookCtx {
    tx: Sender<HookEvent>,
    grab: Arc<AtomicBool>,
    filter: RefCell<HookFilter>,
}

impl HookCtx {
    /// Decides and queues. Returns `true` if the event has to be swallowed.
    fn handle(&self, decide: impl FnOnce(&mut HookFilter, bool) -> (Verdict, HookEvent)) -> bool {
        let grab = self.grab.load(Ordering::Relaxed);
        // The callbacks are not reentered (we do not pump messages inside), but a
        // panic here would abort the process: when in doubt, let it pass.
        let Ok(mut filter) = self.filter.try_borrow_mut() else {
            return false;
        };
        hook_step(&mut filter, grab, &self.tx, decide)
    }
}

thread_local! {
    static CTX: OnceCell<HookCtx> = const { OnceCell::new() };
}

fn with_ctx(f: impl FnOnce(&HookCtx) -> bool) -> bool {
    CTX.try_with(|c| c.get().is_some_and(f)).unwrap_or(false)
}

// SAFETY: the signature matches `HOOKPROC`; it is only invoked by Windows on the thread that installed
// the hook, with a valid `lparam` when `code` is HC_ACTION.
unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: with HC_ACTION, `lparam` points to a valid KBDLLHOOKSTRUCT
        // during the call; it is copied right away.
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
                let event = HookEvent::Keyboard {
                    hook,
                    swallowed: verdict == Verdict::Swallow,
                };
                (verdict, event)
            })
        });
        if swallow {
            return LRESULT(1);
        }
    }
    // SAFETY: the received parameters are passed as is to the next hook.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: with HC_ACTION, `lparam` points to a valid MSLLHOOKSTRUCT
        // during the call; it is copied right away.
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
                let event = HookEvent::Mouse {
                    hook,
                    swallowed: verdict == Verdict::Swallow,
                };
                (verdict, event)
            })
        });
        if swallow {
            return LRESULT(1);
        }
    }
    // SAFETY: the received parameters are passed as is to the next hook.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// The two installed hooks; they are removed when the value is dropped.
struct Hooks {
    keyboard: HHOOK,
    mouse: HHOOK,
}

fn unhook(hook: HHOOK) {
    // SAFETY: handle returned by SetWindowsHookExW on this thread. If Windows already
    // removed it, the call fails without effect.
    let _ = unsafe { UnhookWindowsHookEx(hook) };
}

impl Hooks {
    fn install() -> Result<Self, CaptureError> {
        // SAFETY: queries the executable's module, without effects.
        let module = unsafe { GetModuleHandleW(PCWSTR::null()) }
            .map_err(|e| os_error("GetModuleHandleW", e))?;
        let instance = HINSTANCE::from(module);
        // SAFETY: `keyboard_proc` and `mouse_proc` have the signature of HOOKPROC and
        // exist as long as the program lives.
        let keyboard =
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(instance), 0) }
                .map_err(|e| os_error("SetWindowsHookExW(WH_KEYBOARD_LL)", e))?;
        // SAFETY: the same as the keyboard one.
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

/// Generic Desktop / Mouse (deltas and watchdog) and Keyboard (watchdog only).
fn raw_devices(flags: RAWINPUTDEVICE_FLAGS, hwnd: HWND) -> [RAWINPUTDEVICE; 2] {
    [0x02, 0x06].map(|usage| RAWINPUTDEVICE {
        usUsagePage: 0x01,
        usUsage: usage,
        dwFlags: flags,
        hwndTarget: hwnd,
    })
}

/// Message-only window that receives mouse and keyboard Raw Input even if it does not
/// have focus.
struct SinkWindow(HWND);

impl SinkWindow {
    fn create() -> Result<Self, CaptureError> {
        // SAFETY: "STATIC" is a system class; with parent HWND_MESSAGE the
        // window is neither shown nor receives broadcasts.
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
        let devices = raw_devices(RIDEV_INPUTSINK, hwnd);
        // SAFETY: array of structures with the size of each.
        unsafe { RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32) }
            .map_err(|e| os_error("RegisterRawInputDevices", e))?;
        Ok(window)
    }
}

impl Drop for SinkWindow {
    fn drop(&mut self) {
        let devices = raw_devices(RIDEV_REMOVE, HWND::default());
        // SAFETY: the same as when registering; RIDEV_REMOVE requires a null hwndTarget.
        let _ = unsafe { RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32) };
        // SAFETY: window created by this thread.
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

fn metric_span(
    cx: ::windows::Win32::UI::WindowsAndMessaging::SYSTEM_METRICS_INDEX,
    cy: ::windows::Win32::UI::WindowsAndMessaging::SYSTEM_METRICS_INDEX,
) -> (f64, f64) {
    // SAFETY: queries without effects.
    let (w, h) = unsafe { (GetSystemMetrics(cx), GetSystemMetrics(cy)) };
    (f64::from(w), f64::from(h))
}

/// Reads a WM_INPUT. For the keyboard only which device it came from matters.
fn read_raw(lparam: LPARAM) -> Option<HookEvent> {
    let mut raw = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    // SAFETY: the buffer is a complete RAWINPUT and `size` says its size; a
    // mouse or keyboard RAWINPUT always fits.
    let n = unsafe {
        GetRawInputData(
            HRAWINPUT(lparam.0 as *mut _),
            RID_INPUT,
            Some(&mut raw as *mut RAWINPUT as *mut _),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if n == 0 || n == u32::MAX {
        return None;
    }
    let device = raw.header.hDevice.0 as usize;
    if raw.header.dwType == RIM_TYPEKEYBOARD.0 {
        return Some(HookEvent::RawKeyboard { device });
    }
    if raw.header.dwType != RIM_TYPEMOUSE.0 {
        return None;
    }
    // SAFETY: dwType == RIM_TYPEMOUSE: the valid variant of the union is `mouse`.
    let mouse = unsafe { raw.data.mouse };
    let flags = mouse.usFlags.0;
    let span = if flags & consts::MOUSE_MOVE_ABSOLUTE == 0 {
        (0.0, 0.0)
    } else if flags & consts::MOUSE_VIRTUAL_DESKTOP != 0 {
        metric_span(SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN)
    } else {
        metric_span(SM_CXSCREEN, SM_CYSCREEN)
    };
    Some(HookEvent::Raw(RawMouse {
        device,
        flags,
        last_x: mouse.lLastX,
        last_y: mouse.lLastY,
        span,
    }))
}

fn hook_thread(
    tx: Sender<HookEvent>,
    grab: Arc<AtomicBool>,
    reinstalls: Arc<AtomicU32>,
    ready: SyncSender<Result<u32, CaptureError>>,
) {
    let mut msg = MSG::default();
    // SAFETY: creates the thread's message queue so that PostThreadMessageW does not
    // fail before the first GetMessageW.
    let _ = unsafe { PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE) };
    // This thread almost always waits; when it wakes up, it has to answer at once.
    // SAFETY: pseudo-handle of the current thread.
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
    // SAFETY: no parameters.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));

    loop {
        // SAFETY: `msg` is an own, valid MSG.
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 == 0 || r.0 == -1 {
            break;
        }
        if msg.hwnd.is_invalid() && msg.message == WM_REINSTALL {
            // The old ones are removed first so as not to receive each event twice.
            drop(hooks.take());
            hooks = Hooks::install().ok();
            reinstalls.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        if msg.message == WM_INPUT
            && let Some(event) = read_raw(msg.lParam)
        {
            with_ctx(|ctx| {
                let _ = ctx.tx.send(event);
                false
            });
        }
        // WM_INPUT also goes through DefWindowProc, which frees its data.
        // SAFETY: message just taken from this thread's queue.
        unsafe { DispatchMessageW(&msg) };
    }
    drop(hooks);
    drop(window);
    // CTX's Sender is dropped when the thread ends and the process one exits.
}

/// Reinstalling the hooks, from the process thread.
struct SysControl {
    hook_thread: u32,
}

impl HookControl for SysControl {
    fn reinstall(&mut self) {
        // SAFETY: message without pointers to a thread with a queue.
        let _ = unsafe { PostThreadMessageW(self.hook_thread, WM_REINSTALL, WPARAM(0), LPARAM(0)) };
    }
}

/// Windows keyboard and mouse capture.
///
/// While there is no grab, everything passes to the local system and is reported anyway (for
/// shortcuts, what is pressed and the cursor position). With a grab, local input is
/// swallowed and the cursor stays still; events keep reaching `sink`.
pub struct WindowsCapture {
    grab: Arc<AtomicBool>,
    reinstalls: Arc<AtomicU32>,
    hook_thread_id: u32,
    hook_thread: Option<JoinHandle<()>>,
    pipeline: Option<JoinHandle<()>>,
}

impl WindowsCapture {
    /// Starts the two threads. `sink` runs on the process thread, never on the hooks one:
    /// it can take long without putting the hooks at risk.
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
        // If this fails, `capture`'s Drop stops the hook thread.
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

    /// `true`: swallow local input (the focus is on the other machine).
    pub fn set_grab(&self, grab: bool) {
        self.grab.store(grab, Ordering::Relaxed);
    }

    pub fn is_grabbed(&self) -> bool {
        self.grab.load(Ordering::Relaxed)
    }

    /// Puts the cursor at `pos`, in physical pixels of the virtual desktop.
    pub fn warp(&self, pos: Point) -> Result<(), CaptureError> {
        // SAFETY: no pointers; coordinates outside the screen are clipped.
        unsafe { SetCursorPos(pos.x.round() as i32, pos.y.round() as i32) }
            .map_err(|e| os_error("SetCursorPos", e))
    }

    /// How many times the watchdog had to reinstall the hooks.
    pub fn reinstalls(&self) -> u32 {
        self.reinstalls.load(Ordering::Relaxed)
    }
}

impl Drop for WindowsCapture {
    fn drop(&mut self) {
        self.grab.store(false, Ordering::Relaxed);
        // SAFETY: message without pointers to a thread with a queue.
        let _ = unsafe { PostThreadMessageW(self.hook_thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(t) = self.hook_thread.take() {
            let _ = t.join();
        }
        // Without the hook thread the channel closes and the process one ends.
        if let Some(t) = self.pipeline.take() {
            let _ = t.join();
        }
    }
}
