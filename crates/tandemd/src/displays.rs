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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tandem_core::{BuiltinMode, DisplayKey, Machine};
use tandem_input::displays::{BuiltinState, DisplayControl, DisplayPower, SystemDisplay};
use tandem_input::rescue::{WindowMover, WindowRescue};

/// How long a monitor's input can be unknown (the other agent left, the owner changes) before its
/// display is turned back on: shorter would turn it on and off during a normal owner handover.
pub const UNKNOWN_GRACE: Duration = Duration::from_secs(15);

/// How long a display woken for a switch stays on while the monitor does not show this machine
/// yet: the switch's maximum time with room. If the switch failed, it goes off again after this.
pub const WAKE_HOLD: Duration = Duration::from_secs(30);

/// The first wait before trying again to turn a display back on after a failure; it doubles up to
/// [`RETRY_MAX`]. macOS refuses (`CGCompleteDisplayConfiguration` 1001) while the display is not
/// there to take the change, for example while the monitor's input is on the other machine or the
/// id it had is gone, so retrying every turn only fills the log.
pub const RETRY_FIRST: Duration = Duration::from_secs(1);
pub const RETRY_MAX: Duration = Duration::from_secs(60);

/// When the next try of a display that failed to come back is due, and how many failed.
#[derive(Debug, Clone, Copy)]
struct Retry {
    at: Instant,
    wait: Duration,
    failures: u32,
}

impl Retry {
    fn first(now: Instant) -> Self {
        Retry {
            at: now + RETRY_FIRST,
            wait: RETRY_FIRST,
            failures: 1,
        }
    }

    fn next(self, now: Instant) -> Self {
        let wait = (self.wait * 2).min(RETRY_MAX);
        Retry {
            at: now + wait,
            wait,
            failures: self.failures + 1,
        }
    }
}

/// What the built-in display was before this machine changed it for its mode (T-353), to put it
/// back at exit and after a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct SavedBuiltin {
    id: u32,
    /// It was on (a screen of its own or a mirror).
    on: bool,
    /// The display it mirrored.
    mirror_of: Option<u32>,
}

/// A display this machine turned off: the monitor it is, its system id and its EDID identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct OffDisplay {
    id: u32,
    vendor: u16,
    product: u16,
    serial: u32,
}

impl OffDisplay {
    fn key(self) -> DisplayKey {
        DisplayKey {
            vendor: self.vendor,
            product: self.product,
            serial: self.serial,
        }
    }
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
    /// Displays that failed to come back: when to try again (T-353).
    retry: BTreeMap<String, Retry>,
    /// The built-in display as it was before its mode changed it (T-353).
    builtin: Option<SavedBuiltin>,
    /// The last state the built-in display was set to, and the next try after a failure.
    builtin_set: Option<BuiltinState>,
    builtin_retry: Option<Retry>,
    /// The agent is exiting: nothing goes off any more.
    stopped: bool,
    /// When each screen was last known to show the other machine, for the window rescue: an
    /// input that is briefly unknown (a DDC read that failed) does not bring the windows back.
    hidden_at: BTreeMap<DisplayKey, Instant>,
}

