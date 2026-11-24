//! What the service does with the helper: keep exactly one, in the session that owns the
//! console, and bring it back when it dies.
//!
//! [`Supervisor`] is a state machine over [`SessionHost`]; the Windows host
//! (`windows::host`) spawns with `CreateProcessAsUser`, and the tests use a double. Every
//! event ends in [`Supervisor::reconcile`]: whatever woke the service up (a WTS
//! notification, the helper's exit or the periodic poll), the decision is the same and only
//! depends on what is true now.

use std::io;
use std::time::{Duration, Instant};

/// Console session id that means "no session is attached to the console right now" (it
/// happens for a moment while switching users).
pub const NO_SESSION: u32 = 0xFFFF_FFFF;

/// The system around the service.
pub trait SessionHost {
    /// A running helper (a process handle on Windows).
    type Helper;

    /// The session attached to the physical console, if there is one
    /// (`WTSGetActiveConsoleSessionId`).
    fn console_session(&mut self) -> Option<u32>;

    /// Launches the helper in `session`.
    fn spawn(&mut self, session: u32) -> io::Result<Self::Helper>;

    /// `Some(exit code)` if the helper already ended.
    fn exited(&mut self, helper: &mut Self::Helper) -> Option<u32>;

    /// Ends the helper and waits for it.
    fn kill(&mut self, helper: Self::Helper);

    /// Ctrl+Alt+Del (`SendSAS`).
    fn send_sas(&mut self) -> io::Result<()>;
}

/// Where the helper's token comes from: the service's own process token (LocalSystem), never
/// another process's. Picking a process by name (`winlogon.exe`) would let a user plant a
/// renamed binary and have the service give *their* token UIAccess.
pub trait TokenSource {
    type Token;

    /// A primary copy of the service process's own token.
    fn duplicate_own(&mut self) -> io::Result<Self::Token>;

    /// The token's user is LocalSystem (`WinLocalSystemSid`).
    fn is_local_system(&mut self, token: &Self::Token) -> io::Result<bool>;

    /// Moves the token to `session` (`TokenSessionId`, needs SeTcbPrivilege).
    fn set_session(&mut self, token: &Self::Token, session: u32) -> io::Result<()>;

    /// The token's session (`TokenSessionId`).
    fn session(&mut self, token: &Self::Token) -> io::Result<u32>;

    /// `TokenUIAccess = 1`.
    fn enable_ui_access(&mut self, token: &Self::Token) -> io::Result<()>;
}

/// The token the helper is launched with: LocalSystem's, in `session`, with UIAccess. Fails
/// closed: a token that is not LocalSystem's (the service running under another account) or
/// that did not land in `session` never gets UIAccess.
pub fn helper_token<S: TokenSource>(source: &mut S, session: u32) -> io::Result<S::Token> {
    let token = source.duplicate_own()?;
    if !source.is_local_system(&token)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the service does not run as LocalSystem: the helper is not launched",
        ));
    }
    source.set_session(&token, session)?;
    let actual = source.session(&token)?;
    if actual != session {
        return Err(io::Error::other(format!(
            "the helper token is in session {actual}, not {session}"
        )));
    }
    source.enable_ui_access(&token)?;
    Ok(token)
}

/// What can wake the supervisor up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Periodic check (and the helper's exit, which is noticed on it).
    Poll,
    /// A WTS session notification (`SERVICE_CONTROL_SESSIONCHANGE`).
    SessionChange,
    /// The helper asked for Ctrl+Alt+Del.
    Sas,
    /// The SCM asked the service to stop.
    Stop,
}

/// Delay before relaunching a helper that died: it doubles on each failure up to `max`, and
/// a helper that lived `stable` starts again from `min`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub min: Duration,
    pub max: Duration,
    pub stable: Duration,
    failures: u32,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff::new(
            Duration::from_secs(1),
            Duration::from_secs(30),
            Duration::from_secs(60),
        )
    }
}

impl Backoff {
    pub fn new(min: Duration, max: Duration, stable: Duration) -> Self {
        Backoff {
            min,
            max,
            stable,
            failures: 0,
        }
    }

