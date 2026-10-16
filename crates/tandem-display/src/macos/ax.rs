//! [`WindowApi`] real: API de Accesibilidad (`AXUIElement`) por FFI directo.
//!
//! - Qué aplicaciones tienen ventanas: `CGWindowListCopyWindowInfo` (capa 0, la de las
//!   ventanas normales). Así no hace falta AppKit para recorrer las aplicaciones.
//! - Las ventanas de cada una: `AXWindows` de `AXUIElementCreateApplication(pid)`, con su
//!   `AXPosition`, `AXSize`, `AXMinimized` y `AXTitle`.
//! - Mover y redimensionar: `AXUIElementSetAttributeValue` con un `AXValue`.
//!
//! Requiere el permiso de Accesibilidad (el mismo que pide la captura de entrada, S7).
//! Las coordenadas de AX son las del escritorio global de Quartz, las mismas que
//! `CGDisplayBounds`.

use std::collections::BTreeMap;
use std::ffi::{CStr, c_char, c_void};

use crate::gather::{GatherError, GatherReport, Window, WindowApi, WindowId};
use crate::quartz::{Bounds, QuartzApi};

use super::CgApi;

type CfTypeRef = *const c_void;
type AxError = i32;

const AX_SUCCESS: AxError = 0;
/// `kAXErrorAPIDisabled`: el proceso no tiene permiso de Accesibilidad.
const AX_API_DISABLED: AxError = -25211;
/// `kAXErrorNoValue`: el atributo existe pero está vacío (ventana sin título, etc.).
const AX_NO_VALUE: AxError = -25212;
const AX_VALUE_CG_POINT: u32 = 1;
const AX_VALUE_CG_SIZE: u32 = 2;
/// Una app colgada no debe colgar "traer ventanas" (el predeterminado son 6 s).
const AX_TIMEOUT_SECS: f32 = 1.0;

/// `kCGWindowListOptionAll | kCGWindowListExcludeDesktopElements`: también las de otros
/// Spaces y las minimizadas, sin el escritorio.
const CG_WINDOW_LIST_OPTIONS: u32 = 1 << 4;
const CG_NULL_WINDOW: u32 = 0;
const CF_NUMBER_SINT32: isize = 3;
const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CgPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CgSize {
    width: f64,
    height: f64,
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXUIElementCreateApplication(pid: i32) -> CfTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: CfTypeRef,
        attribute: CfTypeRef,
        value: *mut CfTypeRef,
    ) -> AxError;
    fn AXUIElementSetAttributeValue(
        element: CfTypeRef,
        attribute: CfTypeRef,
        value: CfTypeRef,
    ) -> AxError;
    fn AXUIElementSetMessagingTimeout(element: CfTypeRef, seconds: f32) -> AxError;
    fn AXValueCreate(kind: u32, value: *const c_void) -> CfTypeRef;
    fn AXValueGetValue(value: CfTypeRef, kind: u32, out: *mut c_void) -> u8;
    fn AXValueGetTypeID() -> usize;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CfTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(alloc: CfTypeRef, s: *const c_char, encoding: u32) -> CfTypeRef;
    fn CFStringGetCString(s: CfTypeRef, buf: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFStringGetLength(s: CfTypeRef) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFArrayGetCount(array: CfTypeRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CfTypeRef, index: isize) -> CfTypeRef;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFDictionaryGetValue(dict: CfTypeRef, key: CfTypeRef) -> CfTypeRef;
    fn CFNumberGetTypeID() -> usize;
    fn CFNumberGetValue(number: CfTypeRef, kind: isize, out: *mut c_void) -> u8;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(boolean: CfTypeRef) -> u8;
    fn CFGetTypeID(cf: CfTypeRef) -> usize;
    fn CFRetain(cf: CfTypeRef) -> CfTypeRef;
    fn CFRelease(cf: CfTypeRef);
}

/// Referencia CF propia (regla Create/Copy): se suelta al salir de alcance.
struct Owned(CfTypeRef);

impl Owned {
    /// `None` si `cf` es nulo.
    fn new(cf: CfTypeRef) -> Option<Self> {
        (!cf.is_null()).then_some(Self(cf))
    }

    /// Retiene un valor prestado para que sobreviva a su contenedor.
    fn retain(cf: CfTypeRef) -> Option<Self> {
        if cf.is_null() {
            return None;
        }
        // SAFETY: `cf` es un objeto CF válido y no nulo; el `CFRelease` de `Drop` lo equilibra.
        Some(Self(unsafe { CFRetain(cf) }))
    }