/// This machine's display control in the agent.
#[derive(Debug)]
pub struct DisplayGuard {
    power: Arc<dyn DisplayPower>,
    /// Where the displays turned off are saved; `None` keeps them in memory (tests).
    file: Option<PathBuf>,
    inner: Mutex<Inner>,
    /// Moves the windows off a screen nobody sees where the display cannot be turned off
    /// (Windows, T-360).
    rescue: Option<WindowRescue>,
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
            rescue: None,
        }
    }

    /// With the window rescue of this system, if it has one.
    #[must_use]
    pub fn with_rescue(mut self, mover: Option<Arc<dyn WindowMover>>) -> Self {
        self.rescue = mover.map(WindowRescue::new);
        self
    }

    /// Moves the windows off the screens of the monitors in `hidden` (those that show the other
    /// machine) and puts them back when they are seen again (`shown`, they show this one). A
    /// screen in neither, whose input is not known, stays hidden for [`UNKNOWN_GRACE`].
    pub fn rescue_windows(
        &self,
        hidden: &BTreeSet<DisplayKey>,
        shown: &BTreeSet<DisplayKey>,
        now: Instant,
    ) {
        let Some(r) = &self.rescue else { return };
        let effective = {
            let mut inner = lock(&self.inner);
            for k in hidden {
                inner.hidden_at.insert(*k, now);
            }
            inner.hidden_at.retain(|k, at| {
                !shown.contains(k) && now.saturating_duration_since(*at) < UNKNOWN_GRACE
            });
            inner.hidden_at.keys().copied().collect::<BTreeSet<_>>()
        };
        r.reconcile(&effective);
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
        if let Some(b) = self.builtin_file().as_deref().and_then(load_builtin) {
            tracing::info!("putting the built-in display back as a previous run found it");
            lock(&self.inner).builtin = Some(b);
            self.restore_builtin();
        }
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
            if let Err(e) = self.enable(d) {
                tracing::warn!("could not turn the display of {monitor} back on: {e}");
                inner.off.insert(monitor, d);
            }
        }
        inner.holds.clear();
        inner.unknown_since.clear();
        inner.retry.clear();
        self.save(&inner);
        drop(inner);
        self.restore_builtin();
    }

    /// Turns `d` back on: by the id it had or, if macOS refuses it, by the id the system now gives
    /// a display with its EDID identity. `Ok` also when it is already on.
    fn enable(&self, d: OffDisplay) -> Result<(), String> {
        let first = match self.power.set_enabled(d.id, true) {
            Ok(()) => return Ok(()),
            Err(e) => e.to_string(),
        };
        if self.already_on(d) {
            return Ok(());
        }
        let key = d.key();
        let online = self.power.online().unwrap_or_default();
        match online.iter().find(|s| s.key == Some(key)) {
            Some(s) if s.id != d.id => self
                .power
                .set_enabled(s.id, true)
                .map_err(|e| format!("{first}; with its new id {}: {e}", s.id)),
            Some(_) => Err(first),
            None if online.is_empty() => Err(first),
            None => Err(format!("{first} (the system does not list it now)")),
        }
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
                if let Some(b) = self.builtin_file().as_deref().and_then(load_builtin)
                    && b.on
                {
                    let _ = self.power.set_builtin(b.id, BuiltinState::Own);
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
                inner.retry.remove(&monitor);
                continue;
            }
            let retry = inner.retry.get(&monitor).copied();
            if retry.is_some_and(|r| now < r.at) {
                continue;
            }
            match self.enable(d) {
                Ok(()) => {
                    match retry {
                        Some(r) => tracing::info!(
                            monitor,
                            failures = r.failures,
                            "display back on after failing"
                        ),
                        None => tracing::info!(monitor, "display back on"),
                    }
                    inner.off.remove(&monitor);
                    inner.unknown_since.remove(&monitor);
                    inner.retry.remove(&monitor);
                    changed = true;
                }
                Err(e) => {
                    // Reported once; then tried again less and less often, without a word.
                    let next = match retry {
                        None => {
                            tracing::warn!(
                                "could not turn the display of {monitor} back on, trying again: {e}"
                            );
                            Retry::first(now)
                        }
                        Some(r) => {
                            tracing::debug!(monitor, failures = r.failures + 1, "still off: {e}");
                            r.next(now)
                        }
                    };
                    inner.retry.insert(monitor, next);
                }
            }
        }
        if changed {
            self.save(inner);
        }
    }

    /// The system lists a screen with the identity of `d` again: it is on (the id it had may
    /// not exist after a restart of the session).
    fn already_on(&self, d: OffDisplay) -> bool {
        let key = d.key();
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

    /// The built-in display follows `mode` (T-353): off, mirroring the main external display or
    /// on its own while an external display is active; on and on its own when none is. In
    /// `Extended` mode only what this machine changed is undone: macOS's own settings stay.
    /// With the lid closed it is never touched.
    pub fn reconcile_builtin(&self, mode: BuiltinMode, now: Instant) {
        let mut inner = lock(&self.inner);
        if inner.stopped
            || self.power.control() != DisplayControl::Available
            || self.power.lid_closed()
        {
            return;
        }
        let Ok(online) = self.power.online() else {
            return;
        };
        let Some(builtin) = online.iter().find(|d| d.builtin).copied() else {
            return;
        };
        let Some(want) = builtin_target(mode, &builtin, &online, inner.builtin) else {
            inner.builtin_retry = None;
            return;
        };
        if current_state(&builtin) == want {
            inner.builtin_set = Some(want);
            inner.builtin_retry = None;
            if inner.builtin.is_some_and(|b| restored(b, &builtin)) {
                inner.builtin = None;
                self.save_builtin(&inner);
            }
            return;
        }
        if inner.builtin_retry.is_some_and(|r| now < r.at) {
            return;
        }
        if inner.builtin.is_none() {
            // Saved first: a crash right after the change still puts it back at start-up.
            inner.builtin = Some(SavedBuiltin {
                id: builtin.id,
                on: builtin.enabled,
                mirror_of: builtin.mirror_of,
            });
            self.save_builtin(&inner);
        }
        match self.power.set_builtin(builtin.id, want) {
            Ok(()) => {
                if inner.builtin_set != Some(want) {
                    tracing::info!(mode = mode.as_str(), state = ?want, "built-in display set");
                }
                inner.builtin_set = Some(want);
                inner.builtin_retry = None;
                let back = SystemDisplay {
                    enabled: want != BuiltinState::Off,
                    mirror_of: match want {
                        BuiltinState::Mirror(of) => Some(of),
                        _ => None,
                    },
                    ..builtin
                };
                if inner.builtin.is_some_and(|b| restored(b, &back)) {
                    inner.builtin = None;
                    self.save_builtin(&inner);
                }
            }
            Err(e) => {
                inner.builtin_retry = Some(match inner.builtin_retry {
                    None => {
                        tracing::warn!(
                            "could not set the built-in display ({want:?}), trying again: {e}"
                        );
                        Retry::first(now)
                    }
                    Some(r) => r.next(now),
                });
            }
        }
    }

    /// At exit and at start-up: the built-in display goes back to how this machine found it. One
    /// that was off is left as it is: turning it off again could leave no display at all.
    fn restore_builtin(&self) {
        let mut inner = lock(&self.inner);
        let Some(saved) = inner.builtin else { return };
        let online = self.power.online().unwrap_or_default();
        let now = online.iter().find(|d| d.builtin).copied();
        let id = now.map_or(saved.id, |d| d.id);
        if saved.on && now.is_none_or(|d| !restored(saved, &d)) {
            let state = match saved.mirror_of {
                Some(of) if online.iter().any(|d| d.id == of && d.enabled) => {
                    BuiltinState::Mirror(of)
                }
                _ => BuiltinState::Own,
            };
            if let Err(e) = self.power.set_builtin(id, state) {
                tracing::warn!("could not put the built-in display back: {e}");
                return;
            }
        }
        inner.builtin = None;
        inner.builtin_set = None;
        inner.builtin_retry = None;
        self.save_builtin(&inner);
    }

    fn builtin_file(&self) -> Option<PathBuf> {
        self.file
            .as_ref()
            .map(|f| f.with_file_name("builtin-display.json"))
    }

    fn save_builtin(&self, inner: &Inner) {
        let Some(file) = self.builtin_file() else {
            return;
        };
        let result = match inner.builtin {
            Some(b) => serde_json::to_vec_pretty(&b)
                .map_err(std::io::Error::other)
                .and_then(|bytes| write_atomic(&file, &bytes)),
            None => match std::fs::remove_file(&file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
        };
        if let Err(e) = result {
            tracing::warn!("could not save the built-in display state: {e}");
        }
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

fn current_state(d: &SystemDisplay) -> BuiltinState {
    match (d.enabled, d.mirror_of) {
        (false, _) => BuiltinState::Off,
        (true, Some(of)) => BuiltinState::Mirror(of),
        (true, None) => BuiltinState::Own,
    }
}

/// The built-in display is as it was saved.
fn restored(saved: SavedBuiltin, now: &SystemDisplay) -> bool {
    now.enabled == saved.on && (!saved.on || now.mirror_of == saved.mirror_of)
}

/// What the built-in display should be, or `None` to leave it as it is. Without an active external
/// display it is always on: the user is never left without a screen.
fn builtin_target(
    mode: BuiltinMode,
    builtin: &SystemDisplay,
    online: &[SystemDisplay],
    saved: Option<SavedBuiltin>,
) -> Option<BuiltinState> {
    let externals: Vec<&SystemDisplay> = online
        .iter()
        .filter(|d| !d.builtin && d.enabled && d.mirror_of.is_none())
        .collect();
    let Some(first) = externals.first() else {
        return Some(match current_state(builtin) {
            // A mirror of a display that is gone: on its own.
            BuiltinState::Mirror(of) if !online.iter().any(|d| d.id == of && d.enabled) => {
                BuiltinState::Own
            }
            BuiltinState::Off => BuiltinState::Own,
            s => s,
        });
    };
    match mode {
        BuiltinMode::ExternalOnly => Some(BuiltinState::Off),
        BuiltinMode::Mirror => {
            let main = externals.iter().find(|d| d.main).unwrap_or(first);
            Some(BuiltinState::Mirror(main.id))
        }
        BuiltinMode::Extended => saved.map(|s| match (s.on, s.mirror_of) {
            (true, Some(of)) if externals.iter().any(|d| d.id == of) => BuiltinState::Mirror(of),
            _ => BuiltinState::Own,
        }),
    }
}

fn load_builtin(file: &Path) -> Option<SavedBuiltin> {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!("could not read {}: {e}", file.display());
            return None;
        }
    };
    serde_json::from_slice(&bytes)
        .map_err(|e| tracing::warn!("{} is unreadable, ignored: {e}", file.display()))
        .ok()
}

fn write_atomic(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, file)
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
    let bytes = serde_json::to_vec_pretty(off).map_err(std::io::Error::other)?;
    write_atomic(file, &bytes)
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

    #[test]
    fn the_guard_rescues_windows_only_with_a_mover() {
        use tandem_input::geometry::Rect;
        use tandem_input::rescue::{
            FakeWindowMover, RescueScreen, ShowState, WindowInfo, WindowMover,
        };
        let key = DisplayKey {
            vendor: 9,
            product: 9,
            serial: 9,
        };
        let r = Rect::new(-1000, 0, 500, 500);
        let fake = Arc::new(FakeWindowMover::new(
            vec![
                RescueScreen {
                    rect: Rect::new(-2560, 0, 2560, 1440),
                    work: Rect::new(-2560, 0, 2560, 1400),
                    key: Some(key),
                    primary: false,
                },
                RescueScreen {
                    rect: Rect::new(0, 0, 1920, 1080),
                    work: Rect::new(0, 0, 1920, 1040),
                    key: None,
                    primary: true,
                },
            ],
            vec![WindowInfo {
                id: 1,
                rect: r,
                normal: r,
                state: ShowState::Normal,
            }],
        ));
        let mover: Arc<dyn WindowMover> = fake.clone();
        let g = DisplayGuard::new(Arc::new(FakeDisplayPower::unavailable("x")), None)
            .with_rescue(Some(mover));
        let t = Instant::now();
        let none = BTreeSet::new();
        g.rescue_windows(&BTreeSet::from([key]), &none, t);
        assert!(fake.window(1).unwrap().rect.x >= 0);
        // Its input unknown for less than the grace: the windows stay where they were moved.
        g.rescue_windows(&none, &none, t + UNKNOWN_GRACE - Duration::from_secs(1));
        assert!(fake.window(1).unwrap().rect.x >= 0);
        // Unknown for longer: they go back.
        g.rescue_windows(&none, &none, t + UNKNOWN_GRACE);
        assert_eq!(fake.window(1).unwrap().rect.x, -1000);
        // Shown again on this machine: back at once.
        g.rescue_windows(&BTreeSet::from([key]), &none, t + UNKNOWN_GRACE * 2);
        assert!(fake.window(1).unwrap().rect.x >= 0);
        g.rescue_windows(&none, &BTreeSet::from([key]), t + UNKNOWN_GRACE * 2);
        assert_eq!(fake.window(1).unwrap().rect.x, -1000);
        // Without a mover (macOS) it does nothing.
        DisplayGuard::new(Arc::new(FakeDisplayPower::unavailable("x")), None).rescue_windows(
            &BTreeSet::from([key]),
            &none,
            t,
        );
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

    /// The T-341 log: macOS refused to turn the display back on every second for minutes while the
    /// monitor was on the other machine. It is reported once and tried less and less often.
    #[test]
    fn a_refused_display_is_tried_again_with_a_growing_wait() {
        let fake = mac();
        let g = DisplayGuard::new(fake.clone(), None);
        let t = Instant::now();
        g.reconcile(ME, &shows(Machine::A), &keys(), t);
        fake.unplug(HDMI, true);
        let tries = || fake.calls().iter().filter(|c| **c == (HDMI, true)).count();
        let mut at = t;
        for _ in 0..40 {
            at += Duration::from_secs(1);
            g.reconcile(ME, &shows(ME), &keys(), at);
        }
        assert_eq!(g.off(), [MONITOR], "still off: the display is not there");
        assert_eq!(tries(), 0, "a refused change is not recorded by the double");
        let retry = lock(&g.inner).retry.get(MONITOR).copied().unwrap();
        assert!(retry.failures <= 6, "{} tries in 40 s", retry.failures);
        assert!(retry.wait >= Duration::from_secs(16));
        fake.unplug(HDMI, false);
        g.reconcile(ME, &shows(ME), &keys(), retry.at);
        assert!(fake.is_on(HDMI));
        assert!(g.off().is_empty());
        assert!(lock(&g.inner).retry.is_empty());
    }

    #[test]
    fn the_wait_is_capped() {
        let t = Instant::now();
        let mut r = Retry::first(t);
        for _ in 0..20 {
            r = r.next(t);
        }
        assert_eq!(r.wait, RETRY_MAX);
    }

    /// The display came back with another id (a new session, a reconnection): it is found by its
    /// EDID identity and turned on with the new one.
    #[test]
    fn a_display_with_a_new_id_is_turned_on_by_its_edid() {
        let fake = mac();
        let g = DisplayGuard::new(fake.clone(), None);
        let t = Instant::now();
        g.reconcile(ME, &shows(Machine::A), &keys(), t);
        fake.renumber(HDMI, 9);
        g.reconcile(ME, &shows(ME), &keys(), t);
        assert!(fake.is_on(9));
        assert!(g.off().is_empty());
    }

    const PANEL: u32 = 1;

    fn panel_key() -> DisplayKey {
        DisplayKey {
            vendor: 0x0610,
            product: 0xA061,
            serial: 1,
        }
    }

    /// The Mac with its panel, the G5 (main) and the 34".
    fn macbook() -> Arc<FakeDisplayPower> {
        Arc::new(
            FakeDisplayPower::new([
                PowerDisplay {
                    id: 3,
                    key: Some(DisplayKey {
                        vendor: 0x4C2D,
                        product: 0x748A,
                        serial: 5,
                    }),
                },
                PowerDisplay {
                    id: PANEL,
                    key: Some(panel_key()),
                },
                PowerDisplay {
                    id: HDMI,
                    key: Some(key()),
                },
            ])
            .with_builtin(PANEL),
        )
    }

    #[test]
    fn external_only_turns_the_panel_off_while_an_external_display_is_on() {
        let fake = macbook();
        let g = DisplayGuard::new(fake.clone(), None);
        let t = Instant::now();
        g.reconcile_builtin(BuiltinMode::ExternalOnly, t);
        assert!(!fake.is_on(PANEL));
        g.reconcile_builtin(BuiltinMode::ExternalOnly, t);
        assert_eq!(fake.builtin_calls(), [(PANEL, BuiltinState::Off)], "once");
        // Both external displays unplugged: the panel comes back, never no screen at all.
        fake.unplug(3, true);
        fake.unplug(HDMI, true);
        g.reconcile_builtin(BuiltinMode::ExternalOnly, t);
        assert!(fake.is_on(PANEL));
        assert_eq!(fake.active().unwrap().len(), 1);
    }

    #[test]
    fn mirror_mirrors_the_main_external_display() {
        let fake = macbook();
        let g = DisplayGuard::new(fake.clone(), None);
        g.reconcile_builtin(BuiltinMode::Mirror, Instant::now());
        assert_eq!(fake.mirror_of(PANEL), Some(3));
        assert!(fake.is_on(PANEL));
        // The main display leaves: it mirrors the other one.
        fake.unplug(3, true);
        g.reconcile_builtin(BuiltinMode::Mirror, Instant::now());
        assert_eq!(fake.mirror_of(PANEL), Some(HDMI));
    }

    /// Extended undoes what Tandem did and leaves alone what macOS (or the user) set.
    #[test]
    fn extended_only_undoes_what_tandem_changed() {
        let fake = macbook();
        let g = DisplayGuard::new(fake.clone(), None);
        let t = Instant::now();
        g.reconcile_builtin(BuiltinMode::Extended, t);
        assert!(fake.builtin_calls().is_empty());
        g.reconcile_builtin(BuiltinMode::ExternalOnly, t);
        g.reconcile_builtin(BuiltinMode::Extended, t);
        assert!(fake.is_on(PANEL));
        assert_eq!(fake.mirror_of(PANEL), None);
        assert!(lock(&g.inner).builtin.is_none(), "nothing left to put back");
    }

    #[test]
    fn with_the_lid_closed_the_panel_is_never_touched() {
        let fake = macbook();
        fake.set_lid_closed(true);
        let g = DisplayGuard::new(fake.clone(), None);
        g.reconcile_builtin(BuiltinMode::Mirror, Instant::now());
        g.reconcile_builtin(BuiltinMode::ExternalOnly, Instant::now());
        assert!(fake.builtin_calls().is_empty());
    }

    #[test]
    fn the_panel_is_put_back_at_exit_and_after_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("displays-off.json");
        let fake = macbook();
        let g = DisplayGuard::new(fake.clone(), Some(file.clone()));
        g.reconcile_builtin(BuiltinMode::ExternalOnly, Instant::now());
        assert!(dir.path().join("builtin-display.json").exists());
        g.shut_down();
        assert!(fake.is_on(PANEL));
        assert!(!dir.path().join("builtin-display.json").exists());
        g.reconcile_builtin(BuiltinMode::ExternalOnly, Instant::now());
        assert!(fake.is_on(PANEL), "nothing changes after the exit");

        let crashed = DisplayGuard::new(fake.clone(), Some(file.clone()));
        crashed.reconcile_builtin(BuiltinMode::Mirror, Instant::now());
        drop(crashed);
        let next = DisplayGuard::new(fake.clone(), Some(file));
        next.restore_saved();
        assert_eq!(
            fake.mirror_of(PANEL),
            None,
            "back as it was found: on its own"
        );
        assert!(fake.is_on(PANEL));
    }

    #[test]
    fn a_failed_panel_change_waits_before_trying_again() {
        let fake = macbook();
        let g = DisplayGuard::new(fake.clone(), None);
        let t = Instant::now();
        fake.fail_next(1);
        g.reconcile_builtin(BuiltinMode::ExternalOnly, t);
        assert!(fake.is_on(PANEL));
        g.reconcile_builtin(BuiltinMode::ExternalOnly, t + Duration::from_millis(500));
        assert!(fake.is_on(PANEL), "too soon");
        g.reconcile_builtin(BuiltinMode::ExternalOnly, t + RETRY_FIRST);
        assert!(!fake.is_on(PANEL));
    }

    /// The panel off and T-341 together: the agent never leaves the Mac without a screen.
    #[test]
    fn external_only_and_the_phantom_display_rule_leave_a_screen() {
        let fake = macbook();
        let g = DisplayGuard::new(fake.clone(), None);
        let t = Instant::now();
        let g5 = DisplayKey {
            vendor: 0x4C2D,
            product: 0x748A,
            serial: 5,
        };
        let keys = BTreeMap::from([(MONITOR.to_owned(), key()), ("odyssey-g5".to_owned(), g5)]);
        let away = BTreeMap::from([
            (MONITOR.to_owned(), Machine::A),
            ("odyssey-g5".to_owned(), Machine::A),
        ]);
        for _ in 0..3 {
            g.reconcile(ME, &away, &keys, t);
            g.reconcile_builtin(BuiltinMode::ExternalOnly, t);
        }
        assert!(!fake.active().unwrap().is_empty());
    }
}
