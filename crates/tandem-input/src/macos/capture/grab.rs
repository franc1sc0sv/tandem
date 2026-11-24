//! Swallowing input and freezing/hiding the cursor, in a balanced way.
//!
//! `CGDisplayHideCursor` keeps a counter: every hide needs its show, or the
//! cursor stays invisible. This guarantees that it is hidden and frozen only once for
//! each time input is taken, and that everything is given back even if the grab is
//! dropped from the tap thread (the event consumer disappeared) or on
//! destroying the capture.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::CaptureError;
use crate::geometry::Point;

/// What the grab needs from the system. On macOS `sys` implements it with
/// CoreGraphics; tests use a double.
pub trait CursorBackend: Send {
    /// `true`: the mouse stops moving the cursor (but its deltas keep reaching
    /// the tap). `CGAssociateMouseAndMouseCursorPosition(!frozen)`.
    fn set_frozen(&mut self, frozen: bool) -> Result<(), CaptureError>;
    /// Hide or show the cursor even if Tandem is not the active app.
    fn set_hidden(&mut self, hidden: bool) -> Result<(), CaptureError>;
    /// Put the cursor at `pos` (global coordinates in points).
    fn warp(&mut self, pos: Point) -> Result<(), CaptureError>;
    /// Secure Event Input (a password field has focus): the tap does not see
    /// the keyboard, so there is no crossing to the other machine.
    fn secure_input_active(&self) -> bool;
}

struct Inner<C> {
    backend: C,
    /// What is really applied. They go separately so that a failure halfway
    /// does not unbalance the hide counter: what could not be
    /// undone is retried next time.
    frozen: bool,
    hidden: bool,
}

/// State of the grab, shared between the capture's owner and the tap thread.
pub struct Grab<C> {
    /// What the tap reads on each event, without blocking.
    grabbed: AtomicBool,
    inner: Mutex<Inner<C>>,
}

impl<C: CursorBackend> Grab<C> {
    pub fn new(backend: C) -> Self {
        Self {
            grabbed: AtomicBool::new(false),
            inner: Mutex::new(Inner {
                backend,
                frozen: false,
                hidden: false,
            }),
        }
    }

    /// Whether the tap has to swallow input.
    pub fn is_grabbed(&self) -> bool {
        self.grabbed.load(Ordering::Acquire)
    }

    /// Takes (`true`) or gives back (`false`) local input. Repeating the same
    /// value does nothing.
    ///
    /// Taking it fails with [`CaptureError::SecureInput`] if a password
    /// field is active. If freezing or hiding fails, what was done is undone and
    /// input stays local.
    pub fn set(&self, grab: bool) -> Result<(), CaptureError> {
        let mut inner = self.lock();
        if grab {
            if self.is_grabbed() {
                return Ok(());
            }
            if inner.backend.secure_input_active() {
                return Err(CaptureError::SecureInput);
            }
            if !inner.frozen {
                inner.backend.set_frozen(true)?;
                inner.frozen = true;
            }
            if !inner.hidden {
                if let Err(e) = inner.backend.set_hidden(true) {
                    // The most important thing is not to leave the mouse dead.
                    if inner.backend.set_frozen(false).is_ok() {
                        inner.frozen = false;
                    }
                    return Err(e);
                }
                inner.hidden = true;
            }
            self.grabbed.store(true, Ordering::Release);
            Ok(())
        } else {
            // First it stops swallowing: even if the rest fails, the Mac gets its
            // keyboard back.
            self.grabbed.store(false, Ordering::Release);
            let mut result = Ok(());
            if inner.hidden {
                match inner.backend.set_hidden(false) {
                    Ok(()) => inner.hidden = false,
                    Err(e) => result = Err(e),
                }
            }
            if inner.frozen {
                match inner.backend.set_frozen(false) {
                    Ok(()) => inner.frozen = false,
                    Err(e) => result = result.and(Err(e)),
                }
            }
            result
        }
    }

    /// Gives input back regardless of errors (on destroying the capture, or
    /// when the event consumer no longer exists).
    pub fn release(&self) {
        let _ = self.set(false);
    }

    /// Takes the cursor to `pos`. With local input, it reassociates the mouse:
    /// that way macOS does not ignore movement during the quarter second that follows
    /// a warp.
    pub fn warp(&self, pos: Point) -> Result<(), CaptureError> {
        let mut inner = self.lock();
        inner.backend.warp(pos)?;
        if !self.is_grabbed() {
            inner.backend.set_frozen(false)?;
            inner.frozen = false;
        }
        Ok(())
    }

    pub fn secure_input_active(&self) -> bool {
        self.lock().backend.secure_input_active()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner<C>> {
        // A panic with the lock held does not invalidate the state (they are booleans),
        // and giving the cursor back matters more than propagating the poison.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Call {
        Frozen(bool),
        Hidden(bool),
        Warp(Point),
    }

    /// Test double: records the calls and simulates the
    /// `CGDisplayHideCursor` counter.
    #[derive(Debug, Default)]
    pub struct State {
        pub calls: Vec<Call>,
        pub hide_count: i32,
        pub frozen: bool,
        pub secure_input: bool,
        pub fail_frozen: bool,
        pub fail_hidden: bool,
    }

    #[derive(Debug, Clone, Default)]
    pub struct FakeCursor(pub Arc<Mutex<State>>);

    impl FakeCursor {
        pub fn state(&self) -> std::sync::MutexGuard<'_, State> {
            self.0.lock().unwrap()
        }
    }