    fn get(&self) -> CfTypeRef {
        self.0
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: el objeto es nuestro (Create/Copy/Retain) y no nulo.
        unsafe { CFRelease(self.0) };
    }
}

fn type_is(cf: CfTypeRef, type_id: usize) -> bool {
    // SAFETY: `cf` es un objeto CF válido y no nulo (lo comprueban quienes llaman).
    !cf.is_null() && unsafe { CFGetTypeID(cf) } == type_id
}

fn cf_str(s: &CStr) -> Option<Owned> {
    // SAFETY: `s` es UTF-8 terminado en NUL; el resultado es nuestro (regla Create).
    Owned::new(unsafe {
        CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), CF_STRING_ENCODING_UTF8)
    })
}

fn to_string(cf: CfTypeRef) -> Option<String> {
    // SAFETY: consulta de tipo sin efectos.
    if !type_is(cf, unsafe { CFStringGetTypeID() }) {
        return None;
    }
    // SAFETY: `cf` es una CFString; el tamaño máximo cubre cualquier contenido + NUL.
    let size = unsafe {
        CFStringGetMaximumSizeForEncoding(CFStringGetLength(cf), CF_STRING_ENCODING_UTF8)
    }
    .max(0)
        + 1;
    let mut buf = vec![0 as c_char; usize::try_from(size).ok()?];
    // SAFETY: el búfer tiene `size` elementos, lo que se declara.
    let ok = unsafe { CFStringGetCString(cf, buf.as_mut_ptr(), size, CF_STRING_ENCODING_UTF8) };
    if ok == 0 {
        return None;
    }
    // SAFETY: `CFStringGetCString` devolvió éxito, así que `buf` termina en NUL.
    let s = unsafe { CStr::from_ptr(buf.as_ptr()) };
    Some(s.to_string_lossy().into_owned())
}

fn cf_bool(cf: CfTypeRef) -> bool {
    // SAFETY: consulta de tipo sin efectos.
    if !type_is(cf, unsafe { CFBooleanGetTypeID() }) {
        return false;
    }
    // SAFETY: `cf` es un CFBoolean (comprobado arriba).
    unsafe { CFBooleanGetValue(cf) != 0 }
}

/// `dict[key]` como `i32`, si es un CFNumber.
fn dict_i32(dict: CfTypeRef, key: &CStr) -> Option<i32> {
    let v = dict_value(dict, key)?;
    // SAFETY: consulta de tipo sin efectos.
    if !type_is(v, unsafe { CFNumberGetTypeID() }) {
        return None;
    }
    let mut n = 0i32;
    // SAFETY: `v` es un CFNumber y `n` tiene el tamaño de `kCFNumberSInt32Type`.
    let ok = unsafe { CFNumberGetValue(v, CF_NUMBER_SINT32, (&raw mut n).cast()) };
    (ok != 0).then_some(n)
}

/// `dict[key]`, prestado.
fn dict_value(dict: CfTypeRef, key: &CStr) -> Option<CfTypeRef> {
    let k = cf_str(key)?;
    // SAFETY: `dict` es un CFDictionary (lo comprueba quien llama) y `k` una CFString.
    let v = unsafe { CFDictionaryGetValue(dict, k.get()) };
    (!v.is_null()).then_some(v)
}

fn ax_err(op: &'static str, code: AxError) -> GatherError {
    if code == AX_API_DISABLED {
        GatherError::PermissionDenied
    } else {
        GatherError::Os { op, code }
    }
}

/// Atributo de un elemento AX, propio. `Ok(None)` si no tiene valor.
fn copy_attr(element: CfTypeRef, attr: &CStr) -> Result<Option<Owned>, GatherError> {
    let name = cf_str(attr).ok_or(GatherError::Os {
        op: "CFStringCreateWithCString",
        code: 0,
    })?;
    let mut value: CfTypeRef = std::ptr::null();
    // SAFETY: `element` es un AXUIElement válido; `value` recibe un objeto propio (Copy).
    let err = unsafe { AXUIElementCopyAttributeValue(element, name.get(), &mut value) };
    match err {
        AX_SUCCESS => Ok(Owned::new(value)),
        AX_NO_VALUE => Ok(None),
        e => Err(ax_err("AXUIElementCopyAttributeValue", e)),
    }
}