    /// Records a failure and returns how long to wait before the next attempt.
    pub fn fail(&mut self) -> Duration {
        let delay = self
            .min
            .checked_mul(1u32 << self.failures.min(16))
            .unwrap_or(self.max)
            .min(self.max);
        self.failures = self.failures.saturating_add(1);
        delay
    }

    pub fn reset(&mut self) {
        self.failures = 0;
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }
}

struct Running<T> {
    session: u32,
    helper: T,
    since: Instant,
}

/// What happened in a reconcile, for the log and the tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Spawned {
        session: u32,
    },
    SpawnFailed {
        session: u32,
        retry_in: Duration,
    },
    Exited {
        session: u32,
        code: u32,
        retry_in: Duration,
    },
    /// The console moved to another session (or to none): the old helper is ended.
    Killed {
        session: u32,
    },
    SasSent,
    SasFailed(String),
    Stopped,
}

pub struct Supervisor<H: SessionHost> {
    host: H,
    running: Option<Running<H::Helper>>,
    backoff: Backoff,
    /// No new helper before this instant.
    retry_at: Option<Instant>,
    stopped: bool,
}

impl<H: SessionHost> Supervisor<H> {
    pub fn new(host: H, backoff: Backoff) -> Self {
        Supervisor {
            host,
            running: None,
            backoff,
            retry_at: None,
            stopped: false,
        }
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    pub fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }

    /// The session of the running helper.
    pub fn session(&self) -> Option<u32> {
        self.running.as_ref().map(|r| r.session)
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// When the next poll is needed at the latest: the pending retry, if any.
    pub fn retry_at(&self) -> Option<Instant> {
        self.retry_at
    }

    pub fn handle(&mut self, event: Event, now: Instant) -> Vec<Action> {
        if self.stopped {
            return Vec::new();
        }
        match event {
            Event::Stop => {
                let mut out = Vec::new();
                if let Some(r) = self.running.take() {
                    self.host.kill(r.helper);
                    out.push(Action::Killed { session: r.session });
                }
                self.stopped = true;
                out.push(Action::Stopped);
                out
            }
            Event::Sas => vec![match self.host.send_sas() {
                Ok(()) => Action::SasSent,
                Err(e) => Action::SasFailed(e.to_string()),
            }],
            Event::Poll => self.reconcile(now),
            // A new session is a fresh start: a helper that kept failing in the old one says
            // nothing about this one.
            Event::SessionChange => {
                let target = self.target();
                if self.session() != target {
                    self.retry_at = None;
                    self.backoff.reset();
                }
                self.reconcile(now)
            }
        }
    }

    fn target(&mut self) -> Option<u32> {
        // Session 0 is the services' one: it never has a desktop to drive.
        self.host
            .console_session()
            .filter(|&s| s != NO_SESSION && s != 0)
    }

    /// Leaves one helper in the console's session if it can, and none elsewhere.
    pub fn reconcile(&mut self, now: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        if let Some(r) = self.running.as_mut()
            && let Some(code) = self.host.exited(&mut r.helper)
        {
            let lived = now.saturating_duration_since(r.since);
            if lived >= self.backoff.stable {
                self.backoff.reset();
            }
            let retry_in = self.backoff.fail();
            self.retry_at = Some(now + retry_in);
            out.push(Action::Exited {
                session: r.session,
                code,
                retry_in,
            });
            self.running = None;
        }

        let target = self.target();
        if let Some(r) = self.running.take_if(|r| Some(r.session) != target) {
            self.host.kill(r.helper);
            out.push(Action::Killed { session: r.session });
        }

        let due = self.retry_at.is_none_or(|t| now >= t);
        if let (None, Some(session), true) = (&self.running, target, due) {
            match self.host.spawn(session) {
                Ok(helper) => {
                    self.retry_at = None;
                    self.running = Some(Running {
                        session,
                        helper,
                        since: now,
                    });
                    out.push(Action::Spawned { session });
                }
                Err(e) => {
                    let retry_in = self.backoff.fail();
                    self.retry_at = Some(now + retry_in);
                    tracing::warn!(session, error = %e, "could not launch the helper");
                    out.push(Action::SpawnFailed { session, retry_in });
                }
            }
        }
        out
    }
}

/// Test double of [`SessionHost`]: a console session that the test moves and helpers that
/// it ends by hand.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct FakeHost {
    pub console: Option<u32>,
    pub next_id: u32,
    /// `(id, session)` of the live helpers.
    pub alive: Vec<(u32, u32)>,
    /// Exit codes the test set for helpers (by id).
    pub exits: std::collections::HashMap<u32, u32>,
    pub spawns: Vec<u32>,
    pub kills: Vec<u32>,
    pub fail_spawn: bool,
    pub fail_sas: bool,
    pub sas: usize,
    /// Every spawn builds its token through [`helper_token`], as the Windows host does.
    pub tokens: FakeTokens,
}

