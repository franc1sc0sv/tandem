//! Hooks watchdog.
//!
//! Windows removes a low-level hook without warning if a call takes longer than
//! `LowLevelHooksTimeout`. The watchdog is passive: it never injects anything, because
//! injected input resets Windows's inactivity counter and the PC would never
//! lock or suspend. The reference is Raw Input from real devices,
//! which does not depend on the hooks: if Raw Input sees real keyboard (or mouse) and
//! the hook of that type sees nothing for `hook_timeout`, the hook was lost and
//! both are reinstalled. Without real input there is nothing to check.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookKind {
    Keyboard,
    Mouse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchdogConfig {
    /// How long real input can go without the hook seeing it before taking it as
    /// lost. It is also the tolerance between the hook and Raw Input of the same
    /// event, which do not arrive in a fixed order.
    pub hook_timeout: Duration,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            hook_timeout: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogAction {
    Idle,
    Reinstall,
}

#[derive(Debug, Clone, Copy, Default)]
struct Liveness {
    /// Last event the hook saw.
    hook_seen: Option<Instant>,
    /// First real input that the hook has not yet confirmed.
    unanswered: Option<Instant>,
}

impl Liveness {
    fn hook(&mut self, now: Instant) {
        self.hook_seen = Some(now);
        self.unanswered = None;
    }

    fn raw(&mut self, now: Instant, timeout: Duration) {
        // The hook's event may have arrived just before Raw Input's.
        let recent = self
            .hook_seen
            .is_some_and(|seen| now.saturating_duration_since(seen) <= timeout);
        if !recent && self.unanswered.is_none() {
            self.unanswered = Some(now);
        }
    }

    fn lost(&self, now: Instant, timeout: Duration) -> bool {
        self.unanswered
            .is_some_and(|since| now.saturating_duration_since(since) >= timeout)
    }
}

#[derive(Debug)]
pub struct Watchdog {
    config: WatchdogConfig,
    keyboard: Liveness,
    mouse: Liveness,
    reinstalls: u32,
}

impl Watchdog {
    pub fn new(config: WatchdogConfig) -> Self {
        Self {
            config,
            keyboard: Liveness::default(),
            mouse: Liveness::default(),
            reinstalls: 0,
        }
    }

    fn liveness(&mut self, kind: HookKind) -> &mut Liveness {
        match kind {
            HookKind::Keyboard => &mut self.keyboard,
            HookKind::Mouse => &mut self.mouse,
        }
    }

    /// The hook of this type saw an event: it is installed.
    pub fn hook_activity(&mut self, kind: HookKind, now: Instant) {
        self.liveness(kind).hook(now);
    }

    /// Raw Input saw real input (`hDevice != 0`) of this type.
    pub fn real_activity(&mut self, kind: HookKind, now: Instant) {
        let timeout = self.config.hook_timeout;
        self.liveness(kind).raw(now, timeout);
    }

    pub fn reinstalls(&self) -> u32 {
        self.reinstalls
    }

    pub fn tick(&mut self, now: Instant) -> WatchdogAction {
        let timeout = self.config.hook_timeout;
        if self.keyboard.lost(now, timeout) || self.mouse.lost(now, timeout) {
            // Both are reinstalled; the input that follows is compared with the new ones.
            self.keyboard = Liveness::default();
            self.mouse = Liveness::default();
            self.reinstalls += 1;
            return WatchdogAction::Reinstall;
        }
        WatchdogAction::Idle
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const S: Duration = Duration::from_secs(1);
    const MS: Duration = Duration::from_millis(1);

    fn cfg() -> WatchdogConfig {
        WatchdogConfig { hook_timeout: S }
    }

    #[test]
    fn idle_for_minutes_does_nothing() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(cfg());
        // Ten minutes of ticks every 100 ms without any event.
        for i in 0..6000u32 {
            assert_eq!(w.tick(t0 + i * 100 * MS), WatchdogAction::Idle);
        }
        assert_eq!(w.reinstalls(), 0);
    }

    #[test]
    fn hooks_without_raw_input_do_nothing() {
        // A hook that sees events without Raw Input (e.g. injected) is never a reason
        // to act.
        let t0 = Instant::now();
        let mut w = Watchdog::new(cfg());
        for i in 0..100u32 {
            let now = t0 + i * S;
            w.hook_activity(HookKind::Mouse, now);
            assert_eq!(w.tick(now), WatchdogAction::Idle);
        }
    }

    #[test]
    fn keyboard_raw_input_without_hook_reinstalls() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(cfg());
        // The mouse is still alive; from the keyboard only Raw Input arrives.
        for i in 0..10u32 {
            let now = t0 + i * 100 * MS;
            w.hook_activity(HookKind::Mouse, now);
            w.real_activity(HookKind::Mouse, now);
            w.real_activity(HookKind::Keyboard, now);
            assert_eq!(w.tick(now), WatchdogAction::Idle);
        }
        assert_eq!(w.tick(t0 + S), WatchdogAction::Reinstall);
        assert_eq!(w.reinstalls(), 1);
        // After reinstalling, the deadline starts over and the new hook answers.
        assert_eq!(w.tick(t0 + S + 500 * MS), WatchdogAction::Idle);
        w.real_activity(HookKind::Keyboard, t0 + 2 * S);
        w.hook_activity(HookKind::Keyboard, t0 + 2 * S + 5 * MS);
        assert_eq!(w.tick(t0 + 4 * S), WatchdogAction::Idle);
        assert_eq!(w.reinstalls(), 1);
    }

    #[test]
    fn mouse_raw_input_without_hook_reinstalls() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(cfg());
        w.real_activity(HookKind::Mouse, t0);
        assert_eq!(w.tick(t0 + 999 * MS), WatchdogAction::Idle);
        assert_eq!(w.tick(t0 + S), WatchdogAction::Reinstall);
    }

    #[test]
    fn the_hook_may_arrive_before_or_after_raw_input() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(cfg());
        // Hook first.
        w.hook_activity(HookKind::Keyboard, t0);
        w.real_activity(HookKind::Keyboard, t0 + 2 * MS);
        // Raw first.
        w.real_activity(HookKind::Mouse, t0);
        w.hook_activity(HookKind::Mouse, t0 + 2 * MS);
        for i in 0..50u32 {
            assert_eq!(w.tick(t0 + i * 100 * MS), WatchdogAction::Idle);
        }
    }

    #[test]
    fn a_hook_that_died_after_its_last_event_is_detected() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(cfg());
        w.hook_activity(HookKind::Keyboard, t0);
        w.real_activity(HookKind::Keyboard, t0);
        // The hook is lost; real input continues for a while afterwards.
        let later = t0 + 5 * S;
        w.real_activity(HookKind::Keyboard, later);
        assert_eq!(w.tick(later + 500 * MS), WatchdogAction::Idle);
        assert_eq!(w.tick(later + S), WatchdogAction::Reinstall);
    }

    proptest! {
        /// With live hooks (every real input also reaches the hook, before or after
        /// and with an offset smaller than the deadline), it is never reinstalled.
        #[test]
        fn with_live_hooks_it_never_reinstalls(
            steps in prop::collection::vec(
                (0u64..5000, any::<bool>(), any::<bool>(), 0u64..900, 0u64..1200),
                1..80,
            )
        ) {
            let t0 = Instant::now();
            let mut w = Watchdog::new(cfg());
            let mut now = t0;
            for (gap, keyboard, raw_first, skew, tick_after) in steps {
                now += Duration::from_millis(gap);
                let kind = if keyboard { HookKind::Keyboard } else { HookKind::Mouse };
                let first = now;
                let second = now + Duration::from_millis(skew);
                if raw_first {
                    w.real_activity(kind, first);
                    // A tick between the two is still within the deadline.
                    prop_assert_eq!(w.tick(first + Duration::from_millis(skew / 2)), WatchdogAction::Idle);
                    w.hook_activity(kind, second);
                } else {
                    w.hook_activity(kind, first);
                    w.real_activity(kind, second);
                }
                now = second;
                prop_assert_eq!(w.tick(now + Duration::from_millis(tick_after)), WatchdogAction::Idle);
            }
            prop_assert_eq!(w.reinstalls(), 0);
        }

        /// Without any event, however long, it never acts.
        #[test]
        fn without_events_it_never_acts(ticks in prop::collection::vec(0u64..600_000, 1..50)) {
            let t0 = Instant::now();
            let mut w = Watchdog::new(cfg());
            for ms in ticks {
                prop_assert_eq!(w.tick(t0 + Duration::from_millis(ms)), WatchdogAction::Idle);
            }
        }

        /// A dead hook is detected at most one deadline after real input
        /// starts.
        #[test]
        fn a_dead_hook_is_detected_in_time(tick_ms in 10u64..500) {
            let t0 = Instant::now();
            let c = cfg();
            let mut w = Watchdog::new(c);
            let limit = c.hook_timeout + Duration::from_millis(tick_ms);
            let mut now = t0;
            loop {
                now += Duration::from_millis(tick_ms);
                // The mouse is still alive; the keyboard hook no longer sees anything.
                w.hook_activity(HookKind::Mouse, now);
                w.real_activity(HookKind::Mouse, now);
                w.real_activity(HookKind::Keyboard, now);
                if w.tick(now) == WatchdogAction::Reinstall {
                    break;
                }
                prop_assert!(now - t0 <= limit);
            }
        }
    }
}