fn set_attr(element: CfTypeRef, attr: &CStr, value: &Owned) -> Result<(), GatherError> {
    let name = cf_str(attr).ok_or(GatherError::Os {
        op: "CFStringCreateWithCString",
        code: 0,
    })?;
    // SAFETY: `element` es un AXUIElement válido y `value` un AXValue propio.
    let err = unsafe { AXUIElementSetAttributeValue(element, name.get(), value.get()) };
    if err == AX_SUCCESS {
        Ok(())
    } else {
        Err(ax_err("AXUIElementSetAttributeValue", err))
    }
}

/// Lee un `AXValue` de tipo `kind` en `out`.
fn ax_value<T: Default>(value: &Owned, kind: u32) -> Option<T> {
    // SAFETY: consulta de tipo sin efectos.
    if !type_is(value.get(), unsafe { AXValueGetTypeID() }) {
        return None;
    }
    let mut out = T::default();
    // SAFETY: `T` es el struct `#[repr(C)]` que corresponde a `kind` (CGPoint o CGSize).
    let ok = unsafe { AXValueGetValue(value.get(), kind, (&raw mut out).cast()) };
    (ok != 0).then_some(out)
}

fn new_ax_value<T>(kind: u32, value: &T) -> Result<Owned, GatherError> {
    // SAFETY: `T` es el struct `#[repr(C)]` que corresponde a `kind`; AX copia el valor.
    Owned::new(unsafe { AXValueCreate(kind, (value as *const T).cast()) }).ok_or(GatherError::Os {
        op: "AXValueCreate",
        code: 0,
    })
}

/// Procesos con ventanas normales (capa 0), con el nombre de la aplicación.
fn window_owners() -> Vec<(i32, String)> {
    // SAFETY: función Copy: el arreglo es nuestro.
    let Some(list) =
        Owned::new(unsafe { CGWindowListCopyWindowInfo(CG_WINDOW_LIST_OPTIONS, CG_NULL_WINDOW) })
    else {
        return Vec::new();
    };
    // SAFETY: consulta de tipo sin efectos.
    if !type_is(list.get(), unsafe { CFArrayGetTypeID() }) {
        return Vec::new();
    }
    let mut owners = BTreeMap::new();
    // SAFETY: `list` es un CFArray.
    let n = unsafe { CFArrayGetCount(list.get()) };
    for i in 0..n {
        // SAFETY: `i` está dentro del arreglo; el valor es prestado y vive con `list`.
        let info = unsafe { CFArrayGetValueAtIndex(list.get(), i) };
        // SAFETY: consulta de tipo sin efectos.
        if !type_is(info, unsafe { CFDictionaryGetTypeID() }) {
            continue;
        }
        if dict_i32(info, c"kCGWindowLayer") != Some(0) {
            continue;
        }
        let Some(pid) = dict_i32(info, c"kCGWindowOwnerPID") else {
            continue;
        };
        let name = dict_value(info, c"kCGWindowOwnerName")
            .and_then(to_string)
            .unwrap_or_else(|| format!("pid {pid}"));
        owners.entry(pid).or_insert(name);
    }
    owners.into_iter().collect()
}

/// Permiso de Accesibilidad concedido a este proceso.
pub fn accessibility_trusted() -> bool {
    // SAFETY: consulta sin efectos.
    unsafe { AXIsProcessTrusted() != 0 }
}

/// Backend real de macOS. Guarda los elementos AX del último listado: los [`WindowId`]
/// son índices en él.
#[derive(Default)]
pub struct AxApi {
    elements: Vec<Owned>,
}

impl std::fmt::Debug for AxApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AxApi")
            .field("windows", &self.elements.len())
            .finish()
    }
}

impl AxApi {
    pub fn new() -> Self {
        Self::default()
    }

    fn element(&self, id: WindowId) -> Result<CfTypeRef, GatherError> {
        self.elements
            .get(id.0)
            .map(Owned::get)
            .ok_or(GatherError::UnknownWindow(id))
    }

