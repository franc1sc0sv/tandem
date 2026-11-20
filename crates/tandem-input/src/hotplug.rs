//! Display changes reported by the system (T-349): a monitor plugged in or unplugged, turned on
//! or off, or its arrangement changed. The agent rescans its monitors right away instead of
//! waiting for the next periodic scan.
//!
//! macOS: `CGDisplayRegisterReconfigurationCallback`, on a thread of its own with a run loop.
//! Windows: a message-only window that receives `WM_DISPLAYCHANGE` and the monitor-interface
//! `WM_DEVICECHANGE` (`RegisterDeviceNotificationW`). Elsewhere there is nothing to watch.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

/// What is called on each change. It must return quickly: it only asks for a rescan.
pub type OnChange = Arc<dyn Fn() + Send + Sync>;

/// While it lives, the system keeps reporting changes; dropping it stops the watch.
pub type Watch = Box<dyn Send>;

/// Something that reports display changes.
pub trait DisplayChanges: Send + Sync {
    fn watch(&self, on_change: OnChange) -> Result<Watch, String>;
}

/// The system's watcher, if this system has one.
pub fn system() -> Option<Arc<dyn DisplayChanges>> {
    #[cfg(target_os = "macos")]
    {
        Some(Arc::new(crate::macos::hotplug::Reconfiguration))
    }
    #[cfg(windows)]
    {
        Some(Arc::new(crate::windows::hotplug::DisplayMessages))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        None
    }
}

/// A test double: [`FakeDisplayChanges::fire`] is a display change.
#[derive(Clone, Default)]
pub struct FakeDisplayChanges {
    watchers: Arc<Mutex<Vec<OnChange>>>,
}

impl fmt::Debug for FakeDisplayChanges {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeDisplayChanges")
            .field("watchers", &self.watchers().len())
            .finish()
    }
}

impl FakeDisplayChanges {
    pub fn new() -> Self {
        FakeDisplayChanges::default()
    }

    fn watchers(&self) -> std::sync::MutexGuard<'_, Vec<OnChange>> {
        self.watchers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The system reports a display change.
    pub fn fire(&self) {
        let watchers: Vec<OnChange> = self.watchers().clone();
        for w in watchers {
            w();
        }
    }
}

/// Removes its callback when dropped, like a real watch.
struct FakeWatch {
    watchers: Arc<Mutex<Vec<OnChange>>>,
    me: OnChange,
}

impl Drop for FakeWatch {
    fn drop(&mut self) {
        self.watchers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|w| !Arc::ptr_eq(w, &self.me));
    }
}

impl DisplayChanges for FakeDisplayChanges {
    fn watch(&self, on_change: OnChange) -> Result<Watch, String> {
        self.watchers().push(on_change.clone());
        Ok(Box::new(FakeWatch {
            watchers: self.watchers.clone(),
            me: on_change,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    #[test]
    fn the_double_calls_each_watch_until_it_is_dropped() {
        let fake = FakeDisplayChanges::new();
        let n = Arc::new(AtomicU32::new(0));
        let counter = n.clone();
        let watch = fake
            .watch(Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap();
        fake.fire();
        fake.fire();
        assert_eq!(n.load(Ordering::SeqCst), 2);
        drop(watch);
        fake.fire();
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }
}
