//! File drag detector on Windows (T-121): an almost invisible drag target
//! window, flush with the crossing edge, registered with `RegisterDragDrop`.
//!
//! When the user drags files (CF_HDROP) with the button pressed up to the strip,
//! `IDropTarget::DragEnter` receives them and they end up in a [`DragProbe`] the agent queries
//! through [`DragDetector`]. The strip never accepts the drop (`NONE` effect): it only watches.
//!
//! The window, OLE and the message loop live on a thread of their own.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINTL, WPARAM};
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl, OleInitialize,
    OleUninitialize, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_TOPMOST,
    LWA_ALPHA, MSG, PostMessageW, PostQuitMessage, RegisterClassW, SW_HIDE, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, SetLayeredWindowAttributes, SetWindowPos, ShowWindow, TranslateMessage,
    WM_CLOSE, WM_DESTROY, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, Ref, Result as WinResult, w};
use windows_core::implement;

use crate::drag::DragDetector;
use crate::drag_os::{DragProbe, PxRect};
use crate::error::{FilesError, Result};

/// The CF_HDROP paths of `data`, or empty if it carries no files.
fn hdrop_paths(data: &IDataObject) -> Vec<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let fmt = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    // SAFETY: `fmt` is valid; the medium it returns is freed with `ReleaseStgMedium`.
    let Ok(mut medium) = (unsafe { data.GetData(&fmt) }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // SAFETY: with `TYMED_HGLOBAL` the union carries an HGLOBAL with a DROPFILES; it is locked
    // while being read and unlocked before freeing the medium.
    unsafe {
        let hglobal = medium.u.hGlobal;
        if !GlobalLock(hglobal).is_null() {
            let drop = HDROP(hglobal.0);
            let n = DragQueryFileW(drop, u32::MAX, None);
            for i in 0..n {
                let len = DragQueryFileW(drop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                let got = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
                out.push(PathBuf::from(std::ffi::OsString::from_wide(&buf[..got])));
            }
            let _ = GlobalUnlock(hglobal);
        }
        ReleaseStgMedium(&mut medium);
    }
    out
}

/// `IDropTarget` that only observes: records the files that enter and accepts nothing.
#[implement(IDropTarget)]
struct Target {
    probe: DragProbe,
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        data: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        if let Some(d) = data.as_ref() {
            self.probe.enter(hdrop_paths(d));
        }
        // SAFETY: OLE passes a valid pointer to write the effect.
        unsafe { *effect = DROPEFFECT_NONE };
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        // SAFETY: OLE passes a valid pointer to write the effect.
        unsafe { *effect = DROPEFFECT_NONE };
        Ok(())
    }

    fn DragLeave(&self) -> WinResult<()> {
        self.probe.leave();
        Ok(())
    }

    fn Drop(
        &self,
        _data: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        self.probe.leave();
        // SAFETY: OLE passes a valid pointer to write the effect.
        unsafe { *effect = DROPEFFECT_NONE };
        Ok(())
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CLOSE => {
            // SAFETY: `hwnd` is this thread's window.
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: the target of this window is unregistered and the loop ends.
            unsafe {
                let _ = RevokeDragDrop(hwnd);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        // SAFETY: forwarding of the same arguments the procedure received.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Creates the window and registers the target; returns the HWND as an integer to pass it
/// between threads.
fn create_window(probe: &DragProbe) -> WinResult<isize> {
    let class = w!("TandemDragEdge");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        lpszClassName: class,
        ..Default::default()
    };
    // SAFETY: `wc` is valid; registering the class twice only returns 0 and is ignored.
    unsafe { RegisterClassW(&wc) };
    // SAFETY: class registered above; no parent or menu. WS_EX_NOACTIVATE and TOOLWINDOW
    // avoid stealing focus and appearing in the taskbar.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            class,
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            None,
            None,
        )?
    };
    // Alpha 1/255: invisible to the eye but still receives the drag (with 0 the
    // hit test can pass through it).
    // SAFETY: `hwnd` just created on this thread.
    unsafe { SetLayeredWindowAttributes(hwnd, Default::default(), 1, LWA_ALPHA)? };
    let target: IDropTarget = Target {
        probe: probe.clone(),
    }
    .into();
    // SAFETY: `hwnd` belongs to this thread and OLE is initialized on it.
    unsafe { RegisterDragDrop(hwnd, &target)? };
    Ok(hwnd.0 as isize)
}

fn thread_main(probe: &DragProbe, ready: &mpsc::Sender<WinResult<isize>>) {
    // SAFETY: OLE (apartment) on this thread; closed on leaving the loop.
    if let Err(e) = unsafe { OleInitialize(None) } {
        let _ = ready.send(Err(e));
        return;
    }
    match create_window(probe) {
        Ok(h) => {
            let _ = ready.send(Ok(h));
            let mut msg = MSG::default();
            // SAFETY: standard loop; `GetMessageW` returns 0 with WM_QUIT and -1 if it fails.
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
                // SAFETY: `msg` was filled by `GetMessageW`.
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
        Err(e) => {
            let _ = ready.send(Err(e));
        }
    }
    // SAFETY: pairs with the `OleInitialize` above.
    unsafe { OleUninitialize() };
}

fn failed(e: &windows::core::Error) -> FilesError {
    FilesError::io(Path::new(""), std::io::Error::other(e.to_string()))
}

/// The target window on the crossing edge. Dropping it closes the window and the thread.
pub struct WindowsDropWatcher {
    probe: DragProbe,
    hwnd: AtomicIsize,
    thread: Option<JoinHandle<()>>,
}

impl WindowsDropWatcher {
    /// Starts the thread with the window (hidden until [`Self::set_strip`]).
    pub fn new() -> Result<Self> {
        let probe = DragProbe::default();
        let (tx, rx) = mpsc::channel();
        let p = probe.clone();
        let thread = std::thread::Builder::new()
            .name("tandem-drag-edge".into())
            .spawn(move || thread_main(&p, &tx))
            .map_err(|e| FilesError::io(Path::new(""), e))?;
        let hwnd = rx
            .recv()
            .map_err(|_| FilesError::Join)?
            .map_err(|e| failed(&e))?;
        Ok(Self {
            probe,
            hwnd: AtomicIsize::new(hwnd),
            thread: Some(thread),
        })
    }

    fn hwnd(&self) -> HWND {
        HWND(self.hwnd.load(Ordering::Relaxed) as *mut _)
    }

    /// Places the strip at `rect` (screen pixels) or hides it with `None`.
    pub fn set_strip(&self, rect: Option<PxRect>) {
        let hwnd = self.hwnd();
        // SAFETY: `hwnd` lives until `Drop`; the system dispatches these calls to the thread
        // that owns the window.
        unsafe {
            match rect {
                Some((x, y, w, h)) => {
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_TOPMOST),
                        x,
                        y,
                        w,
                        h,
                        SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                }
                None => {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    self.probe.leave();
                }
            }
        }
    }
}

impl DragDetector for WindowsDropWatcher {
    fn file_drag(&self) -> Option<Vec<PathBuf>> {
        self.probe.file_drag()
    }
}

impl Drop for WindowsDropWatcher {
    fn drop(&mut self) {
        // SAFETY: asks to close the window of our thread, which ends its loop.
        unsafe {
            let _ = PostMessageW(Some(self.hwnd()), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