    /// Ventanas de una aplicación. Una app que no responde se salta: no debe impedir
    /// mover las demás.
    fn app_windows(
        &mut self,
        pid: i32,
        app: &str,
        out: &mut Vec<Window>,
    ) -> Result<(), GatherError> {
        // SAFETY: función Create: el elemento es nuestro.
        let Some(element) = Owned::new(unsafe { AXUIElementCreateApplication(pid) }) else {
            return Ok(());
        };
        // SAFETY: `element` es un AXUIElement válido.
        unsafe { AXUIElementSetMessagingTimeout(element.get(), AX_TIMEOUT_SECS) };
        let list = match copy_attr(element.get(), c"AXWindows") {
            Ok(Some(list)) => list,
            Ok(None) => return Ok(()),
            Err(GatherError::PermissionDenied) => return Err(GatherError::PermissionDenied),
            Err(_) => return Ok(()),
        };
        // SAFETY: consulta de tipo sin efectos.
        if !type_is(list.get(), unsafe { CFArrayGetTypeID() }) {
            return Ok(());
        }
        // SAFETY: `list` es un CFArray.
        let n = unsafe { CFArrayGetCount(list.get()) };
        for i in 0..n {
            // SAFETY: `i` está dentro del arreglo; se retiene para que sobreviva a `list`.
            let Some(win) = Owned::retain(unsafe { CFArrayGetValueAtIndex(list.get(), i) }) else {
                continue;
            };
            let Some(frame) = frame(win.get()) else {
                continue;
            };
            let minimized = copy_attr(win.get(), c"AXMinimized")
                .ok()
                .flatten()
                .is_some_and(|b| cf_bool(b.get()));
            let title = copy_attr(win.get(), c"AXTitle")
                .ok()
                .flatten()
                .and_then(|t| to_string(t.get()));
            out.push(Window {
                id: WindowId(self.elements.len()),
                app: app.to_owned(),
                title,
                frame,
                minimized,
            });
            self.elements.push(win);
        }
        Ok(())
    }
}

fn frame(win: CfTypeRef) -> Option<Bounds> {
    let pos: CgPoint = ax_value(&copy_attr(win, c"AXPosition").ok()??, AX_VALUE_CG_POINT)?;
    let size: CgSize = ax_value(&copy_attr(win, c"AXSize").ok()??, AX_VALUE_CG_SIZE)?;
    Some(Bounds {
        x: pos.x.round() as i32,
        y: pos.y.round() as i32,
        width: size.width.round() as i32,
        height: size.height.round() as i32,
    })
}

impl WindowApi for AxApi {
    fn windows(&mut self) -> Result<Vec<Window>, GatherError> {
        if !accessibility_trusted() {
            return Err(GatherError::PermissionDenied);
        }
        self.elements.clear();
        let mut out = Vec::new();
        for (pid, app) in window_owners() {
            self.app_windows(pid, &app, &mut out)?;
        }
        Ok(out)
    }

    fn move_to(&mut self, id: WindowId, x: i32, y: i32) -> Result<(), GatherError> {
        let el = self.element(id)?;
        let p = CgPoint {
            x: f64::from(x),
            y: f64::from(y),
        };
        set_attr(el, c"AXPosition", &new_ax_value(AX_VALUE_CG_POINT, &p)?)
    }

    fn resize(&mut self, id: WindowId, width: i32, height: i32) -> Result<(), GatherError> {
        let el = self.element(id)?;
        let s = CgSize {
            width: f64::from(width),
            height: f64::from(height),
        };
        set_attr(el, c"AXSize", &new_ax_value(AX_VALUE_CG_SIZE, &s)?)
    }
}

/// "Traer ventanas" de punta a punta: pantallas actuales de Quartz + ventanas por AX.
pub fn gather_builtin_windows() -> Result<GatherReport, GatherError> {
    let displays = CgApi::new().online()?;
    crate::gather::gather(&mut AxApi::new(), &displays)
}

/// Pruebas en la Mac real (`cargo test -p tandem-display -- --ignored`). Necesitan el
/// permiso de Accesibilidad para la terminal desde la que se corren.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "hardware"]
    fn lists_windows() {
        eprintln!("Accesibilidad: {}", accessibility_trusted());
        let windows = AxApi::new().windows().unwrap();
        for w in &windows {
            eprintln!("{w}{}", if w.minimized { " [minimizada]" } else { "" });
        }
    }

    /// Mueve de verdad las ventanas que haya en la integrada. No cambia pantallas, así
    /// que no necesita vuelta automática.
    #[test]
    #[ignore = "hardware"]
    fn gathers_builtin_windows() {
        let r = gather_builtin_windows().unwrap();
        eprintln!("destino: {:?}", r.target);
        for m in &r.moved {
            eprintln!("movida: {} -> {:?}", m.window, m.to);
        }
        for w in &r.skipped {
            eprintln!("minimizada, se deja: {w}");
        }
        for (w, e) in &r.failed {
            eprintln!("falló: {w}: {e}");
        }
        assert!(r.complete());
    }
}
