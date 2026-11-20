//! Real [`DisplayPower`] on macOS (T-341): `CGSConfigureDisplayEnabled`, the private SkyLight
//! call that removes a display from the desktop and brings it back, inside a public Quartz
//! display configuration.
//!
//! It is resolved with `dlsym` at run time, like `monitor-ddc`'s `IOAVService`: a macOS without
//! it still loads the binary and reports display control as unavailable.
//!
//! Scope `kCGConfigureForSession`: a display left off by a crash comes back at the latest when
//! the user logs out; the agent turns it back on long before (at start-up).

use std::ffi::{CStr, c_void};
use std::sync::OnceLock;

use objc2_core_graphics::{
    CGBeginDisplayConfiguration, CGCancelDisplayConfiguration, CGCompleteDisplayConfiguration,
    CGConfigureOption, CGDirectDisplayID, CGDisplayConfigRef, CGError,
};

use crate::displays::{DisplayControl, DisplayPower, DisplayPowerError, PowerDisplay};

const SKYLIGHT: &CStr = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";
const SYMBOL: &CStr = c"CGSConfigureDisplayEnabled";

/// `CGError CGSConfigureDisplayEnabled(CGDisplayConfigRef, CGDirectDisplayID, bool)`.
type ConfigureEnabledFn = unsafe extern "C" fn(CGDisplayConfigRef, CGDirectDisplayID, bool) -> i32;

/// The address of `name`: first among the images already loaded, then in `library`.
fn resolve(library: &CStr, name: &CStr) -> Option<usize> {
    // SAFETY: both strings are NUL-terminated; `RTLD_DEFAULT` searches the loaded images and
    // `dlopen` of a system framework has no other preconditions. The handle is never closed,
    // so the address stays valid for the life of the process.
    let p = unsafe {
        let mut p = libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr());
        if p.is_null() {
            let lib = libc::dlopen(library.as_ptr(), libc::RTLD_LAZY);
            if !lib.is_null() {
                p = libc::dlsym(lib, name.as_ptr());
            }
        }
        p
    };
    (!p.is_null()).then_some(p as usize)
}

fn configure_enabled() -> Option<ConfigureEnabledFn> {
    static ADDR: OnceLock<Option<usize>> = OnceLock::new();
    let addr = (*ADDR.get_or_init(|| resolve(SKYLIGHT, SYMBOL)))?;
    // SAFETY: the address comes from `dlsym` of this symbol, which has this C signature (the one
    // display utilities declare); data and function pointers have the same size on Apple targets.
    Some(unsafe { std::mem::transmute::<*mut c_void, ConfigureEnabledFn>(addr as *mut c_void) })
}

fn failed(what: &str, err: CGError) -> DisplayPowerError {
    DisplayPowerError::Platform(format!("{what} failed: {}", err.0))
}

#[derive(Debug, Default, Clone, Copy)]
pub struct MacDisplayPower;

impl MacDisplayPower {
    pub fn new() -> Self {
        MacDisplayPower
    }
}

impl DisplayPower for MacDisplayPower {
    fn control(&self) -> DisplayControl {
        match configure_enabled() {
            Some(_) => DisplayControl::Available,
            None => DisplayControl::Unavailable(
                "this macOS does not have CGSConfigureDisplayEnabled".into(),
            ),
        }
    }

    fn active(&self) -> Result<Vec<PowerDisplay>, DisplayPowerError> {
        super::emulate::power_displays().map_err(|e| DisplayPowerError::Platform(e.to_string()))
    }

    fn set_enabled(&self, id: u32, on: bool) -> Result<(), DisplayPowerError> {
        let Some(configure) = configure_enabled() else {
            return Err(DisplayPowerError::Unavailable(
                "this macOS does not have CGSConfigureDisplayEnabled".into(),
            ));
        };
        let config = begin()?;
        // SAFETY: `config` comes from a successful `CGBeginDisplayConfiguration` and has not been
        // completed or cancelled yet; `configure` has the signature it was resolved with.
        let err = unsafe { configure(config, id, on) };
        if err != CGError::Success.0 {
            cancel(config);
            return Err(DisplayPowerError::Platform(format!(
                "CGSConfigureDisplayEnabled({id}, {on}) failed: {err}"
            )));
        }
        complete(config)
    }
}

fn begin() -> Result<CGDisplayConfigRef, DisplayPowerError> {
    let mut config: CGDisplayConfigRef = std::ptr::null_mut();
    // SAFETY: `config` is a valid place where CoreGraphics leaves the new configuration.
    let err = unsafe { CGBeginDisplayConfiguration(&mut config) };
    if err != CGError::Success || config.is_null() {
        return Err(failed("CGBeginDisplayConfiguration", err));
    }
    Ok(config)
}

fn cancel(config: CGDisplayConfigRef) {
    // SAFETY: `config` comes from a successful `CGBeginDisplayConfiguration` and is used no more.
    let _ = unsafe { CGCancelDisplayConfiguration(config) };
}

fn complete(config: CGDisplayConfigRef) -> Result<(), DisplayPowerError> {
    // SAFETY: `config` comes from a successful `CGBeginDisplayConfiguration`; completing it
    // releases it, and it is used no more.
    let err = unsafe { CGCompleteDisplayConfiguration(config, CGConfigureOption::ForSession) };
    if err != CGError::Success {
        return Err(failed("CGCompleteDisplayConfiguration", err));
    }
    Ok(())
}
