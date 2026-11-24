//! Synthetic drag on Windows (T-121): `DoDragDrop` with an `IDataObject` of files
//! (CF_HDROP, built by the shell from the temporary folder's paths) and an `IDropSource`
//! of its own. The system does the copy on drop; this only waits for how it ends.
//!
//! `DoDragDrop` blocks and requires a thread with OLE initialized: it goes on a separate thread.

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

/// `IDropSource`: Esc cancels, releasing the left button drops.
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

/// An absolute shell PIDL; freed when dropped.
struct Pidl(*mut ITEMIDLIST);

impl Drop for Pidl {
    fn drop(&mut self) {
        // SAFETY: the shell allocated it with the COM allocator in `SHParseDisplayName` and
        // it is freed here only once.
        unsafe { CoTaskMemFree(Some(self.0.cast_const().cast())) };
    }
}

fn parse(path: &std::path::Path) -> WinResult<Pidl> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    // SAFETY: `wide` ends in null and lives during the call; `pidl` is a valid output.
    unsafe { SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None)? };
    Ok(Pidl(pidl))
}

/// The shell's `IDataObject` with CF_HDROP of `paths` (absolute paths).
fn data_object(paths: &[PathBuf]) -> WinResult<IDataObject> {
    let pidls: Vec<Pidl> = paths.iter().map(|p| parse(p)).collect::<WinResult<_>>()?;
    let raw: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.0.cast_const()).collect();
    // SAFETY: `raw` has valid absolute PIDLs that `pidls` keeps alive until the end.
    unsafe { SHCreateDataObject(None, Some(&raw), None) }
}

/// Result of `DoDragDrop` with the accepted effects.
fn run(data: &IDataObject) -> WinResult<DragEnd> {
    let source: IDropSource = Source.into();
    let mut effect = DROPEFFECT_NONE;
    // SAFETY: `data` and `source` are live COM objects and `effect` a valid output; the thread
    // has OLE initialized (done by `drag_blocking`).
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
    // SAFETY: OLE is initialized on this thread and closed at the end, even if what is in between fails.
    unsafe { OleInitialize(None)? };
    let end = data_object(staged).and_then(|d| run(&d));
    // SAFETY: pairs with the `OleInitialize` above.
    unsafe { OleUninitialize() };
    end
}

/// Windows [`DragStarter`].
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