/// Test double of [`TokenSource`]: a token is `(is LocalSystem, session)`.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct FakeTokens {
    /// The service runs under another account.
    pub not_system: bool,
    /// `set_session` does nothing (the session stays 0).
    pub ignore_session: bool,
    /// Sessions whose token got UIAccess.
    pub ui_access: Vec<u32>,
}

#[cfg(test)]
impl TokenSource for FakeTokens {
    type Token = std::cell::Cell<(bool, u32)>;

    fn duplicate_own(&mut self) -> io::Result<Self::Token> {
        Ok(std::cell::Cell::new((!self.not_system, 0)))
    }

    fn is_local_system(&mut self, token: &Self::Token) -> io::Result<bool> {
        Ok(token.get().0)
    }

    fn set_session(&mut self, token: &Self::Token, session: u32) -> io::Result<()> {
        if !self.ignore_session {
            token.set((token.get().0, session));
        }
        Ok(())
    }

    fn session(&mut self, token: &Self::Token) -> io::Result<u32> {
        Ok(token.get().1)
    }

    fn enable_ui_access(&mut self, token: &Self::Token) -> io::Result<()> {
        self.ui_access.push(token.get().1);
        Ok(())
    }
}

#[cfg(test)]
impl FakeHost {
    pub fn crash(&mut self, id: u32, code: u32) {
        self.exits.insert(id, code);
    }
}

#[cfg(test)]
impl SessionHost for FakeHost {
    type Helper = u32;

    fn console_session(&mut self) -> Option<u32> {
        self.console
    }

    fn spawn(&mut self, session: u32) -> io::Result<u32> {
        self.spawns.push(session);
        if self.fail_spawn {
            return Err(io::Error::other("CreateProcessAsUser failed"));
        }
        helper_token(&mut self.tokens, session)?;
        self.next_id += 1;
        self.alive.push((self.next_id, session));
        Ok(self.next_id)
    }

    fn exited(&mut self, helper: &mut u32) -> Option<u32> {
        let code = self.exits.get(helper).copied()?;
        self.alive.retain(|(id, _)| id != helper);
        Some(code)
    }

    fn kill(&mut self, helper: u32) {
        self.kills.push(helper);
        self.alive.retain(|(id, _)| *id != helper);
    }

