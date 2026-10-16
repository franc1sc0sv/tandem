//! [`QuartzApi`] real: CoreGraphics por FFI directo (sin dependencias nuevas).
//!
//! - Listar: `CGGetOnlineDisplayList` + `CGDisplay*`. El nombre del producto sale de
//!   `CoreDisplay_DisplayCreateInfoDictionary` (privada), si existe.
//! - Desactivar/activar: `CGSConfigureDisplayEnabled` (privada, SkyLight), dentro de una
//!   configuración de Quartz. Se resuelve con `dlsym` en tiempo de ejecución: si Apple la
//!   quita, el binario sigue cargando y la lógica cae al espejo.
//! - Espejo: `CGConfigureDisplayMirrorOfDisplay` (pública).
//!
//! Ver `docs/research/macos-pantallas-s2.md`.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::OnceLock;

use crate::quartz::{Bounds, CgDisplay, DisplayId, QuartzApi};
use crate::topology::TopologyError;

type CgError = i32;
type CgDisplayConfigRef = *mut c_void;
type CfTypeRef = *const c_void;

const CG_SUCCESS: CgError = 0;
/// `kCGConfigureForSession`: dura hasta cerrar sesión. `ForAppOnly` revertiría al salir
/// el proceso, y un CLI de diagnóstico no podría dejar nada aplicado.
const CG_CONFIGURE_FOR_SESSION: u32 = 1;
/// `kCGNullDirectDisplay`: sin espejo.
const CG_NULL_DISPLAY: u32 = 0;
/// Una Mac no pasa de unas pocas pantallas; se deja margen.
const MAX_DISPLAYS: usize = 32;
const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const RTLD_LAZY: c_int = 0x1;
/// `RTLD_DEFAULT` en macOS (`dlfcn.h`): buscar en todas las imágenes cargadas.
const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

const SKYLIGHT: &CStr = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";
const CORE_DISPLAY: &CStr = c"/System/Library/Frameworks/CoreDisplay.framework/CoreDisplay";

#[repr(C)]
#[derive(Clone, Copy)]
struct CgPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CgSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CgRect {
    origin: CgPoint,
    size: CgSize,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> CgError;
    fn CGDisplayIsBuiltin(display: u32) -> u32;
    fn CGDisplayIsMain(display: u32) -> u32;
    fn CGDisplayIsActive(display: u32) -> u32;
    fn CGDisplayMirrorsDisplay(display: u32) -> u32;
    fn CGDisplayVendorNumber(display: u32) -> u32;
    fn CGDisplayModelNumber(display: u32) -> u32;
    fn CGDisplaySerialNumber(display: u32) -> u32;
    fn CGDisplayBounds(display: u32) -> CgRect;
    fn CGDisplayScreenSize(display: u32) -> CgSize;
    fn CGBeginDisplayConfiguration(config: *mut CgDisplayConfigRef) -> CgError;
    fn CGConfigureDisplayMirrorOfDisplay(
        config: CgDisplayConfigRef,
        display: u32,
        master: u32,
    ) -> CgError;
    fn CGCompleteDisplayConfiguration(config: CgDisplayConfigRef, option: u32) -> CgError;
    fn CGCancelDisplayConfiguration(config: CgDisplayConfigRef) -> CgError;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(alloc: CfTypeRef, s: *const c_char, encoding: u32) -> CfTypeRef;
    fn CFStringGetCString(s: CfTypeRef, buf: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFStringGetTypeID() -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFGetTypeID(cf: CfTypeRef) -> usize;
    fn CFDictionaryGetValue(dict: CfTypeRef, key: CfTypeRef) -> CfTypeRef;
    fn CFDictionaryGetCount(dict: CfTypeRef) -> isize;
    fn CFDictionaryGetKeysAndValues(dict: CfTypeRef, keys: *mut CfTypeRef, values: *mut CfTypeRef);
    fn CFRelease(cf: CfTypeRef);
}

// libSystem se enlaza siempre.
unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

type ConfigureEnabledFn = unsafe extern "C" fn(CgDisplayConfigRef, u32, bool) -> CgError;
type CreateInfoFn = unsafe extern "C" fn(u32) -> CfTypeRef;

/// Busca un símbolo en las imágenes ya cargadas y, si no está, en `library`.
fn resolve(library: &CStr, symbol: &CStr) -> Option<usize> {
    // SAFETY: `dlsym`/`dlopen` reciben cadenas C válidas y terminadas en NUL; los
    // handles que devuelven no se liberan (las bibliotecas del sistema viven todo el proceso).
    let ptr = unsafe {
        let mut p = dlsym(RTLD_DEFAULT, symbol.as_ptr());
        if p.is_null() {
            let handle = dlopen(library.as_ptr(), RTLD_LAZY);
            if !handle.is_null() {
                p = dlsym(handle, symbol.as_ptr());
            }
        }
        p
    };
    (!ptr.is_null()).then_some(ptr as usize)
}

fn configure_enabled_fn() -> Option<ConfigureEnabledFn> {
    static FN: OnceLock<Option<usize>> = OnceLock::new();
    let addr = (*FN.get_or_init(|| resolve(SKYLIGHT, c"CGSConfigureDisplayEnabled")))?;
    // SAFETY: la dirección viene de `dlsym` del símbolo con esta firma, la que usan las
    // herramientas conocidas (BetterDisplay, displayplacer): (config, display, bool) -> CGError.
    Some(unsafe { std::mem::transmute::<usize, ConfigureEnabledFn>(addr) })
}

fn create_info_fn() -> Option<CreateInfoFn> {
    static FN: OnceLock<Option<usize>> = OnceLock::new();
    let addr =
        (*FN.get_or_init(|| resolve(CORE_DISPLAY, c"CoreDisplay_DisplayCreateInfoDictionary")))?;
    // SAFETY: símbolo de CoreDisplay con firma (CGDirectDisplayID) -> CFDictionaryRef retenido.
    Some(unsafe { std::mem::transmute::<usize, CreateInfoFn>(addr) })
}

fn os(op: &'static str, code: CgError) -> TopologyError {
    TopologyError::Os {
        op,
        message: format!("CGError {code}"),
    }
}

/// Backend real de macOS.
#[derive(Debug, Default)]
pub struct CgApi;

impl CgApi {
    pub fn new() -> Self {
        Self
    }

