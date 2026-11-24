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
    CGConfigureDisplayMirrorOfDisplay, CGConfigureOption, CGDirectDisplayID, CGDisplayConfigRef,
    CGDisplayIsActive, CGDisplayIsBuiltin, CGDisplayIsMain, CGDisplayMirrorsDisplay,
    CGDisplayModelNumber, CGDisplaySerialNumber, CGDisplayVendorNumber, CGError,
    CGGetOnlineDisplayList,
};

use crate::displays::{
    BuiltinState, DisplayControl, DisplayPower, DisplayPowerError, PowerDisplay, SystemDisplay,
    mac_display_key,
};

/// Maximum number of displays requested from CoreGraphics; plenty for a desktop.
const MAX_DISPLAYS: usize = 32;

/// `kCGNullDirectDisplay`: as the master of a mirror, "mirror nothing".
const NO_DISPLAY: CGDirectDisplayID = 0;

const SKYLIGHT: &CStr = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";
const SYMBOL: &CStr = c"CGSConfigureDisplayEnabled";

/// `CGError CGSConfigureDisplayEnabled(CGDisplayConfigRef, CGDirectDisplayID, bool)`.
// SAFETY: only a type; each call says why it is sound.
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

    fn online(&self) -> Result<Vec<SystemDisplay>, DisplayPowerError> {
        let mut ids: [CGDirectDisplayID; MAX_DISPLAYS] = [0; MAX_DISPLAYS];
        let mut count: u32 = 0;
        // SAFETY: `ids` has room for `MAX_DISPLAYS` and `count` is a valid u32; CoreGraphics
        // writes no more than it is told.
        let err =
            unsafe { CGGetOnlineDisplayList(MAX_DISPLAYS as u32, ids.as_mut_ptr(), &mut count) };
        if err != CGError::Success {
            return Err(failed("CGGetOnlineDisplayList", err));
        }
        let count = (count as usize).min(MAX_DISPLAYS);
        Ok(ids[..count]
            .iter()
            .map(|&id| {
                let mirror = CGDisplayMirrorsDisplay(id);
                let mirror_of = (mirror != NO_DISPLAY).then_some(mirror);
                SystemDisplay {
                    id,
                    key: mac_display_key(
                        CGDisplayVendorNumber(id),
                        CGDisplayModelNumber(id),
                        CGDisplaySerialNumber(id),
                    ),
                    builtin: CGDisplayIsBuiltin(id),
                    enabled: CGDisplayIsActive(id) || mirror_of.is_some(),
                    mirror_of,
                    main: CGDisplayIsMain(id),
                }
            })
            .collect())
    }

    fn lid_closed(&self) -> bool {
        super::iokit::lid_closed()
    }

    fn set_builtin(&self, id: u32, state: BuiltinState) -> Result<(), DisplayPowerError> {
        let Some(configure) = configure_enabled() else {
            return Err(DisplayPowerError::Unavailable(
                "this macOS does not have CGSConfigureDisplayEnabled".into(),
            ));
        };
        let on = CGDisplayIsActive(id) || CGDisplayMirrorsDisplay(id) != NO_DISPLAY;
        let config = begin()?;
        let master = match state {
            BuiltinState::Mirror(of) => of,
            BuiltinState::Off | BuiltinState::Own => NO_DISPLAY,
        };
        // Turned on first (a display that is off cannot be set to mirror), then its mirror, and
        // only then off: everything lands in one configuration.
        let mut steps: Vec<(&str, i32)> = Vec::new();
        if state != BuiltinState::Off && !on {
            // SAFETY: `config` comes from a successful `CGBeginDisplayConfiguration` and has not
            // been completed or cancelled yet; `configure` has the signature it was resolved with.
            steps.push(("CGSConfigureDisplayEnabled", unsafe {
                configure(config, id, true)
            }));
        }
        // SAFETY: as above; a null master leaves the display unmirrored.
        let err = unsafe { CGConfigureDisplayMirrorOfDisplay(config, id, master) };
        steps.push(("CGConfigureDisplayMirrorOfDisplay", err.0));
        if state == BuiltinState::Off && on {
            // SAFETY: as above.
            steps.push(("CGSConfigureDisplayEnabled", unsafe {
                configure(config, id, false)
            }));
        }
        if let Some((what, err)) = steps.into_iter().find(|(_, e)| *e != CGError::Success.0) {
            cancel(config);
            return Err(DisplayPowerError::Platform(format!(
                "{what}({id}, {state:?}) failed: {err}"
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
