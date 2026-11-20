//! Display changes on macOS (T-349): `CGDisplayRegisterReconfigurationCallback`.
//!
//! CoreGraphics delivers that callback through a run loop, and the agent's main thread runs
//! tokio, not a run loop. So a thread of its own registers the callback, runs its run loop in
//! short slices and, between slices, compares the online display list: whichever notices a change
//! first asks for the rescan.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use objc2_core_foundation::{CFRunLoop, kCFRunLoopDefaultMode};

use crate::hotplug::{DisplayChanges, OnChange, Watch};

/// How often the online display list is compared, and the run loop slice.
const SLICE: Duration = Duration::from_millis(250);

// SAFETY: the C signature of `CGDisplayReconfigurationCallBack` (CGDisplayConfiguration.h).
type ReconfigurationCallback = unsafe extern "C-unwind" fn(u32, u32, *mut c_void);

#[link(name = "CoreGraphics", kind = "framework")]
// SAFETY: signatures from the SDK (CGDisplayConfiguration.h, CGDirectDisplay.h): ids and flags are
// `uint32_t`, `CGError` is `int32_t`; the list pointers are valid for the call only.
unsafe extern "C" {
    fn CGDisplayRegisterReconfigurationCallback(
        callback: ReconfigurationCallback,
        user_info: *mut c_void,
    ) -> i32;
    fn CGDisplayRemoveReconfigurationCallback(
        callback: ReconfigurationCallback,
        user_info: *mut c_void,
    ) -> i32;
    fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
}

/// `kCGDisplayBeginConfigurationFlag`: the change is about to happen; the rescan waits for the end.
const BEGIN_CONFIGURATION: u32 = 1;

// SAFETY: called by CoreGraphics only, with the `user_info` registered in `run`: a boxed `OnChange`
// that outlives the registration (see the dereference below).
unsafe extern "C-unwind" fn on_reconfiguration(_display: u32, flags: u32, user_info: *mut c_void) {
    if flags & BEGIN_CONFIGURATION != 0 || user_info.is_null() {
        return;
    }
    // SAFETY: `user_info` is the `OnChange` boxed by `run`, alive until the callback is removed.
    let on_change = unsafe { &*(user_info as *const OnChange) };
    on_change();
}

/// The online displays now, sorted; empty if CoreGraphics did not answer.
fn online() -> Vec<u32> {
    let mut ids = [0u32; 32];
    let mut count = 0u32;
    // SAFETY: `ids` holds the 32 entries announced and `count` is a valid out pointer.
    let err = unsafe { CGGetOnlineDisplayList(32, ids.as_mut_ptr(), &mut count) };
    if err != 0 {
        return Vec::new();
    }
    let mut list = ids[..(count as usize).min(ids.len())].to_vec();
    list.sort_unstable();
    list
}

/// Registers the callback with `user_info`; `false` if CoreGraphics refused it.
fn register(user_info: *mut c_void) -> bool {
    // SAFETY: the callback has the documented signature and `user_info` outlives the registration.
    unsafe { CGDisplayRegisterReconfigurationCallback(on_reconfiguration, user_info) == 0 }
}

fn unregister(user_info: *mut c_void) {
    // SAFETY: the same pair that was registered.
    unsafe { CGDisplayRemoveReconfigurationCallback(on_reconfiguration, user_info) };
}

fn run(on_change: OnChange, stop: Arc<AtomicBool>) {
    let boxed = Box::into_raw(Box::new(on_change.clone()));
    let user_info = boxed.cast::<c_void>();
    let registered = register(user_info);
    if !registered {
        tracing::debug!("CGDisplayRegisterReconfigurationCallback refused: polling only");
    }
    let mut last = online();
    // SAFETY: CoreFoundation constant, valid for the whole process.
    let mode = unsafe { kCFRunLoopDefaultMode };
    while !stop.load(Ordering::Acquire) {
        let started = std::time::Instant::now();
        CFRunLoop::run_in_mode(mode, SLICE.as_secs_f64(), false);
        // Without sources the run loop returns at once: wait the rest of the slice.
        if let Some(rest) = SLICE.checked_sub(started.elapsed()) {
            std::thread::sleep(rest);
        }
        let now = online();
        if now != last {
            last = now;
            on_change();
        }
    }
    if registered {
        unregister(user_info);
    }
    // SAFETY: the callback no longer holds the pointer: it was removed above (or never added).
    drop(unsafe { Box::from_raw(boxed) });
}

/// Stops the thread when dropped.
struct ThreadWatch {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ThreadWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The system watcher on macOS.
#[derive(Debug, Clone, Copy)]
pub struct Reconfiguration;

impl DisplayChanges for Reconfiguration {
    fn watch(&self, on_change: OnChange) -> Result<Watch, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("tandem-hotplug".into())
            .spawn(move || run(on_change, flag))
            .map_err(|e| format!("could not start the display watch: {e}"))?;
        Ok(Box::new(ThreadWatch {
            stop,
            thread: Some(thread),
        }))
    }
}
