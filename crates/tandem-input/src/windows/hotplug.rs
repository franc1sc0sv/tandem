//! Display changes on Windows (T-349): a hidden top-level window of its own thread receives
//! `WM_DISPLAYCHANGE` (broadcast to top-level windows only, so not a message-only window) and,
//! with `RegisterDeviceNotificationW` for the monitor interface class, the `WM_DEVICECHANGE` of a
//! monitor that arrives or leaves.

use std::cell::RefCell;
use std::sync::mpsc;
use std::thread::JoinHandle;

use ::windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::System::Threading::GetCurrentThreadId;
use ::windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DBT_DEVTYP_DEVICEINTERFACE, DEV_BROADCAST_DEVICEINTERFACE_W,
    DEVICE_NOTIFY_WINDOW_HANDLE, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    HDEVNOTIFY, MSG, PostThreadMessageW, RegisterClassW, RegisterDeviceNotificationW,
    UnregisterDeviceNotification, WINDOW_EX_STYLE, WM_DEVICECHANGE, WM_DISPLAYCHANGE, WM_QUIT,
    WNDCLASSW, WS_OVERLAPPED,
};
use ::windows::core::{GUID, w};

use crate::hotplug::{DisplayChanges, OnChange, Watch};

/// `GUID_DEVINTERFACE_MONITOR` (devguid.h).
const MONITOR_INTERFACE: GUID = GUID::from_u128(0xe6f07b5f_ee97_4a90_b076_33f57bf4eaa7);
/// `DBT_DEVICEARRIVAL` and `DBT_DEVICEREMOVECOMPLETE`.
const ARRIVAL: usize = 0x8000;
const REMOVE_COMPLETE: usize = 0x8004;

thread_local! {
    /// The callback of this thread's window (one window per watch thread).
    static ON_CHANGE: RefCell<Option<OnChange>> = const { RefCell::new(None) };
}

fn fire() {
    let cb = ON_CHANGE.with(|c| c.borrow().clone());
    if let Some(cb) = cb {
        cb();
    }
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_DISPLAYCHANGE => fire(),
        WM_DEVICECHANGE if matches!(wparam.0, ARRIVAL | REMOVE_COMPLETE) => fire(),
        _ => {}
    }
    // SAFETY: the parameters are the ones Windows passed to this window procedure.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// The hidden window and its monitor notification, destroyed in reverse order.
struct Window {
    hwnd: HWND,
    notify: Option<HDEVNOTIFY>,
}

impl Window {
    fn create() -> Result<Self, String> {
        // SAFETY: no arguments; the module of this process.
        let instance =
            unsafe { GetModuleHandleW(None) }.map_err(|e| format!("GetModuleHandleW: {e}"))?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("tandem-hotplug"),
            ..Default::default()
        };
        // SAFETY: a complete class whose name is a static string. A second registration (another
        // watch) fails with "class already exists", which is fine: the class is the same.
        unsafe { RegisterClassW(&class) };
        // SAFETY: the class registered above; the window is never shown.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("tandem-hotplug"),
                w!("tandem-hotplug"),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )
        }
        .map_err(|e| format!("CreateWindowExW: {e}"))?;
        let filter = DEV_BROADCAST_DEVICEINTERFACE_W {
            dbcc_size: size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32,
            dbcc_devicetype: DBT_DEVTYP_DEVICEINTERFACE.0,
            dbcc_classguid: MONITOR_INTERFACE,
            ..Default::default()
        };
        // SAFETY: `filter` is a complete structure with its size; Windows copies it.
        let notify = unsafe {
            RegisterDeviceNotificationW(
                HANDLE(hwnd.0),
                (&raw const filter).cast(),
                DEVICE_NOTIFY_WINDOW_HANDLE,
            )
        };
        // Without it, WM_DISPLAYCHANGE still arrives.
        let notify = notify
            .map_err(|e| tracing::debug!("RegisterDeviceNotificationW: {e}"))
            .ok();
        Ok(Window { hwnd, notify })
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        if let Some(n) = self.notify.take() {
            // SAFETY: the handle registered in `create`.
            let _ = unsafe { UnregisterDeviceNotification(n) };
        }
        // SAFETY: window created by this thread.
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

fn run(on_change: OnChange, ready: mpsc::Sender<Result<u32, String>>) {
    ON_CHANGE.with(|c| *c.borrow_mut() = Some(on_change));
    let window = match Window::create() {
        Ok(w) => w,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    // SAFETY: no arguments.
    let thread = unsafe { GetCurrentThreadId() };
    if ready.send(Ok(thread)).is_err() {
        return;
    }
    let mut msg = MSG::default();
    // SAFETY: `msg` is valid for each call; a WM_QUIT posted by `Drop` ends the loop (0) and an
    // error (-1) ends it too.
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        // SAFETY: a message just read by GetMessageW.
        unsafe { DispatchMessageW(&msg) };
    }
    drop(window);
}

/// Ends the window's thread when dropped.
struct ThreadWatch {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ThreadWatch {
    fn drop(&mut self) {
        // SAFETY: a thread of this process with a message queue (it created a window).
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The system watcher on Windows.
#[derive(Debug, Clone, Copy)]
pub struct DisplayMessages;

impl DisplayChanges for DisplayMessages {
    fn watch(&self, on_change: OnChange) -> Result<Watch, String> {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("tandem-hotplug".into())
            .spawn(move || run(on_change, tx))
            .map_err(|e| format!("could not start the display watch: {e}"))?;
        let thread_id = rx
            .recv()
            .map_err(|_| "the display watch ended at once".to_owned())??;
        Ok(Box::new(ThreadWatch {
            thread_id,
            thread: Some(thread),
        }))
    }
}
