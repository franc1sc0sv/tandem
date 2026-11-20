//! No phantom display (T-341, DESIGN §10): while a monitor shows the other machine, this machine
//! turns off its own display of that monitor, so its trackpad and its windows do not go into a
//! screen nobody sees. It turns it back on when the monitor shows this machine again, before a
//! switch to this machine's input ([`DisplayGuard::wake`]), when the agent exits and at start-up.
//!
//! What it turned off is written to a file next to the state before each change, so a restart
//! after a crash turns it back on ([`DisplayGuard::restore_saved`]).
//!
//! Safety rules: it never turns off the last screen, nor one that is not the last in the order
//! the input engine numbers screens (`screen-N`): turning it off would renumber the others, and
//! the cables would then name the wrong screen. A monitor whose input stops being known is turned
//! back on after [`UNKNOWN_GRACE`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tandem_core::{DisplayKey, Machine};
use tandem_input::displays::{DisplayControl, DisplayPower};

/// How long a monitor's input can be unknown (the other agent left, the owner changes) before its
/// display is turned back on: shorter would turn it on and off during a normal owner handover.
pub const UNKNOWN_GRACE: Duration = Duration::from_secs(15);

/// How long a display woken for a switch stays on while the monitor does not show this machine
/// yet: the switch's maximum time with room. If the switch failed, it goes off again after this.
pub const WAKE_HOLD: Duration = Duration::from_secs(30);

/// A display this machine turned off: the monitor it is, its system id and its EDID identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct OffDisplay {
    id: u32,
    vendor: u16,
    product: u16,
    serial: u32,
}

#[derive(Debug, Default)]
struct Inner {
    /// Monitor id → its display that this machine turned off.
    off: BTreeMap<String, OffDisplay>,
    /// Monitors woken for a switch to this machine: kept on until then.
    holds: BTreeMap<String, Instant>,
    /// Since when the input of a monitor whose display is off is not known.
    unknown_since: BTreeMap<String, Instant>,
    /// Why each monitor that shows the other was left on, so it is logged once.
    skipped: BTreeMap<String, String>,
    /// The agent is exiting: nothing goes off any more.
    stopped: bool,
}

/// This machine's display control in the agent.
#[derive(Debug)]
pub struct DisplayGuard {
    power: Arc<dyn DisplayPower>,
    /// Where the displays turned off are saved; `None` keeps them in memory (tests).
    file: Option<PathBuf>,
    inner: Mutex<Inner>,
}

