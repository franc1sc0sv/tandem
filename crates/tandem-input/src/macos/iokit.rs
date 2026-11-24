//! What the IOKit registry says about the displays (T-353): whether the lid is closed, and the EDID
//! of every external display, even one without a DDC/CI channel (a USB-C to HDMI adapter).
//!
//! Read-only: it only reads registry properties. Plain C calls of IOKit and CoreFoundation, which
//! every macOS has.

use std::ffi::{CStr, c_char, c_void};

type CFTypeRef = *const c_void;
type IoObject = u32;

const MAIN_PORT: u32 = 0;
const UTF8: u32 = 0x0800_0100;

#[link(name = "IOKit", kind = "framework")]
// SAFETY: declarations of IOKit functions with their C signatures; each call says why it is sound.
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingServices(main: u32, matching: *mut c_void, iter: *mut IoObject) -> i32;
    fn IOIteratorNext(iter: IoObject) -> IoObject;
    fn IOObjectRelease(object: IoObject) -> i32;
    fn IORegistryEntryCreateCFProperty(
        entry: IoObject,
        key: CFTypeRef,
        allocator: CFTypeRef,
        options: u32,
    ) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
// SAFETY: declarations of CoreFoundation functions with their C signatures; each call says why it is sound.
unsafe extern "C" {
    fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFTypeRef;
    fn CFRelease(object: CFTypeRef);
    fn CFGetTypeID(object: CFTypeRef) -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFDataGetLength(data: CFTypeRef) -> isize;
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(b: CFTypeRef) -> u8;
}

/// A CF object this code owns (+1), released once. Never built from a null pointer: CoreFoundation
/// traps on `CFRelease(NULL)`, so the constructors check first and build it lazily.
struct Owned(CFTypeRef);

impl Owned {
    fn new(p: CFTypeRef) -> Option<Self> {
        if p.is_null() { None } else { Some(Owned(p)) }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the pointer is non-null and came from a CF "Create" call (+1), released once here.
        unsafe { CFRelease(self.0) };
    }
}

fn string(s: &CStr) -> Option<Owned> {
    // SAFETY: `s` is NUL-terminated; a null allocator is the default one.
    let p = unsafe { CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), UTF8) };
    Owned::new(p)
}

/// The registry property `key` of `entry`, if it has one.
fn property(entry: IoObject, key: &CStr) -> Option<Owned> {
    let key = string(key)?;
    // SAFETY: `entry` is a live registry entry and `key` a live CFString; the result is +1.
    let p = unsafe { IORegistryEntryCreateCFProperty(entry, key.0, std::ptr::null(), 0) };
    Owned::new(p)
}

fn bytes(value: &Owned) -> Option<Vec<u8>> {
    // SAFETY: `value.0` is a live CF object; its type is checked before reading it as CFData.
    unsafe {
        if CFGetTypeID(value.0) != CFDataGetTypeID() {
            return None;
        }
        let len = usize::try_from(CFDataGetLength(value.0)).ok()?;
        let ptr = CFDataGetBytePtr(value.0);
        if ptr.is_null() {
            return None;
        }
        Some(std::slice::from_raw_parts(ptr, len).to_vec())
    }
}

fn boolean(value: &Owned) -> Option<bool> {
    // SAFETY: `value.0` is a live CF object; its type is checked before reading it as CFBoolean.
    unsafe {
        (CFGetTypeID(value.0) == CFBooleanGetTypeID()).then(|| CFBooleanGetValue(value.0) != 0)
    }
}

/// Calls `f` with every registry entry of class `class`.
fn each_service(class: &CStr, mut f: impl FnMut(IoObject)) {
    let mut iter: IoObject = 0;
    // SAFETY: `class` is NUL-terminated; the matching dictionary is consumed by
    // `IOServiceGetMatchingServices`, and `iter` is a valid place for the iterator.
    let rc = unsafe {
        let matching = IOServiceMatching(class.as_ptr());
        if matching.is_null() {
            return;
        }
        IOServiceGetMatchingServices(MAIN_PORT, matching, &mut iter)
    };
    if rc != 0 {
        return;
    }
    loop {
        // SAFETY: `iter` is a live iterator.
        let entry = unsafe { IOIteratorNext(iter) };
        if entry == 0 {
            break;
        }
        f(entry);
        // SAFETY: `entry` came from `IOIteratorNext` and is released once.
        unsafe { IOObjectRelease(entry) };
    }
    // SAFETY: `iter` came from `IOServiceGetMatchingServices` and is released once.
    unsafe { IOObjectRelease(iter) };
}

/// The lid of a MacBook is closed (`AppleClamshellState` of the power domain).
pub fn lid_closed() -> bool {
    let mut closed = false;
    each_service(c"IOPMrootDomain", |entry| {
        closed |= property(entry, c"AppleClamshellState")
            .and_then(|v| boolean(&v))
            .unwrap_or(false);
    });
    closed
}

/// The EDID of every display attached to an external port (HDMI or USB-C), as the port's
/// DisplayPort transport state holds it: also the ones whose path has no DDC/CI channel.
pub fn external_edids() -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    each_service(c"IOPortTransportStateDisplayPort", |entry| {
        if let Some(edid) = property(entry, c"EDID").and_then(|v| bytes(&v))
            && edid.len() >= 128
            && !out.contains(&edid)
        {
            out.push(edid);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use crate::displays::DisplayPower;
    use crate::macos::display_power::MacDisplayPower;

    /// Read-only, on a real Mac (T-353): every external display's EDID is listed, also one behind
    /// an adapter without DDC/CI, and CoreGraphics knows the built-in display at most once.
    /// `cargo test -p tandem-input -- --ignored --nocapture reads_the_real_displays`
    #[test]
    #[ignore = "hardware"]
    fn reads_the_real_displays() {
        let edids = super::external_edids();
        for e in &edids {
            assert!(e.len() >= 128 && e[..8] == [0, 255, 255, 255, 255, 255, 255, 0]);
            println!("EDID {:02x}{:02x} {:02x}{:02x}", e[8], e[9], e[10], e[11]);
        }
        let online = MacDisplayPower::new().online().unwrap();
        println!("lid closed: {}", super::lid_closed());
        for d in &online {
            println!(
                "display {} builtin {} enabled {} mirror {:?}",
                d.id, d.builtin, d.enabled, d.mirror_of
            );
        }
        assert!(online.iter().filter(|d| d.builtin).count() <= 1);
    }
}
