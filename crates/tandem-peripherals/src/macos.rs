//! macOS: USB devices present, through IOKit.
//!
//! The `IOUSBHostDevice` entries of the IOKit registry are walked and their
//! `idVendor` and `idProduct` properties are read, the same VID/PID shown by the System
//! Report and that Windows puts in the instance ID.

use std::ffi::{CStr, c_char, c_void};

use crate::id::UsbId;
use crate::watcher::DeviceScanner;

type KernReturn = i32;
type MachPort = u32;
type IoObject = u32;
type CFTypeRef = *const c_void;
type CFIndex = isize;

const KERN_SUCCESS: KernReturn = 0;
/// `kIOMainPortDefault`: IOKit's main port.
const IO_MAIN_PORT_DEFAULT: MachPort = 0;
const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const CF_NUMBER_SINT32_TYPE: CFIndex = 3;

#[link(name = "IOKit", kind = "framework")]
// SAFETY: signatures copied from the IOKit headers of the SDK; the pointers
// passed only live during each call and the returned objects are released by the caller.
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
// SAFETY: signatures copied from CoreFoundation; the C strings are NUL-terminated and do not
// outlive the call, and objects created with `Create` are released by the caller.
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
    #[error("IOServiceMatching did not return the dictionary")]
    Matching,
    #[error("IOServiceGetMatchingServices failed (kern_return {0:#x})")]
    Services(KernReturn),
    #[error("could not create the key {0}")]
    Key(&'static str),
}

/// Real macOS scanner.
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

/// IOKit object (service or iterator); freed when dropped.
struct IoOwned(IoObject);

impl IoOwned {
    fn next(&self) -> Option<IoOwned> {
        // SAFETY: `self.0` is a valid iterator while `self` lives.
        let obj = unsafe { IOIteratorNext(self.0) };
        (obj != 0).then_some(IoOwned(obj))
    }
}

impl Drop for IoOwned {
    fn drop(&mut self) {
        // SAFETY: the object is ours (IOKit returned it with a reference) and is
        // released only once.
        unsafe { IOObjectRelease(self.0) };
    }
}

/// CoreFoundation value with its own reference; freed when dropped.
struct CfOwned(CFTypeRef);

impl CfOwned {
    fn string(s: &CStr) -> Option<Self> {
        // SAFETY: `s` ends in zero and lives during the call; the null allocator
        // is the default one.
        let cf = unsafe {
            CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), CF_STRING_ENCODING_UTF8)
        };
        (!cf.is_null()).then_some(CfOwned(cf))
    }
}

impl Drop for CfOwned {
    fn drop(&mut self) {
        // SAFETY: only built from non-null pointers of "Create" functions.
        unsafe { CFRelease(self.0) };
    }
}

fn usb_devices() -> Result<IoOwned, ScanError> {
    // SAFETY: the class name is a zero-terminated static string.
    let matching = unsafe { IOServiceMatching(c"IOUSBHostDevice".as_ptr()) };
    if matching.is_null() {
        return Err(ScanError::Matching);
    }
    let mut iter: IoObject = 0;
    // SAFETY: `IOServiceGetMatchingServices` takes the dictionary (it
    // releases it itself, also on failure) and initializes `iter`, a local variable.
    let kr = unsafe { IOServiceGetMatchingServices(IO_MAIN_PORT_DEFAULT, matching, &mut iter) };
    if kr != KERN_SUCCESS {
        return Err(ScanError::Services(kr));
    }
    Ok(IoOwned(iter))
}

fn u16_property(device: &IoOwned, key: &CfOwned) -> Option<u16> {
    // SAFETY: valid service and key while their owners live; the property
    // returned is ours ("Create") and `CfOwned` releases it.
    let raw = unsafe { IORegistryEntryCreateCFProperty(device.0, key.0, std::ptr::null(), 0) };
    if raw.is_null() {
        return None;
    }
    let value = CfOwned(raw);
    // SAFETY: `value.0` is a valid CF object; it is checked to be a number
    // before reading it into a local i32.
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

    /// Runs on the macOS runner: enumeration works even when there is
    /// no USB device.
    #[test]
    fn scan_runs() {
        let ids = IoKitScanner::new().scan().unwrap();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids, sorted);
    }
}
