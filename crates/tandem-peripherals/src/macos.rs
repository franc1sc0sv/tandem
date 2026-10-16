//! macOS: dispositivos USB presentes, por IOKit.
//!
//! Se recorren los `IOUSBHostDevice` del registro de IOKit y se leen sus
//! propiedades `idVendor` e `idProduct`, el mismo VID/PID que muestra el Informe
//! del sistema y que Windows pone en el identificador de instancia.

use std::ffi::{CStr, c_char, c_void};

use crate::id::UsbId;
use crate::watcher::DeviceScanner;

type KernReturn = i32;
type MachPort = u32;
type IoObject = u32;
type CFTypeRef = *const c_void;
type CFIndex = isize;

const KERN_SUCCESS: KernReturn = 0;
/// `kIOMainPortDefault`: el puerto principal de IOKit.
const IO_MAIN_PORT_DEFAULT: MachPort = 0;
const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const CF_NUMBER_SINT32_TYPE: CFIndex = 3;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingServices(
        main_port: MachPort,
        matching: *mut c_void,
        existing: *mut IoObject,
    ) -> KernReturn;
    fn IOIteratorNext(iterator: IoObject) -> IoObject;
    fn IOObjectRelease(object: IoObject) -> KernReturn;
    fn IORegistryEntryCreateCFProperty(
        entry: IoObject,
        key: CFTypeRef,
        allocator: CFTypeRef,
        options: u32,
    ) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(
        allocator: CFTypeRef,
        cstr: *const c_char,
        encoding: u32,
    ) -> CFTypeRef;
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFNumberGetValue(number: CFTypeRef, the_type: CFIndex, value: *mut c_void) -> u8;
    fn CFRelease(cf: CFTypeRef);
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScanError {
    #[error("IOServiceMatching no devolvió el diccionario")]
    Matching,
    #[error("IOServiceGetMatchingServices falló (kern_return {0:#x})")]
    Services(KernReturn),
    #[error("no se pudo crear la clave {0}")]
    Key(&'static str),
}

/// Escáner real de macOS.
#[derive(Debug, Default, Clone, Copy)]
pub struct IoKitScanner;

impl IoKitScanner {
    pub fn new() -> Self {
        IoKitScanner
    }
}

impl DeviceScanner for IoKitScanner {
    type Error = ScanError;

    fn scan(&mut self) -> Result<Vec<UsbId>, ScanError> {
        let vendor_key = CfOwned::string(c"idVendor").ok_or(ScanError::Key("idVendor"))?;
        let product_key = CfOwned::string(c"idProduct").ok_or(ScanError::Key("idProduct"))?;
        let iter = usb_devices()?;
        let mut ids = Vec::new();
        while let Some(device) = iter.next() {
            let vendor = u16_property(&device, &vendor_key);
            let product = u16_property(&device, &product_key);
            if let (Some(vendor), Some(product)) = (vendor, product) {
                ids.push(UsbId { vendor, product });
            }
        }
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }
}

/// Objeto de IOKit (servicio o iterador); se libera al soltarlo.
struct IoOwned(IoObject);

impl IoOwned {
    fn next(&self) -> Option<IoOwned> {
        // SAFETY: `self.0` es un iterador válido mientras viva `self`.
        let obj = unsafe { IOIteratorNext(self.0) };
        (obj != 0).then_some(IoOwned(obj))
    }
}

impl Drop for IoOwned {
    fn drop(&mut self) {
        // SAFETY: el objeto es nuestro (lo devolvió IOKit con una referencia) y se
        // libera una sola vez.
        unsafe { IOObjectRelease(self.0) };
    }
}

/// Valor de CoreFoundation con una referencia propia; se libera al soltarlo.
struct CfOwned(CFTypeRef);

impl CfOwned {
    fn string(s: &CStr) -> Option<Self> {
        // SAFETY: `s` termina en cero y vive durante la llamada; el asignador nulo
        // es el predeterminado.
        let cf = unsafe {
            CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), CF_STRING_ENCODING_UTF8)
        };
        (!cf.is_null()).then_some(CfOwned(cf))
    }
}

impl Drop for CfOwned {
    fn drop(&mut self) {
        // SAFETY: solo se construye con punteros no nulos de funciones «Create».
        unsafe { CFRelease(self.0) };
    }
}

fn usb_devices() -> Result<IoOwned, ScanError> {
    // SAFETY: el nombre de clase es una cadena estática terminada en cero.
    let matching = unsafe { IOServiceMatching(c"IOUSBHostDevice".as_ptr()) };
    if matching.is_null() {
        return Err(ScanError::Matching);
    }
    let mut iter: IoObject = 0;
    // SAFETY: `IOServiceGetMatchingServices` se queda con el diccionario (lo
    // libera ella, también si falla) e inicializa `iter`, una variable local.
    let kr = unsafe { IOServiceGetMatchingServices(IO_MAIN_PORT_DEFAULT, matching, &mut iter) };
    if kr != KERN_SUCCESS {
        return Err(ScanError::Services(kr));
    }
    Ok(IoOwned(iter))
}

fn u16_property(device: &IoOwned, key: &CfOwned) -> Option<u16> {
    // SAFETY: servicio y clave válidos mientras vivan sus dueños; la propiedad
    // devuelta es nuestra («Create») y la suelta `CfOwned`.
    let raw = unsafe { IORegistryEntryCreateCFProperty(device.0, key.0, std::ptr::null(), 0) };
    if raw.is_null() {
        return None;
    }
    let value = CfOwned(raw);
    // SAFETY: `value.0` es un objeto CF válido; se comprueba que sea un número
    // antes de leerlo en un i32 local.
    unsafe {
        if CFGetTypeID(value.0) != CFNumberGetTypeID() {
            return None;
        }
        let mut n: i32 = 0;
        if CFNumberGetValue(value.0, CF_NUMBER_SINT32_TYPE, (&raw mut n).cast()) == 0 {
            return None;
        }
        u16::try_from(n).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Corre en el runner de macOS: la enumeración funciona aunque no haya
    /// ningún dispositivo USB.
    #[test]
    fn scan_runs() {
        let ids = IoKitScanner::new().scan().unwrap();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids, sorted);
    }
}