fn lock(m: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl DisplayGuard {
    pub fn new(power: Arc<dyn DisplayPower>, file: Option<PathBuf>) -> Self {
        DisplayGuard {
            power,
            file,
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn control(&self) -> DisplayControl {
        self.power.control()
    }

    /// The monitors whose display this machine has off now.
    pub fn off(&self) -> Vec<String> {
        lock(&self.inner).off.keys().cloned().collect()
    }

    /// Start-up: what a previous run left off (it crashed or was killed) is turned back on.
    pub fn restore_saved(&self) {
        let saved = self.file.as_deref().map(load).unwrap_or_default();
        if saved.is_empty() {
            return;
        }
        tracing::info!(
            monitors = ?saved.keys().collect::<Vec<_>>(),
            "turning back on the displays a previous run left off"
        );
        lock(&self.inner).off.extend(saved);
        self.restore_all();
    }

    /// Turns every display this machine turned off back on: at exit, on a panic and at start-up.
    /// The ones that fail stay recorded, so the next turn tries again.
    pub fn restore_all(&self) {
        let mut inner = lock(&self.inner);
        let off = std::mem::take(&mut inner.off);
        for (monitor, d) in off {
            if let Err(e) = self.power.set_enabled(d.id, true)
                && !self.already_on(d)
            {
                tracing::warn!("could not turn the display of {monitor} back on: {e}");
                inner.off.insert(monitor, d);
            }
        }
        inner.holds.clear();
        inner.unknown_since.clear();
        self.save(&inner);
    }

    /// From the panic hook: the panic may have happened with the lock held on this very thread,
    /// so it never waits for it. Without the lock, it turns on what the file says and leaves the
    /// record as it is.
    pub fn restore_on_panic(&self) {
        match self.inner.try_lock() {
            Ok(inner) => {
                drop(inner);
                self.restore_all();
            }
            Err(TryLockError::Poisoned(p)) => {
                drop(p);
                self.restore_all();
            }
            Err(TryLockError::WouldBlock) => {
                for d in self.file.as_deref().map(load).unwrap_or_default().values() {
                    let _ = self.power.set_enabled(d.id, true);
                }
            }
        }
    }

    /// Exit: everything goes back on and nothing goes off again, whatever the loop still sees.
    pub fn shut_down(&self) {
        lock(&self.inner).stopped = true;
        self.restore_all();
    }

    /// Before a switch to this machine's input of `monitor`: its display goes back on now, so the
    /// monitor never shows a black input, and stays on for [`WAKE_HOLD`] while the switch lands.
    pub fn wake(&self, monitor: &str, now: Instant) -> Result<(), String> {
        let mut inner = lock(&self.inner);
        inner.holds.insert(monitor.to_owned(), now + WAKE_HOLD);
        let Some(d) = inner.off.get(monitor).copied() else {
            return Ok(());
        };
        self.power
            .set_enabled(d.id, true)
            .map_err(|e| format!("turn the display of {monitor} back on: {e}"))?;
        tracing::info!(monitor, "display back on before the switch to this machine");
        inner.off.remove(monitor);
        inner.unknown_since.remove(monitor);
        self.save(&inner);
        Ok(())
    }

    /// Follows what each monitor shows (`shows`, from the owner's real input) with this machine's
    /// displays: off while the monitor shows the other machine, on otherwise. `keys` is the EDID
    /// identity of each monitor this machine sees.
    pub fn reconcile(
        &self,
        me: Machine,
        shows: &BTreeMap<String, Machine>,
        keys: &BTreeMap<String, DisplayKey>,
        now: Instant,
    ) {
        let mut inner = lock(&self.inner);
        inner
            .holds
            .retain(|m, until| now < *until && shows.get(m) != Some(&me));
        self.turn_on_shown(&mut inner, me, shows, now);
        if inner.stopped || self.power.control() != DisplayControl::Available {
            return;
        }
        for (monitor, key) in keys {
            let away = shows.get(monitor) == Some(&me.other());
            if !away || inner.off.contains_key(monitor) || inner.holds.contains_key(monitor) {
                inner.skipped.remove(monitor);
                continue;
            }
            match self.turn_off(&mut inner, monitor, *key) {
                Ok(()) => {
                    inner.skipped.remove(monitor);
                }
                Err(why) => {
                    if inner.skipped.get(monitor) != Some(&why) {
                        tracing::info!(monitor, "its display stays on: {why}");
                        inner.skipped.insert(monitor.clone(), why);
                    }
                }
            }
        }
    }

    /// The displays off whose monitor shows this machine again, or whose input has not been known
    /// for [`UNKNOWN_GRACE`], go back on.
    fn turn_on_shown(
        &self,
        inner: &mut Inner,
        me: Machine,
        shows: &BTreeMap<String, Machine>,
        now: Instant,
    ) {
        let mut changed = false;
        for (monitor, d) in inner.off.clone() {
            let back = match shows.get(&monitor) {
                Some(m) if *m == me.other() => {
                    inner.unknown_since.remove(&monitor);
                    false
                }
                Some(_) => true,
                None => {
                    let since = *inner.unknown_since.entry(monitor.clone()).or_insert(now);
                    now.saturating_duration_since(since) >= UNKNOWN_GRACE
                }
            };
            if !back {
                continue;
            }
            match self.power.set_enabled(d.id, true) {
                Ok(()) => {
                    tracing::info!(monitor, "display back on");
                    inner.off.remove(&monitor);
                    inner.unknown_since.remove(&monitor);
                    changed = true;
                }
                Err(_) if self.already_on(d) => {
                    inner.off.remove(&monitor);
                    inner.unknown_since.remove(&monitor);
                    changed = true;
                }
                Err(e) => tracing::warn!("could not turn the display of {monitor} back on: {e}"),
            }
        }
        if changed {
            self.save(inner);
        }
    }

    /// The system lists a screen with the identity of `d` again: it is on (the id it had may
    /// not exist after a restart of the session).
    fn already_on(&self, d: OffDisplay) -> bool {
        let key = DisplayKey {
            vendor: d.vendor,
            product: d.product,
            serial: d.serial,
        };
        self.power
            .active()
            .is_ok_and(|a| a.iter().any(|s| s.key == Some(key)))
    }

    fn turn_off(&self, inner: &mut Inner, monitor: &str, key: DisplayKey) -> Result<(), String> {
        let active = self.power.active().map_err(|e| e.to_string())?;
        let mut found = active
            .iter()
            .enumerate()
            .filter(|(_, d)| d.key == Some(key));
        let (index, display) = match (found.next(), found.next()) {
            (Some(one), None) => one,
            (None, _) => return Err("the system does not list a screen with its EDID".into()),
            (Some(_), Some(_)) => return Err("two screens have its EDID".into()),
        };
        if active.len() < 2 {
            return Err("it is the only screen".into());
        }
        if index + 1 != active.len() {
            return Err(
                "it is not the last screen: turning it off would renumber the others (make another screen the main one)"
                    .into(),
            );
        }
        let entry = OffDisplay {
            id: display.id,
            vendor: key.vendor,
            product: key.product,
            serial: key.serial,
        };
        // Saved first: a crash right after the change still turns it back on at start-up.
        inner.off.insert(monitor.to_owned(), entry);
        self.save(inner);
        if let Err(e) = self.power.set_enabled(display.id, false) {
            inner.off.remove(monitor);
            self.save(inner);
            return Err(format!("could not turn it off: {e}"));
        }
        tracing::info!(monitor, "display off: the monitor shows the other machine");
        Ok(())
    }

    fn save(&self, inner: &Inner) {
        let Some(file) = &self.file else { return };
        if let Err(e) = save(file, &inner.off) {
            tracing::warn!("could not save the displays turned off: {e}");
        }
    }
}

/// The agent's guard, for an exit that does not go back through the agent: on macOS AppKit ends
/// the process itself at the end of the session (T-333, T-334).
static AT_PROCESS_EXIT: Mutex<Option<Weak<DisplayGuard>>> = Mutex::new(None);

impl DisplayGuard {
    /// Makes this guard the one [`restore_at_process_exit`] turns back on.
    pub fn register_for_process_exit(self: &Arc<Self>) {
        *AT_PROCESS_EXIT
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::downgrade(self));
    }
}

/// Right before the process ends outside the agent's shutdown: the displays this machine turned
/// off go back on. It runs on AppKit's main thread, so like a panic it never waits for the
/// guard's lock ([`DisplayGuard::restore_on_panic`]).
pub fn restore_at_process_exit() {
    let guard = AT_PROCESS_EXIT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(Weak::upgrade);
    if let Some(guard) = guard {
        guard.restore_on_panic();
    }
}

fn load(file: &Path) -> BTreeMap<String, OffDisplay> {
    match std::fs::read(file) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            tracing::warn!("{} is unreadable, ignored: {e}", file.display());
            BTreeMap::new()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => {
            tracing::warn!("could not read {}: {e}", file.display());
            BTreeMap::new()
        }
    }
}

/// Empty, the file goes away; otherwise it is replaced in one step (temporary file + rename).
fn save(file: &Path, off: &BTreeMap<String, OffDisplay>) -> std::io::Result<()> {
    if off.is_empty() {
        return match std::fs::remove_file(file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(off).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, file)
}

#[cfg(test)]
mod tests {
    use tandem_input::displays::{FakeDisplayPower, PowerDisplay};

    use super::*;

    const MONITOR: &str = "lc34g55t-0a1b2c3d";
    const BUILTIN: u32 = 1;
    const HDMI: u32 = 7;

    fn key() -> DisplayKey {
        DisplayKey {
            vendor: 0x4C2D,
            product: 0x7105,
            serial: 9,
        }
    }

    fn mac() -> Arc<FakeDisplayPower> {
        Arc::new(FakeDisplayPower::new([
            PowerDisplay {
                id: BUILTIN,
                key: None,
            },
            PowerDisplay {
                id: HDMI,
                key: Some(key()),
            },
        ]))
    }

    fn keys() -> BTreeMap<String, DisplayKey> {
        BTreeMap::from([(MONITOR.to_owned(), key())])
    }

    fn shows(machine: Machine) -> BTreeMap<String, Machine> {
        BTreeMap::from([(MONITOR.to_owned(), machine)])
    }

    // The Mac is B; the PC is A.
    const ME: Machine = Machine::B;

    #[test]
    fn off_while_the_monitor_shows_the_other_and_on_when_it_comes_back() {
        let fake = mac();
        let g = DisplayGuard::new(fake.clone(), None);
        let t = Instant::now();
        g.reconcile(ME, &shows(ME), &keys(), t);
        assert!(fake.calls().is_empty(), "it shows the Mac: nothing to do");
        g.reconcile(ME, &shows(Machine::A), &keys(), t);
        assert!(!fake.is_on(HDMI));
        assert_eq!(g.off(), [MONITOR]);
        g.reconcile(ME, &shows(Machine::A), &keys(), t);
        assert_eq!(fake.calls(), [(HDMI, false)], "once");
        g.reconcile(ME, &shows(ME), &keys(), t);
        assert!(fake.is_on(HDMI));
        assert!(g.off().is_empty());
    }

    /// The switch back to the Mac: the display is on before the switch is done, and the next turns
    /// do not turn it off again while the monitor still shows the PC.
    #[test]
    fn woken_before_the_switch_back_and_kept_on_while_it_lands() {
        let fake = mac();
        let g = DisplayGuard::new(fake.clone(), None);
        let t = Instant::now();
        g.reconcile(ME, &shows(Machine::A), &keys(), t);
        g.wake(MONITOR, t).unwrap();
        assert!(fake.is_on(HDMI));
        g.reconcile(ME, &shows(Machine::A), &keys(), t + Duration::from_secs(5));
        assert!(fake.is_on(HDMI), "the switch has not landed yet");
        g.reconcile(ME, &shows(ME), &keys(), t + Duration::from_secs(6));
        // The switch failed and the monitor stayed on the PC: it goes off after the hold.
        g.reconcile(ME, &shows(Machine::A), &keys(), t + Duration::from_secs(7));
        assert!(!fake.is_on(HDMI), "the hold ended when it showed the Mac");
        g.wake(MONITOR, t).unwrap();
        g.reconcile(ME, &shows(Machine::A), &keys(), t + WAKE_HOLD);
        assert!(!fake.is_on(HDMI), "after the hold it goes off again");
    }

    #[test]
    fn waking_a_display_that_is_on_changes_nothing() {
        let fake = mac();
        let g = DisplayGuard::new(fake.clone(), None);
        g.wake(MONITOR, Instant::now()).unwrap();
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn exit_turns_everything_back_on() {
        let fake = mac();
        let g = DisplayGuard::new(fake.clone(), None);
        g.reconcile(ME, &shows(Machine::A), &keys(), Instant::now());
        g.restore_all();
        assert!(fake.is_on(HDMI));
        assert!(g.off().is_empty());
    }

    /// A crash with the display off: the file says so, and the next start turns it on.
    #[test]
    fn after_the_exit_nothing_goes_off_again() {
        let fake = mac();
        let g = DisplayGuard::new(fake.clone(), None);
        let t = Instant::now();
        g.reconcile(ME, &shows(Machine::A), &keys(), t);
        g.shut_down();
        g.reconcile(ME, &shows(Machine::A), &keys(), t);
        assert!(fake.is_on(HDMI));
        assert_eq!(fake.calls(), [(HDMI, false), (HDMI, true)]);
    }

    /// AppKit's exit at the end of the session (T-333) skips the agent's shutdown: the registered
    /// guard still turns its displays back on.
    #[test]
    fn the_process_exit_outside_the_agent_turns_the_displays_on() {
        let fake = mac();
        let g = Arc::new(DisplayGuard::new(fake.clone(), None));
        g.register_for_process_exit();
        g.reconcile(ME, &shows(Machine::A), &keys(), Instant::now());
        assert!(!fake.is_on(HDMI));
        restore_at_process_exit();
        assert!(fake.is_on(HDMI));
        assert!(g.off().is_empty());
        drop(g);
        restore_at_process_exit();
    }

    #[test]
    fn start_up_after_a_crash_turns_the_saved_displays_on() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("displays-off.json");
        let fake = mac();
        let crashed = DisplayGuard::new(fake.clone(), Some(file.clone()));
        crashed.reconcile(ME, &shows(Machine::A), &keys(), Instant::now());
        assert!(file.exists());
        drop(crashed);
        assert!(!fake.is_on(HDMI));
        let next = DisplayGuard::new(fake.clone(), Some(file.clone()));
        next.restore_saved();
        assert!(fake.is_on(HDMI));
        assert!(!file.exists(), "nothing left off");
    }

    #[test]
    fn the_file_is_written_before_the_display_goes_off() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("displays-off.json");
        let fake = mac();
        fake.set_failing(true);
        let g = DisplayGuard::new(fake.clone(), Some(file.clone()));
        g.reconcile(ME, &shows(Machine::A), &keys(), Instant::now());
        assert!(g.off().is_empty(), "a failed change is not recorded");
        assert!(!file.exists());
    }

    #[test]
    fn a_display_that_fails_to_come_back_is_tried_again() {
        let fake = mac();
        let g = DisplayGuard::new(fake.clone(), None);
        g.reconcile(ME, &shows(Machine::A), &keys(), Instant::now());
        fake.set_failing(true);
        g.restore_all();
        assert_eq!(g.off(), [MONITOR]);
        fake.set_failing(false);
        g.restore_all();
        assert!(fake.is_on(HDMI));
    }

    #[test]
    fn an_unknown_input_turns_it_back_on_after_the_grace() {
        let fake = mac();
        let g = DisplayGuard::new(fake.clone(), None);
        let t = Instant::now();
        g.reconcile(ME, &shows(Machine::A), &keys(), t);
        let none = BTreeMap::new();
        g.reconcile(ME, &none, &keys(), t + Duration::from_secs(1));
        g.reconcile(ME, &none, &keys(), t + Duration::from_secs(10));
        assert!(!fake.is_on(HDMI), "a short gap keeps it off");
        g.reconcile(ME, &shows(Machine::A), &keys(), t + Duration::from_secs(12));
        g.reconcile(ME, &none, &keys(), t + Duration::from_secs(20));
        assert!(!fake.is_on(HDMI), "the gap counts again from zero");
        g.reconcile(
            ME,
            &none,
            &BTreeMap::new(),
            t + Duration::from_secs(20) + UNKNOWN_GRACE,
        );
        assert!(fake.is_on(HDMI));
    }

    #[test]
    fn never_the_only_screen_nor_one_that_would_renumber_the_others() {
        let only = Arc::new(FakeDisplayPower::new([PowerDisplay {
            id: HDMI,
            key: Some(key()),
        }]));
        let g = DisplayGuard::new(only.clone(), None);
        g.reconcile(ME, &shows(Machine::A), &keys(), Instant::now());
        assert!(
            only.calls().is_empty(),
            "the lid is closed: it is the only screen"
        );

        let main = Arc::new(FakeDisplayPower::new([
            PowerDisplay {
                id: HDMI,
                key: Some(key()),
            },
            PowerDisplay {
                id: BUILTIN,
                key: None,
            },
        ]));
        let g = DisplayGuard::new(main.clone(), None);
        g.reconcile(ME, &shows(Machine::A), &keys(), Instant::now());
        assert!(
            main.calls().is_empty(),
            "it is screen-1: the built-in would become it"
        );
    }

    #[test]
    fn without_the_call_it_only_reports() {
        let fake = Arc::new(FakeDisplayPower::unavailable("no symbol"));
        let g = DisplayGuard::new(fake.clone(), None);
        assert!(matches!(g.control(), DisplayControl::Unavailable(_)));
        g.reconcile(ME, &shows(Machine::A), &keys(), Instant::now());
        assert!(g.off().is_empty());
        g.wake(MONITOR, Instant::now()).unwrap();
        g.restore_all();
    }

    /// The id saved belongs to a display that is already back (the session restarted).
    #[test]
    fn a_saved_display_that_is_already_on_is_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("displays-off.json");
        let fake = mac();
        let g = DisplayGuard::new(fake.clone(), Some(file.clone()));
        g.reconcile(ME, &shows(Machine::A), &keys(), Instant::now());
        fake.force(HDMI, true);
        fake.set_failing(true);
        let next = DisplayGuard::new(fake.clone(), Some(file.clone()));
        next.restore_saved();
        assert!(next.off().is_empty());
        assert!(!file.exists());
    }

    #[test]
    fn an_unreadable_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("displays-off.json");
        std::fs::write(&file, b"not json").unwrap();
        let fake = mac();
        DisplayGuard::new(fake.clone(), Some(file)).restore_saved();
        assert!(fake.calls().is_empty());
    }
}