    fn send_sas(&mut self) -> io::Result<()> {
        if self.fail_sas {
            return Err(io::Error::other(
                "SoftwareSASGeneration does not allow services",
            ));
        }
        self.sas += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn sup(console: Option<u32>) -> Supervisor<FakeHost> {
        Supervisor::new(
            FakeHost {
                console,
                ..Default::default()
            },
            Backoff::default(),
        )
    }

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn launches_one_helper_in_the_console_session() {
        let t = Instant::now();
        let mut s = sup(Some(1));
        assert_eq!(s.handle(Event::Poll, t), [Action::Spawned { session: 1 }]);
        // Polling again leaves it alone.
        assert!(s.handle(Event::Poll, t + secs(1)).is_empty());
        assert_eq!(s.host().alive, [(1, 1)]);
    }

    #[test]
    fn nothing_runs_without_a_console_session_or_in_session_zero() {
        let t = Instant::now();
        for console in [None, Some(NO_SESSION), Some(0)] {
            let mut s = sup(console);
            assert!(s.handle(Event::Poll, t).is_empty());
            assert!(s.host().spawns.is_empty());
        }
    }

    #[test]
    fn fast_user_switching_moves_the_helper() {
        let t = Instant::now();
        let mut s = sup(Some(1));
        s.handle(Event::Poll, t);
        // While switching there is no session for a moment: the old helper goes away.
        s.host_mut().console = Some(NO_SESSION);
        assert_eq!(
            s.handle(Event::SessionChange, t + secs(1)),
            [Action::Killed { session: 1 }]
        );
        s.host_mut().console = Some(2);
        assert_eq!(
            s.handle(Event::SessionChange, t + secs(2)),
            [Action::Spawned { session: 2 }]
        );
        assert_eq!(s.host().alive, [(2, 2)]);
        assert_eq!(s.session(), Some(2));
    }

    #[test]
    fn a_session_change_in_the_same_session_keeps_the_helper() {
        // Lock, unlock and sign-in happen inside the same session: the helper follows the
        // desktop by itself.
        let t = Instant::now();
        let mut s = sup(Some(1));
        s.handle(Event::Poll, t);
        assert!(s.handle(Event::SessionChange, t + secs(1)).is_empty());
        assert!(s.host().kills.is_empty());
    }

    #[test]
    fn a_dead_helper_comes_back_with_a_growing_delay() {
        let t = Instant::now();
        let mut s = sup(Some(1));
        s.handle(Event::Poll, t);
        s.host_mut().crash(1, 3);
        assert_eq!(
            s.handle(Event::Poll, t + secs(2)),
            [Action::Exited {
                session: 1,
                code: 3,
                retry_in: secs(1)
            }]
        );
        // Not before the delay.
        assert!(s.handle(Event::Poll, t + secs(2)).is_empty());
        assert_eq!(
            s.handle(Event::Poll, t + secs(3)),
            [Action::Spawned { session: 1 }]
        );
        // It dies right away again: twice the delay.
        s.host_mut().crash(2, 1);
        let out = s.handle(Event::Poll, t + secs(4));
        assert_eq!(
            out,
            [Action::Exited {
                session: 1,
                code: 1,
                retry_in: secs(2)
            }]
        );
        assert_eq!(s.retry_at(), Some(t + secs(6)));
    }

    #[test]
    fn a_helper_that_lived_long_restarts_from_the_minimum() {
        let t = Instant::now();
        let mut s = sup(Some(1));
        s.handle(Event::Poll, t);
        // Each helper lives 100 s (more than `stable`) before dying.
        for i in 1..5 {
            let id = s.host().next_id;
            s.host_mut().crash(id, 1);
            let out = s.handle(Event::Poll, t + secs(100 * i));
            assert!(
                matches!(out[..], [Action::Exited { retry_in, .. }] if retry_in == secs(1)),
                "{out:?}"
            );
            s.handle(Event::Poll, t + secs(100 * i + 1));
        }
        assert_eq!(s.backoff.failures(), 1);
    }

    #[test]
    fn a_failed_spawn_is_retried_later() {
        let t = Instant::now();
        let mut s = sup(Some(1));
        s.host_mut().fail_spawn = true;
        assert_eq!(
            s.handle(Event::Poll, t),
            [Action::SpawnFailed {
                session: 1,
                retry_in: secs(1)
            }]
        );
        assert!(s.handle(Event::Poll, t).is_empty());
        s.host_mut().fail_spawn = false;
        assert_eq!(
            s.handle(Event::Poll, t + secs(1)),
            [Action::Spawned { session: 1 }]
        );
    }

    #[test]
    fn a_new_session_does_not_inherit_the_old_backoff() {
        let t = Instant::now();
        let mut s = sup(Some(1));
        s.host_mut().fail_spawn = true;
        for i in 0..5 {
            s.handle(Event::Poll, t + secs(i * 60));
        }
        s.host_mut().fail_spawn = false;
        s.host_mut().console = Some(2);
        assert_eq!(
            s.handle(Event::SessionChange, t + secs(241)),
            [Action::Spawned { session: 2 }]
        );
    }

    #[test]
    fn sas_goes_to_the_host_and_failures_are_reported() {
        let t = Instant::now();
        let mut s = sup(Some(1));
        assert_eq!(s.handle(Event::Sas, t), [Action::SasSent]);
        s.host_mut().fail_sas = true;
        assert!(matches!(
            s.handle(Event::Sas, t)[..],
            [Action::SasFailed(_)]
        ));
        assert_eq!(s.host().sas, 1);
    }

    #[test]
    fn stop_ends_the_helper_and_ignores_the_rest() {
        let t = Instant::now();
        let mut s = sup(Some(1));
        s.handle(Event::Poll, t);
        assert_eq!(
            s.handle(Event::Stop, t),
            [Action::Killed { session: 1 }, Action::Stopped]
        );
        assert!(s.is_stopped());
        assert!(s.handle(Event::Poll, t + secs(5)).is_empty());
        assert!(s.host().alive.is_empty());
    }

    #[test]
    fn the_helper_token_is_the_service_token_in_the_session_with_ui_access() {
        let mut tokens = FakeTokens::default();
        let token = helper_token(&mut tokens, 3).unwrap();
        assert_eq!(token.get(), (true, 3));
        assert_eq!(tokens.ui_access, [3]);
    }

    #[test]
    fn a_token_that_is_not_local_system_never_gets_ui_access() {
        let mut tokens = FakeTokens {
            not_system: true,
            ..Default::default()
        };
        let e = helper_token(&mut tokens, 1).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        assert!(tokens.ui_access.is_empty());
    }

    #[test]
    fn a_token_left_in_another_session_never_gets_ui_access() {
        let mut tokens = FakeTokens {
            ignore_session: true,
            ..Default::default()
        };
        assert!(helper_token(&mut tokens, 1).is_err());
        assert!(tokens.ui_access.is_empty());
    }

    #[test]
    fn the_supervisor_launches_nothing_with_a_non_system_token() {
        let t = Instant::now();
        let mut s = sup(Some(1));
        s.host_mut().tokens.not_system = true;
        assert_eq!(
            s.handle(Event::Poll, t),
            [Action::SpawnFailed {
                session: 1,
                retry_in: secs(1)
            }]
        );
        assert!(s.host().alive.is_empty());
        assert!(s.host().tokens.ui_access.is_empty());
        assert_eq!(s.session(), None);
    }

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let mut b = Backoff::default();
        let delays: Vec<_> = (0..8).map(|_| b.fail().as_secs()).collect();
        assert_eq!(delays, [1, 2, 4, 8, 16, 30, 30, 30]);
        b.reset();
        assert_eq!(b.fail(), secs(1));
    }

