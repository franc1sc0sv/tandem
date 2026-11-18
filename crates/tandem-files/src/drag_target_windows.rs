//! Detector de arrastre de archivos en Windows (T-121): una ventana destino de arrastre
//! casi invisible, pegada al borde de cruce, registrada con `RegisterDragDrop`.
//!
//! Cuando el usuario arrastra archivos (CF_HDROP) con el botón apretado hasta la franja,
//! `IDropTarget::DragEnter` los recibe y quedan en un [`DragProbe`] que el agente consulta
//! por [`DragDetector`]. La franja nunca acepta el soltar (efecto `NONE`): solo mira.
//!
//! La ventana, OLE y el bucle de mensajes viven en un hilo propio.

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

/// Las rutas de CF_HDROP de `data`, o vacío si no trae archivos.
fn hdrop_paths(data: &IDataObject) -> Vec<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let fmt = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    // SAFETY: `fmt` es válido; el medio que devuelve se libera con `ReleaseStgMedium`.
    let Ok(mut medium) = (unsafe { data.GetData(&fmt) }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // SAFETY: con `TYMED_HGLOBAL` la unión trae un HGLOBAL con un DROPFILES; se bloquea
    // mientras se lee y se desbloquea antes de liberar el medio.
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

/// `IDropTarget` que solo observa: anota los archivos que entran y no acepta nada.
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
        // SAFETY: OLE pasa un puntero válido para escribir el efecto.
        unsafe { *effect = DROPEFFECT_NONE };
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        // SAFETY: OLE pasa un puntero válido para escribir el efecto.
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
        // SAFETY: OLE pasa un puntero válido para escribir el efecto.
        unsafe { *effect = DROPEFFECT_NONE };
        Ok(())
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CLOSE => {
            // SAFETY: `hwnd` es la ventana de este hilo.
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: se desregistra el destino de esta ventana y se termina el bucle.
            unsafe {
                let _ = RevokeDragDrop(hwnd);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        // SAFETY: reenvío de los mismos argumentos que recibió el procedimiento.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Crea la ventana y registra el destino; devuelve el HWND como entero para pasarlo
/// entre hilos.
fn create_window(probe: &DragProbe) -> WinResult<isize> {
    let class = w!("TandemDragEdge");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        lpszClassName: class,
        ..Default::default()
    };
    // SAFETY: `wc` es válido; registrar dos veces la clase solo devuelve 0 y se ignora.
    unsafe { RegisterClassW(&wc) };
    // SAFETY: clase registrada arriba; sin padre ni menú. WS_EX_NOACTIVATE y TOOLWINDOW
    // evitan robar el foco y salir en la barra de tareas.
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
    // Alfa 1/255: invisible a la vista pero que sigue recibiendo el arrastre (con 0 la
    // prueba de impacto la puede atravesar).
    // SAFETY: `hwnd` recién creada en este hilo.
    unsafe { SetLayeredWindowAttributes(hwnd, Default::default(), 1, LWA_ALPHA)? };
    let target: IDropTarget = Target {
        probe: probe.clone(),
    }
    .into();
    // SAFETY: `hwnd` es de este hilo y OLE está iniciado en él.
    unsafe { RegisterDragDrop(hwnd, &target)? };
    Ok(hwnd.0 as isize)
}

fn thread_main(probe: &DragProbe, ready: &mpsc::Sender<WinResult<isize>>) {
    // SAFETY: OLE (apartamento) en este hilo; se cierra al salir del bucle.
    if let Err(e) = unsafe { OleInitialize(None) } {
        let _ = ready.send(Err(e));
        return;
    }
    match create_window(probe) {
        Ok(h) => {
            let _ = ready.send(Ok(h));
            let mut msg = MSG::default();
            // SAFETY: bucle estándar; `GetMessageW` devuelve 0 con WM_QUIT y -1 si falla.
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
                // SAFETY: `msg` lo llenó `GetMessageW`.
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
    // SAFETY: empareja el `OleInitialize` de arriba.
    unsafe { OleUninitialize() };
}

fn failed(e: &windows::core::Error) -> FilesError {
    FilesError::io(Path::new(""), std::io::Error::other(e.to_string()))
}

/// La ventana destino en el borde de cruce. Al soltarla se cierra la ventana y el hilo.
pub struct WindowsDropWatcher {
    probe: DragProbe,
    hwnd: AtomicIsize,
    thread: Option<JoinHandle<()>>,
}

impl WindowsDropWatcher {
    /// Arranca el hilo con la ventana (oculta hasta [`Self::set_strip`]).
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

    /// Coloca la franja en `rect` (píxeles de pantalla) o la oculta con `None`.
    pub fn set_strip(&self, rect: Option<PxRect>) {
        let hwnd = self.hwnd();
        // SAFETY: `hwnd` vive hasta el `Drop`; el sistema despacha estas llamadas al hilo
        // dueño de la ventana.
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
        // SAFETY: pide cerrar la ventana de nuestro hilo, que termina su bucle.
        unsafe {
            let _ = PostMessageW(Some(self.hwnd()), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