    impl CursorBackend for FakeCursor {
        fn set_frozen(&mut self, frozen: bool) -> Result<(), CaptureError> {
            let mut s = self.state();
            s.calls.push(Call::Frozen(frozen));
            if s.fail_frozen {
                return Err(CaptureError::Cursor {
                    op: "CGAssociateMouseAndMouseCursorPosition",
                    code: 1001,
                });
            }
            s.frozen = frozen;
            Ok(())
        }

        fn set_hidden(&mut self, hidden: bool) -> Result<(), CaptureError> {
            let mut s = self.state();
            s.calls.push(Call::Hidden(hidden));
            if s.fail_hidden {
                return Err(CaptureError::Cursor {
                    op: "CGDisplayHideCursor",
                    code: 1001,
                });
            }
            s.hide_count += if hidden { 1 } else { -1 };
            Ok(())
        }

        fn warp(&mut self, pos: Point) -> Result<(), CaptureError> {
            self.state().calls.push(Call::Warp(pos));
            Ok(())
        }

        fn secure_input_active(&self) -> bool {
            self.state().secure_input
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::fake::{Call, FakeCursor};
    use super::*;

    fn grab() -> (Grab<FakeCursor>, FakeCursor) {
        let cursor = FakeCursor::default();
        (Grab::new(cursor.clone()), cursor)
    }

    #[test]
    fn taking_freezes_and_hides_once() {
        let (g, c) = grab();
        assert!(!g.is_grabbed());
        g.set(true).unwrap();
        g.set(true).unwrap();
        assert!(g.is_grabbed());
        {
            let s = c.state();
            assert_eq!(s.calls, vec![Call::Frozen(true), Call::Hidden(true)]);
            assert_eq!(s.hide_count, 1);
            assert!(s.frozen);
        }
        g.set(false).unwrap();
        g.set(false).unwrap();
        assert!(!g.is_grabbed());
        let s = c.state();
        assert_eq!(s.hide_count, 0);
        assert!(!s.frozen);
        assert_eq!(s.calls.len(), 4);
    }

    #[test]
    fn with_secure_input_it_is_not_taken() {
        let (g, c) = grab();
        c.state().secure_input = true;
        assert!(g.secure_input_active());
        assert!(matches!(g.set(true), Err(CaptureError::SecureInput)));
        assert!(!g.is_grabbed());
        assert!(c.state().calls.is_empty());
        // Releasing still works.
        g.set(false).unwrap();
    }

    #[test]
    fn if_hiding_fails_it_is_unfrozen() {
        let (g, c) = grab();
        c.state().fail_hidden = true;
        assert!(g.set(true).is_err());
        assert!(!g.is_grabbed());
        let s = c.state();
        assert!(!s.frozen);
        assert_eq!(s.hide_count, 0);
    }

    #[test]
    fn if_freezing_fails_it_is_not_hidden() {
        let (g, c) = grab();
        c.state().fail_frozen = true;
        assert!(g.set(true).is_err());
        assert!(!g.is_grabbed());
        assert_eq!(c.state().calls, vec![Call::Frozen(true)]);
    }

    #[test]
    fn releasing_stops_swallowing_even_if_showing_fails() {
        let (g, c) = grab();
        g.set(true).unwrap();
        c.state().fail_hidden = true;
        assert!(g.set(false).is_err());
        assert!(!g.is_grabbed());
        // It tried to unfreeze all the same.
        assert!(!c.state().frozen);
    }

    #[test]
    fn release_from_another_thread() {
        let (g, c) = grab();
        let g = std::sync::Arc::new(g);
        g.set(true).unwrap();
        let g2 = g.clone();
        std::thread::spawn(move || g2.release()).join().unwrap();
        assert!(!g.is_grabbed());
        assert_eq!(c.state().hide_count, 0);
    }

    #[test]
    fn warp_reassociates_only_with_local_focus() {
        let (g, c) = grab();
        let p = Point::new(100.0, 200.0);
        g.warp(p).unwrap();
        assert_eq!(c.state().calls, vec![Call::Warp(p), Call::Frozen(false)]);
        g.set(true).unwrap();
        c.state().calls.clear();
        g.warp(p).unwrap();
        assert_eq!(c.state().calls, vec![Call::Warp(p)]);
        assert!(c.state().frozen);
    }

    proptest! {
        /// Any sequence of take/release (with intermediate failures) leaves the
        /// hide counter at 0 or 1, consistent with `is_grabbed`, and releasing at the
        /// end always leaves the cursor visible and free.
        #[test]
        fn the_cursor_stays_balanced(ops in prop::collection::vec((any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()), 0..40)) {
            let (g, c) = grab();
            for (want, secure, fail_frozen, fail_hidden) in ops {
                {
                    let mut s = c.state();
                    s.secure_input = secure;
                    s.fail_frozen = fail_frozen;
                    s.fail_hidden = fail_hidden;
                }
                let _ = g.set(want);
                let s = c.state();
                prop_assert!(s.hide_count == 0 || s.hide_count == 1);
                if g.is_grabbed() {
                    prop_assert_eq!(s.hide_count, 1);
                    prop_assert!(s.frozen);
                }
            }
            {
                let mut s = c.state();
                s.fail_frozen = false;
                s.fail_hidden = false;
            }
            g.release();
            let s = c.state();
            prop_assert!(!g.is_grabbed());
            prop_assert_eq!(s.hide_count, 0);
            prop_assert!(!s.frozen);
        }
    }
}