    proptest! {
        /// Whatever happens, there is never more than one helper, it is always in the console
        /// session, and after a stop there is none.
        #[test]
        fn at_most_one_helper_in_the_console_session(
            ops in proptest::collection::vec((0u8..6, 0u32..4, 0u64..90), 1..60)
        ) {
            let t = Instant::now();
            let mut s = sup(Some(1));
            let mut now = t;
            for (op, arg, dt) in ops {
                now += Duration::from_secs(dt);
                match op {
                    0 => { s.handle(Event::Poll, now); }
                    1 => {
                        s.host_mut().console = match arg { 0 => None, 3 => Some(NO_SESSION), n => Some(n) };
                        s.handle(Event::SessionChange, now);
                    }
                    2 => {
                        if let Some(&(id, _)) = s.host().alive.first() {
                            s.host_mut().crash(id, arg);
                        }
                        s.handle(Event::Poll, now);
                    }
                    3 => { s.host_mut().fail_spawn = arg % 2 == 0; s.handle(Event::Poll, now); }
                    4 => { s.handle(Event::Sas, now); }
                    _ => { s.handle(Event::Stop, now); }
                }
                let alive = &s.host().alive;
                prop_assert!(alive.len() <= 1, "{alive:?}");
                if let Some(&(_, session)) = alive.first() {
                    prop_assert_eq!(Some(session), s.host().console);
                    prop_assert_eq!(s.session(), Some(session));
                }
                if s.is_stopped() {
                    prop_assert!(alive.is_empty());
                }
            }
        }
    }
}