    /// ¿Está la API privada para desactivar pantallas en este macOS?
    pub fn private_api_available() -> bool {
        configure_enabled_fn().is_some()
    }

    /// Abre una configuración, aplica `f` y la confirma; si `f` falla, la cancela.
    fn configure(
        op: &'static str,
        f: impl FnOnce(CgDisplayConfigRef) -> CgError,
    ) -> Result<(), TopologyError> {
        let mut config: CgDisplayConfigRef = std::ptr::null_mut();
        // SAFETY: `config` es un puntero válido donde CoreGraphics deja la configuración.
        let err = unsafe { CGBeginDisplayConfiguration(&mut config) };
        if err != CG_SUCCESS {
            return Err(os("CGBeginDisplayConfiguration", err));
        }
        let err = f(config);
        if err != CG_SUCCESS {
            // SAFETY: `config` viene de un `CGBeginDisplayConfiguration` exitoso y no se usó
            // para completar.
            unsafe { CGCancelDisplayConfiguration(config) };
            return Err(os(op, err));
        }
        // SAFETY: igual que arriba; completar libera la configuración.
        let err = unsafe { CGCompleteDisplayConfiguration(config, CG_CONFIGURE_FOR_SESSION) };
        if err != CG_SUCCESS {
            return Err(os("CGCompleteDisplayConfiguration", err));
        }
        Ok(())
    }
}

fn online_ids() -> Result<Vec<u32>, TopologyError> {
    let mut ids = [0u32; MAX_DISPLAYS];
    let mut count = 0u32;
    // SAFETY: el arreglo tiene `MAX_DISPLAYS` elementos y así se le dice a CoreGraphics.
    let err = unsafe { CGGetOnlineDisplayList(MAX_DISPLAYS as u32, ids.as_mut_ptr(), &mut count) };
    if err != CG_SUCCESS {
        return Err(os("CGGetOnlineDisplayList", err));
    }
    Ok(ids[..(count as usize).min(MAX_DISPLAYS)].to_vec())
}

fn describe(id: u32) -> CgDisplay {
    // SAFETY: funciones de consulta sin efectos; con un id desconocido devuelven 0/rect nulo.
    let (builtin, main, active, mirror, vendor, model, serial, rect, size) = unsafe {
        (
            CGDisplayIsBuiltin(id) != 0,
            CGDisplayIsMain(id) != 0,
            CGDisplayIsActive(id) != 0,
            CGDisplayMirrorsDisplay(id),
            CGDisplayVendorNumber(id),
            CGDisplayModelNumber(id),
            CGDisplaySerialNumber(id),
            CGDisplayBounds(id),
            CGDisplayScreenSize(id),
        )
    };
    CgDisplay {
        id: DisplayId(id),
        name: product_name(id),
        vendor,
        model,
        serial,
        // Sin EDID con tamaño, CoreGraphics da 0 x 0.
        size_mm: (size.width >= 1.0 && size.height >= 1.0)
            .then(|| (size.width.round() as u32, size.height.round() as u32)),
        builtin,
        main,
        // En un grupo de espejo por hardware solo la principal cuenta como "activa", pero
        // la que refleja sigue en el escritorio: no es un fantasma quitado.
        enabled: active || mirror != CG_NULL_DISPLAY,
        mirror_of: (mirror != CG_NULL_DISPLAY).then_some(DisplayId(mirror)),
        bounds: Bounds {
            x: rect.origin.x.round() as i32,
            y: rect.origin.y.round() as i32,
            width: rect.size.width.round() as i32,
            height: rect.size.height.round() as i32,
        },
    }
}

/// `DisplayProductName` del diccionario de CoreDisplay: `{ "en_US": "LC34G55T", ... }`.
fn product_name(id: u32) -> Option<String> {
    let create = create_info_fn()?;
    // SAFETY: función "Create": devuelve un diccionario retenido (o nulo) que soltamos abajo.
    let info = unsafe { create(id) };
    if info.is_null() {
        return None;
    }
    let name = dict_get(info, c"DisplayProductName").and_then(|names| {
        if is_dict(names) {
            first_string_value(names)
        } else {
            cf_string(names)
        }
    });
    // SAFETY: `info` es nuestro (regla Create) y no se usa después.
    unsafe { CFRelease(info) };
    name.filter(|n| !n.trim().is_empty())
}

fn is_dict(cf: CfTypeRef) -> bool {
    // SAFETY: `cf` es un objeto CF válido y no nulo.
    unsafe { CFGetTypeID(cf) == CFDictionaryGetTypeID() }
}

/// Valor de `dict[key]`, prestado (no se suelta).
fn dict_get(dict: CfTypeRef, key: &CStr) -> Option<CfTypeRef> {
    // SAFETY: `key` es UTF-8 terminado en NUL; la cadena creada se suelta al terminar.
    let k = unsafe {
        CFStringCreateWithCString(std::ptr::null(), key.as_ptr(), CF_STRING_ENCODING_UTF8)
    };
    if k.is_null() || !is_dict(dict) {
        if !k.is_null() {
            // SAFETY: `k` viene de una función Create.
            unsafe { CFRelease(k) };
        }
        return None;
    }
    // SAFETY: `dict` es un CFDictionary (comprobado) y `k` una CFString válida.
    let v = unsafe { CFDictionaryGetValue(dict, k) };
    // SAFETY: `k` viene de una función Create.
    unsafe { CFRelease(k) };
    (!v.is_null()).then_some(v)
}

fn first_string_value(dict: CfTypeRef) -> Option<String> {
    // SAFETY: `dict` es un CFDictionary (comprobado por quien llama).
    let n = unsafe { CFDictionaryGetCount(dict) }.max(0) as usize;
    if n == 0 {
        return None;
    }
    let mut keys = vec![std::ptr::null(); n];
    let mut values = vec![std::ptr::null(); n];
    // SAFETY: los dos búferes tienen `n` elementos, el tamaño actual del diccionario.
    unsafe { CFDictionaryGetKeysAndValues(dict, keys.as_mut_ptr(), values.as_mut_ptr()) };
    values.into_iter().find_map(cf_string)
}

fn cf_string(cf: CfTypeRef) -> Option<String> {
    if cf.is_null() {
        return None;
    }
    // SAFETY: `cf` es un objeto CF válido y no nulo.
    if unsafe { CFGetTypeID(cf) != CFStringGetTypeID() } {
        return None;
    }
    let mut buf = [0 as c_char; 256];
    // SAFETY: el búfer tiene el tamaño que se declara; CF lo termina en NUL si cabe.
    let ok = unsafe {
        CFStringGetCString(
            cf,
            buf.as_mut_ptr(),
            buf.len() as isize,
            CF_STRING_ENCODING_UTF8,
        )
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: `CFStringGetCString` devolvió éxito, así que `buf` termina en NUL.
    let s = unsafe { CStr::from_ptr(buf.as_ptr()) };
    Some(s.to_string_lossy().into_owned())
}

impl QuartzApi for CgApi {
    fn online(&mut self) -> Result<Vec<CgDisplay>, TopologyError> {
        Ok(online_ids()?.into_iter().map(describe).collect())
    }

    fn set_enabled(&mut self, id: DisplayId, enabled: bool) -> Result<(), TopologyError> {
        let f = configure_enabled_fn().ok_or(TopologyError::PrivateApiUnavailable)?;
        Self::configure("CGSConfigureDisplayEnabled", |config| {
            // SAFETY: `config` es una configuración abierta; la firma de `f` es la del símbolo.
            unsafe { f(config, id.0, enabled) }
        })
    }

    fn set_mirror(
        &mut self,
        id: DisplayId,
        master: Option<DisplayId>,
    ) -> Result<(), TopologyError> {
        let master = master.map_or(CG_NULL_DISPLAY, |m| m.0);
        Self::configure("CGConfigureDisplayMirrorOfDisplay", |config| {
            // SAFETY: `config` es una configuración abierta.
            unsafe { CGConfigureDisplayMirrorOfDisplay(config, id.0, master) }
        })
    }
}

/// Pruebas en la Mac real (`cargo test -p tandem-display -- --ignored`). Todas las que
/// cambian algo usan [`RestoreGuard`](crate::quartz::RestoreGuard) y además devuelven la
/// integrada a `Extend`: la Mac no puede quedarse sin pantalla.
#[cfg(test)]
mod tests {
    use std::thread::sleep;
    use std::time::Duration;

    use tandem_core::BuiltinDisplay;

    use super::*;
    use crate::quartz::{MacTopology, RestoreGuard};
    use crate::topology::{self, BuiltinApplied, DisplayTopology, monitor_by_model};

    /// Tiempo para que el WindowServer termine de reconfigurar antes de comprobar.
    const SETTLE: Duration = Duration::from_secs(3);

    fn topo() -> MacTopology<CgApi> {
        MacTopology::new(CgApi::new())
    }

    fn has_external(t: &mut MacTopology<CgApi>) -> bool {
        t.displays().unwrap().iter().any(|d| !d.builtin && d.active)
    }

    #[test]
    #[ignore = "hardware"]
    fn lists_displays() {
        let mut t = topo();
        let all = t.displays().unwrap();
        for d in &all {
            eprintln!("{d}");
        }
        eprintln!(
            "CGSConfigureDisplayEnabled: {}",
            CgApi::private_api_available()
        );
        assert!(!all.is_empty());
        assert!(all.iter().filter(|d| d.builtin).count() <= 1);
        assert_eq!(all.iter().filter(|d| d.main).count(), 1);
    }

    #[test]
    #[ignore = "hardware"]
    fn builtin_off_and_back() {
        let mut t = topo();
        if !has_external(&mut t) {
            eprintln!("sin monitor externo: no se puede apagar la integrada");
            return;
        }
        let mut g = RestoreGuard(&mut t);
        let applied = topology::set_builtin(&mut *g, BuiltinDisplay::Off).unwrap();
        eprintln!("Off -> {applied:?}");
        sleep(SETTLE);
        let b = topology::builtin(&mut *g).unwrap();
        match applied {
            BuiltinApplied::Exact(_) => assert!(!b.active, "{b}"),
            BuiltinApplied::MirrorFallback => assert!(b.mirror_of.is_some(), "{b}"),
        }
        topology::set_builtin(&mut *g, BuiltinDisplay::Extend).unwrap();
        sleep(SETTLE);
        let b = topology::builtin(&mut *g).unwrap();
        assert!(b.active && b.mirror_of.is_none(), "{b}");
    }

    #[test]
    #[ignore = "hardware"]
    fn builtin_mirror_and_back() {
        let mut t = topo();
        if !has_external(&mut t) {
            eprintln!("sin monitor externo: no hay de qué hacer espejo");
            return;
        }
        let r = topology::set_builtin(&mut t, BuiltinDisplay::Mirror);
        sleep(SETTLE);
        let mirrored = topology::builtin(&mut t).map(|b| b.mirror_of.is_some());
        // Vuelta automática antes de afirmar nada.
        topology::set_builtin(&mut t, BuiltinDisplay::Extend).unwrap();
        r.unwrap();
        assert_eq!(mirrored, Ok(true));
    }

    /// Quita un monitor externo y lo devuelve. Elige el primero que tenga nombre de EDID.
    #[test]
    #[ignore = "hardware"]
    fn detach_external_and_back() {
        let mut t = topo();
        let Some(ext) =
            t.displays().unwrap().into_iter().find(|d| {
                !d.builtin && d.active && d.edid.as_ref().is_some_and(|e| e.name.is_some())
            })
        else {
            eprintln!("sin monitor externo con nombre de EDID");
            return;
        };
        let cfg = monitor_by_model("prueba", ext.model());
        let mut g = RestoreGuard(&mut t);
        topology::detach(&mut *g, &cfg).unwrap();
        sleep(SETTLE);
        let all = g.displays().unwrap();
        let after = topology::find_monitor(&all, &cfg).unwrap();
        assert!(!after.active, "{after}");
        topology::attach(&mut *g, &cfg).unwrap();
        sleep(SETTLE);
        let all = g.displays().unwrap();
        assert!(topology::find_monitor(&all, &cfg).unwrap().active);
    }
}
