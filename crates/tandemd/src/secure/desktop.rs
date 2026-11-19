//! Following the desktop that receives input.
//!
//! `SendInput` only reaches the desktop of the calling thread. When Windows shows a UAC
//! prompt, the lock screen or the Ctrl+Alt+Del screen it switches the input to the
//! `Winlogon` desktop (or a screen saver one), so the helper's thread has to move there with
//! `SetThreadDesktop` and back when it returns to `Default`.
//!
//! Asking Windows which desktop has the input costs a system call per check, so
//! [`DesktopTracker`] checks at most once per `interval` while injection works, and right away
//! after a failed injection (which is what a desktop switch looks like from `SendInput`).

use std::io;
use std::time::{Duration, Instant};

/// The window station's desktops. On Windows, `OpenInputDesktop` and friends
/// (`windows::desktop`).
pub trait Desktops {
    /// Name of the desktop that receives input now (`Default`, `Winlogon`, `Screen-saver`).
    fn input_name(&mut self) -> io::Result<String>;

    /// Moves the calling thread to the desktop that receives input and returns its name.
    fn attach_input(&mut self) -> io::Result<String>;
}

pub struct DesktopTracker<D: Desktops> {
    desktops: D,
    /// The desktop the thread is on, once attached.
    current: Option<String>,
    last_check: Option<Instant>,
    interval: Duration,
    switches: u64,
}

/// Default time between checks while injection works.
pub const CHECK_INTERVAL: Duration = Duration::from_millis(100);

impl<D: Desktops> DesktopTracker<D> {
    pub fn new(desktops: D, interval: Duration) -> Self {
        DesktopTracker {
            desktops,
            current: None,
            last_check: None,
            interval,
            switches: 0,
        }
    }

    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// How many times the thread changed desktop (for the log).
    pub fn switches(&self) -> u64 {
        self.switches
    }

    pub fn desktops(&self) -> &D {
        &self.desktops
    }

    /// Before injecting: makes sure the thread is on the input desktop, checking only if the
    /// interval passed. `Ok(true)` if it moved.
    pub fn before_send(&mut self, now: Instant) -> io::Result<bool> {
        let due = self
            .last_check
            .is_none_or(|t| now.saturating_duration_since(t) >= self.interval);
        if self.current.is_some() && !due {
            return Ok(false);
        }
        self.check(now)
    }

    /// After a failed injection: checks right away. `Ok(true)` if the thread moved, and then
    /// it is worth retrying once.
    pub fn after_failure(&mut self, now: Instant) -> io::Result<bool> {
        self.check(now)
    }

    fn check(&mut self, now: Instant) -> io::Result<bool> {
        self.last_check = Some(now);
        if let Some(current) = &self.current
            && self.desktops.input_name()? == *current
        {
            return Ok(false);
        }
        let name = self.desktops.attach_input()?;
        let moved = self.current.as_deref() != Some(name.as_str());
        if moved {
            tracing::info!(from = ?self.current, to = %name, "input desktop changed");
            self.switches += 1;
        }
        self.current = Some(name);
        Ok(moved)
    }
}

/// Test double: the test says which desktop has the input; it counts the attaches.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct FakeDesktops {
    pub input: String,
    pub attached: Option<String>,
    pub queries: usize,
    pub attaches: usize,
    pub fail: bool,
}

#[cfg(test)]
impl FakeDesktops {
    pub fn on(name: &str) -> Self {
        FakeDesktops {
            input: name.into(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
impl Desktops for FakeDesktops {
    fn input_name(&mut self) -> io::Result<String> {
        self.queries += 1;
        if self.fail {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        Ok(self.input.clone())
    }

    fn attach_input(&mut self) -> io::Result<String> {
        self.attaches += 1;
        if self.fail {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        self.attached = Some(self.input.clone());
        Ok(self.input.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn attaches_on_the_first_send() {
        let t = Instant::now();
        let mut d = DesktopTracker::new(FakeDesktops::on("Default"), CHECK_INTERVAL);
        assert!(d.before_send(t).unwrap());
        assert_eq!(d.current(), Some("Default"));
        assert_eq!(d.desktops().attaches, 1);
    }

    #[test]
    fn checks_at_most_once_per_interval_while_it_works() {
        let t = Instant::now();
        let mut d = DesktopTracker::new(FakeDesktops::on("Default"), CHECK_INTERVAL);
        d.before_send(t).unwrap();
        for i in 1..10 {
            assert!(!d.before_send(t + ms(i * 5)).unwrap());
        }
        assert_eq!(d.desktops().queries, 0, "no queries inside the interval");
        assert!(!d.before_send(t + ms(100)).unwrap());
        assert_eq!(d.desktops().queries, 1);
        assert_eq!(
            d.desktops().attaches,
            1,
            "the same desktop is not attached again"
        );
    }

    #[test]
    fn follows_a_uac_prompt_and_comes_back() {
        let t = Instant::now();
        let mut d = DesktopTracker::new(FakeDesktops::on("Default"), CHECK_INTERVAL);
        d.before_send(t).unwrap();
        d.desktops.input = "Winlogon".into();
        // Inside the interval the switch is only seen after a failed injection.
        assert!(!d.before_send(t + ms(10)).unwrap());
        assert!(d.after_failure(t + ms(10)).unwrap());
        assert_eq!(d.desktops().attached.as_deref(), Some("Winlogon"));
        // When the prompt closes, the periodic check takes it back.
        d.desktops.input = "Default".into();
        assert!(d.before_send(t + ms(120)).unwrap());
        assert_eq!(d.current(), Some("Default"));
        assert_eq!(d.switches(), 3);
    }

    #[test]
    fn a_failure_on_the_same_desktop_does_not_reattach() {
        let t = Instant::now();
        let mut d = DesktopTracker::new(FakeDesktops::on("Default"), CHECK_INTERVAL);
        d.before_send(t).unwrap();
        assert!(!d.after_failure(t + ms(1)).unwrap());
        assert_eq!(d.desktops().attaches, 1);
    }

    #[test]
    fn errors_are_reported_and_retried_on_the_next_send() {
        let t = Instant::now();
        let mut d = DesktopTracker::new(FakeDesktops::on("Default"), CHECK_INTERVAL);
        d.desktops.fail = true;
        assert!(d.before_send(t).is_err());
        assert_eq!(d.current(), None);
        d.desktops.fail = false;
        // Without a desktop yet it checks again even inside the interval.
        assert!(d.before_send(t + ms(1)).unwrap());
    }
}
