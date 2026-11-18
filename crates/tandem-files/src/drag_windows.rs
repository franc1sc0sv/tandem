//! Arrastre sintético en Windows (T-121): `DoDragDrop` con un `IDataObject` de archivos
//! (CF_HDROP, que arma el shell desde las rutas de la carpeta temporal) y un `IDropSource`
//! propio. El sistema hace la copia al soltar; esto solo espera cómo termina.
//!
//! `DoDragDrop` bloquea y exige un hilo con OLE iniciado: va en un hilo aparte.

use std::path::PathBuf;

use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, S_OK,
};
use windows::Win32::System::Com::{CoTaskMemFree, IDataObject};
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, DoDragDrop, IDropSource, IDropSource_Impl,
    OleInitialize, OleUninitialize,
};
use windows::Win32::System::SystemServices::MK_LBUTTON;
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{SHCreateDataObject, SHParseDisplayName};
use windows::core::{BOOL, HRESULT, PCWSTR, Result as WinResult, implement};

use crate::drag::{DragEnd, DragStarter};
use crate::drag_os::{DropAction, drop_action, end_of};
use crate::error::{FilesError, Result};

/// `IDropSource`: Esc cancela, soltar el botón izquierdo suelta.
#[implement(IDropSource)]
struct Source;

impl IDropSource_Impl for Source_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        match drop_action(escape.as_bool(), keys.0 & MK_LBUTTON.0 != 0) {
            DropAction::Continue => S_OK,
            DropAction::Drop => DRAGDROP_S_DROP,
            DropAction::Cancel => DRAGDROP_S_CANCEL,
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Un PIDL absoluto del shell; se libera al soltarlo.
struct Pidl(*mut ITEMIDLIST);

impl Drop for Pidl {
    fn drop(&mut self) {
        // SAFETY: el shell lo reservó con el asignador de COM en `SHParseDisplayName` y
        // aquí se libera una sola vez.
        unsafe { CoTaskMemFree(Some(self.0.cast_const().cast())) };
    }
}

fn parse(path: &std::path::Path) -> WinResult<Pidl> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    // SAFETY: `wide` termina en nulo y vive durante la llamada; `pidl` es una salida válida.
    unsafe { SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None)? };
    Ok(Pidl(pidl))
}

/// El `IDataObject` del shell con CF_HDROP de `paths` (rutas absolutas).
fn data_object(paths: &[PathBuf]) -> WinResult<IDataObject> {
    let pidls: Vec<Pidl> = paths.iter().map(|p| parse(p)).collect::<WinResult<_>>()?;
    let raw: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.0.cast_const()).collect();
    // SAFETY: `raw` tiene PIDLs absolutos válidos que `pidls` mantiene vivos hasta el final.
    unsafe { SHCreateDataObject(None, Some(&raw), None) }
}

/// Resultado de `DoDragDrop` con los efectos aceptados.
fn run(data: &IDataObject) -> WinResult<DragEnd> {
    let source: IDropSource = Source.into();
    let mut effect = DROPEFFECT_NONE;
    // SAFETY: `data` y `source` son objetos COM vivos y `effect` una salida válida; el hilo
    // tiene OLE iniciado (lo hace `drag_blocking`).
    let hr = unsafe { DoDragDrop(data, &source, DROPEFFECT_COPY, &mut effect) };
    if hr == DRAGDROP_S_DROP {
        Ok(end_of(true, effect != DROPEFFECT_NONE))
    } else if hr == DRAGDROP_S_CANCEL {
        Ok(DragEnd::Cancelled)
    } else {
        hr.ok().map(|()| DragEnd::Cancelled)
    }
}

fn drag_blocking(staged: &[PathBuf]) -> WinResult<DragEnd> {
    // SAFETY: se inicia OLE en este hilo y se cierra al terminar, aunque falle lo de en medio.
    unsafe { OleInitialize(None)? };
    let end = data_object(staged).and_then(|d| run(&d));
    // SAFETY: empareja el `OleInitialize` de arriba.
    unsafe { OleUninitialize() };
    end
}

/// [`DragStarter`] de Windows.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsDragStarter;

impl DragStarter for WindowsDragStarter {
    async fn start(&self, staged: Vec<PathBuf>) -> Result<DragEnd> {
        let first = staged.first().cloned().unwrap_or_default();
        tokio::task::spawn_blocking(move || drag_blocking(&staged))
            .await
            .map_err(|_| FilesError::Join)?
            .map_err(|e| FilesError::io(&first, std::io::Error::other(e.to_string())))
    }
}
