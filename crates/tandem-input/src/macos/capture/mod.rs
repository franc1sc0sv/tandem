//! Keyboard and mouse capture on macOS (docs/research/input-engine.md).
//!
//! An **active** CGEventTap at `kCGHIDEventTap`, on its own thread with its
//! CFRunLoop, sees all input before any app. Each event is translated
//! to the portable model ([`translate`]) and delivered to the consumer (the session
//! driver); while the focus is on the other machine the tap swallows it (returns
//! NULL) and the local cursor is frozen and hidden ([`grab`]).
//!
//! - If the system turns the tap off (`kCGEventTapDisabledByTimeout` or
//!   `…ByUserInput`), it is turned on again in the same callback and recorded.
//! - With Secure Event Input (a password field) input is not taken: the
//!   tap does not see the keyboard and the user would end up typing blind.
//! - The Accessibility permission (TCC) is needed for an active tap; without it,
//!   [`MacCapture::start`] fails before creating anything ([`Permissions`]).
//! - If the event consumer disappears with the input taken, the tap gives it
//!   back on its own: the Mac is never left without a keyboard.

pub mod grab;
pub mod translate;

#[cfg(target_os = "macos")]
mod sys;

use thiserror::Error;

pub use grab::{CursorBackend, Grab};
#[cfg(target_os = "macos")]
pub use sys::{MacCapture, SysCursor, permissions, request_permissions, secure_input_active};
pub use translate::{
    INJECTED_MARKER, RawEvent, TapAction, TapDisabledBy, TapDisabledCount, Translator,
};

#[derive(Debug, Error)]
pub enum CaptureError {
    /// A TCC permission is missing. It is requested with `request_permissions` and the user
    /// grants it in System Settings → Privacy & Security.
    #[error("the {0} permission is missing (System Settings → Privacy & Security)")]
    Permission(Permission),
    /// A password field is active (Secure Event Input).
    #[error("a password field is active (Secure Event Input); the screen is not crossed")]
    SecureInput,
    /// `CGEventTapCreate` returned NULL (normally, permissions).
    #[error("the CGEventTap could not be created")]
    TapCreate,
    /// The tap thread did not start or died before reporting.
    #[error("the event tap thread did not start: {0}")]
    Thread(String),
    /// A CoreGraphics call on the cursor returned a `CGError`.
    #[error("{op} failed with CGError {code}")]
    Cursor { op: &'static str, code: i32 },
}

/// TCC permissions that matter for capturing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Accessibility: needed for an active tap (and for injecting).
    Accessibility,
    /// Input Monitoring: enough for a tap that only listens.
    InputMonitoring,
}

impl std::fmt::Display for Permission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Permission::Accessibility => "Accessibility",
            Permission::InputMonitoring => "Input Monitoring",
        })
    }
}

/// State of the process's TCC permissions. The permission is tied to the binary's
/// signature: a stably signed `.app` is needed so it is not asked for again
/// on every build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Permissions {
    pub accessibility: bool,
    pub input_monitoring: bool,
}

impl Permissions {
    /// What is missing for an active tap, which swallows events.
    pub fn missing_for_active_tap(self) -> Option<Permission> {
        (!self.accessibility).then_some(Permission::Accessibility)
    }

    /// Everything that is missing, to show in the UI.
    pub fn missing(self) -> Vec<Permission> {
        let mut v = Vec::new();
        if !self.accessibility {
            v.push(Permission::Accessibility);
        }
        if !self.input_monitoring {
            v.push(Permission::InputMonitoring);
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_active_tap_only_requires_accessibility() {
        let p = Permissions {
            accessibility: true,
            input_monitoring: false,
        };
        assert_eq!(p.missing_for_active_tap(), None);
        assert_eq!(p.missing(), vec![Permission::InputMonitoring]);
        let p = Permissions::default();
        assert_eq!(p.missing_for_active_tap(), Some(Permission::Accessibility));
        assert_eq!(
            p.missing(),
            vec![Permission::Accessibility, Permission::InputMonitoring]
        );
        assert!(
            Permissions {
                accessibility: true,
                input_monitoring: true
            }
            .missing()
            .is_empty()
        );
    }

    #[test]
    fn error_messages_in_english() {
        assert_eq!(
            CaptureError::Permission(Permission::Accessibility).to_string(),
            "the Accessibility permission is missing (System Settings → Privacy & Security)"
        );
        assert_eq!(
            CaptureError::Cursor {
                op: "CGDisplayHideCursor",
                code: 1001
            }
            .to_string(),
            "CGDisplayHideCursor failed with CGError 1001"
        );
    }
}
