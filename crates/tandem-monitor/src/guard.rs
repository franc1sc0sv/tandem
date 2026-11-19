//! Automatic rollback of a monitor's input (DESIGN §10, DECISIONS A8).
//!
//! Any hardware test that changes the input can leave the user unable to see the machine.
//! There are two nets:
//!
//! - [`InputGuard`], in the same process: it records the real input when armed and returns
//!   to it when dropped (also if the test panics).
//! - [`Watchdog`], a separate process (`tandem-monitor-guard`): if the test process
//!   dies without disarming it (it is killed, it hangs and CI cuts it off), its standard input
//!   closes without the [`DISARM`] command and the watchdog restores the input on its own.
//!   It works the same on macOS and Windows: the system closes the pipe when the parent dies.
//!
//! Both, before looking at the input, wait for the verdict of a switch in progress:
//! if the test dies right after the `Set 0x60`, the monitor still shows the original input
//! and would end up on the other one after the watchdog considered it fine.

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use monitor_ddc::{Clock, DdcBus, DdcError, EventKind, Input, Link, StandardMonitor, Verdict};

/// The command that disarms the watchdog: the test already left the input as it was.
pub const DISARM: &str = "disarm";

/// How long to wait for the verdict on the way back, and the most to wait for one in progress.
pub const RESTORE_TIMEOUT: Duration = Duration::from_secs(12);

/// How many events back to look for a requested switch that has no verdict yet.
const LOOKBACK: u16 = 16;

/// How often to look again while a switch is still in progress.
const SETTLE_POLL: Duration = Duration::from_millis(100);

/// Returns to the input there was when armed, when dropped.
pub struct InputGuard {
    monitor: StandardMonitor,
    clock: Arc<dyn Clock>,
    original: Input,
    done: bool,
}

impl std::fmt::Debug for InputGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputGuard")
            .field("original", &self.original)
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

impl InputGuard {
    /// Records the real input (the `0xEE` one, not the one of the port being asked). `clock`
    /// is the monitor's own: it is used to wait for a switch in progress when returning.
    pub fn arm(monitor: StandardMonitor, clock: Arc<dyn Clock>) -> Result<Self, DdcError> {
        let original = monitor
            .live()?
            .input()
            .ok_or_else(|| DdcError::Protocol("unknown real input".into()))?;
        Ok(InputGuard {
            monitor,
            clock,
            original,
            done: false,
        })
    }

    pub fn original(&self) -> Input {
        self.original
    }

    pub fn monitor(&self) -> &StandardMonitor {
        &self.monitor
    }

    /// Returns now, with the firmware's verdict.
    pub fn restore(mut self) -> Result<Verdict, DdcError> {
        self.done = true;
        restore(&self.monitor, self.clock.as_ref(), self.original)
    }
}

impl Drop for InputGuard {
    fn drop(&mut self) {
        if !self.done
            && let Err(e) = restore(&self.monitor, self.clock.as_ref(), self.original)
        {
            tracing::error!("could not return to input {}: {e}", self.original);
        }
    }
}

/// Sets `to` if it is not the one on screen, after any switch in progress ends.
fn restore(monitor: &StandardMonitor, clock: &dyn Clock, to: Input) -> Result<Verdict, DdcError> {
    settle(monitor, clock);
    if monitor.live().ok().and_then(|l| l.input()) == Some(to) {
        return Ok(Verdict::Confirmed { input: Some(to) });
    }
    monitor.switch_input(to, RESTORE_TIMEOUT)
}

/// Waits, up to [`RESTORE_TIMEOUT`], for a switch requested over DDC to have its verdict. If
/// it expires, it carries on anyway: returning is what matters.
fn settle(monitor: &StandardMonitor, clock: &dyn Clock) {
    let deadline = clock.now() + RESTORE_TIMEOUT;
    while switch_pending(monitor) {
        if clock.now() >= deadline {
            tracing::warn!("the switch in progress gave no verdict: returning anyway");
            return;
        }
        clock.sleep(SETTLE_POLL);
    }
}

/// There is a `ddc-switch-request` without its `switch-done`/`switch-failed`. If it cannot be
/// read, it is assumed there is: while the input changes the link retrains and does not answer.
fn switch_pending(monitor: &StandardMonitor) -> bool {
    let Ok(ring) = monitor.ring_info() else {
        return true;
    };
    for back in 0..LOOKBACK.min(ring.newest) {
        let Ok(e) = monitor.event(ring.newest.wrapping_sub(back)) else {
            return true;
        };
        match e.kind() {
            EventKind::DdcSwitchRequest => return true,
            EventKind::SwitchDone | EventKind::SwitchFailed | EventKind::Boot => return false,
            _ => {}
        }
    }
    false
}

/// What the watchdog has to restore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchPlan {
    /// The monitor id (`<model>-<hash>`): handles do not survive between processes.
    pub monitor: String,
    pub restore_to: Input,
}

impl WatchPlan {
    pub fn to_args(&self) -> Vec<String> {
        vec![self.monitor.clone(), self.restore_to.as_str().to_owned()]
    }

    pub fn from_args(args: &[String]) -> Result<Self, String> {
        match args {
            [monitor, input] => Ok(WatchPlan {
                monitor: monitor.clone(),
                restore_to: input.parse().map_err(|e| format!("{e}"))?,
            }),
            _ => Err("usage: tandem-monitor-guard <monitor-id> <dp|hdmi>".into()),
        }
    }
}

/// What happened to the watched process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchOutcome {
    /// The test ended well and disarmed it.
    Disarmed,
    /// The input was closed without the command: the test process died.
    ParentGone,
}

/// Waits on `control` (the watchdog's standard input) for the command or the close.
pub fn watch(control: impl BufRead) -> WatchOutcome {
    for line in control.lines() {
        match line {
            Ok(l) if l.trim() == DISARM => return WatchOutcome::Disarmed,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    WatchOutcome::ParentGone
}

/// Restores the input of monitor `plan.monitor` with a new bus (the watchdog's own). Only
/// a compatible monitor: the probe goes before the Set.
pub fn restore_by_id(
    bus: Arc<dyn DdcBus>,
    clock: Arc<dyn Clock>,
    plan: &WatchPlan,
) -> Result<Verdict, String> {
    let monitors = bus.monitors().map_err(|e| e.to_string())?;
    let info = monitors
        .into_iter()
        .find(|m| m.id().is_some_and(|id| id.as_str() == plan.monitor))
        .ok_or_else(|| format!("monitor {} is not visible", plan.monitor))?;
    let monitor = Link::new(bus, clock.clone(), info.handle)
        .open_standard()
        .map_err(|e| e.to_string())?;
    restore(&monitor, clock.as_ref(), plan.restore_to).map_err(|e| e.to_string())
}

/// The running watchdog, seen from the test.
#[derive(Debug)]
pub struct Watchdog {
    child: Child,
}

impl Watchdog {
    /// Launches `program` (the `tandem-monitor-guard` binary) with the plan.
    pub fn spawn(program: &Path, plan: &WatchPlan) -> io::Result<Self> {
        let child = Command::new(program)
            .args(plan.to_args())
            .stdin(Stdio::piped())
            .spawn()?;
        Ok(Watchdog { child })
    }

    /// The test already returned on its own: the watchdog ends without touching anything.
    pub fn disarm(mut self) -> io::Result<std::process::ExitStatus> {
        if let Some(mut stdin) = self.child.stdin.take() {
            writeln!(stdin, "{DISARM}")?;
        }
        self.child.wait()
    }

    /// Closes the input without disarming, as if the test had died (to test the
    /// watchdog).
    pub fn abandon(mut self) -> io::Result<std::process::ExitStatus> {
        drop(self.child.stdin.take());
        self.child.wait()
    }
}

#[cfg(test)]
mod tests {
    use monitor_ddc::fake::{BusOp, FakeBus, FakeClock, FakeMonitorConfig, Personality};
    use monitor_ddc::{Link, VCP_INPUT};

    use super::*;

    fn arm(bus: &FakeBus, m: StandardMonitor) -> InputGuard {
        InputGuard::arm(m, Arc::new(bus.clock())).unwrap()
    }

    fn standard() -> (FakeBus, monitor_ddc::MonitorHandle, StandardMonitor) {
        let clock = FakeClock::new();
        let bus = FakeBus::new(clock.clone());
        let h = bus.add(FakeMonitorConfig::new(Personality::Standard));
        let m = Link::new(Arc::new(bus.clone()), Arc::new(clock), h)
            .open_standard()
            .unwrap();
        (bus, h, m)
    }

    #[test]
    fn dropping_the_guard_goes_back() {
        let (bus, h, m) = standard();
        {
            let guard = arm(&bus, m);
            assert_eq!(guard.original(), Input::DisplayPort);
            let v = guard
                .monitor()
                .switch_input(Input::Hdmi, RESTORE_TIMEOUT)
                .unwrap();
            assert_eq!(
                v,
                Verdict::Confirmed {
                    input: Some(Input::Hdmi)
                }
            );
            assert_eq!(bus.screen_input(h), Some(Input::Hdmi));
        }
        assert_eq!(bus.screen_input(h), Some(Input::DisplayPort));
    }

    #[test]
    fn a_panicking_test_still_goes_back() {
        let (bus, h, m) = standard();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let guard = arm(&bus, m);
            guard
                .monitor()
                .switch_input(Input::Hdmi, RESTORE_TIMEOUT)
                .unwrap();
            panic!("the test failed midway");
        }));
        assert!(r.is_err());
        assert_eq!(bus.screen_input(h), Some(Input::DisplayPort));
    }

    #[test]
    fn restoring_what_is_already_there_sends_nothing() {
        let (bus, h, m) = standard();
        bus.clear_ops();
        let v = arm(&bus, m).restore().unwrap();
        assert_eq!(
            v,
            Verdict::Confirmed {
                input: Some(Input::DisplayPort)
            }
        );
        assert!(!bus.ops_for(h).iter().any(|op| matches!(
            op,
            BusOp::Set {
                code: VCP_INPUT,
                ..
            }
        )));
    }

    #[test]
    fn the_watchdog_restores_only_if_the_parent_dies() {
        assert_eq!(
            watch(io::Cursor::new(format!("something\n{DISARM}\n"))),
            WatchOutcome::Disarmed
        );
        assert_eq!(
            watch(io::Cursor::new("something\n")),
            WatchOutcome::ParentGone
        );
        assert_eq!(watch(io::Cursor::new("")), WatchOutcome::ParentGone);
    }

    #[test]
    fn the_watchdog_finds_the_monitor_by_id_and_goes_back() {
        let clock = FakeClock::new();
        let bus = FakeBus::new(clock.clone());
        let h = bus.add(FakeMonitorConfig::new(Personality::Standard));
        bus.osd_switch(h, Input::Hdmi);
        let id = monitor_ddc::DdcBus::monitors(&bus).unwrap()[0]
            .id()
            .unwrap()
            .to_string();
        let plan = WatchPlan::from_args(&[id, "dp".into()]).unwrap();
        assert_eq!(WatchPlan::from_args(&plan.to_args()).unwrap(), plan);
        let v = restore_by_id(Arc::new(bus.clone()), Arc::new(clock), &plan).unwrap();
        assert_eq!(
            v,
            Verdict::Confirmed {
                input: Some(Input::DisplayPort)
            }
        );
        assert_eq!(bus.screen_input(h), Some(Input::DisplayPort));
    }

    /// Review of #80: the test dies right after the Set to HDMI and the watchdog wakes up
    /// when the monitor still shows DP. It cannot take it for granted: the switch in progress
    /// ends later and the user is left unable to see the PC.
    #[test]
    fn the_watchdog_waits_for_a_switch_in_flight() {
        let clock = FakeClock::new();
        let bus = FakeBus::new(clock.clone());
        let h = bus.add(FakeMonitorConfig::new(Personality::Standard));
        let id = monitor_ddc::DdcBus::monitors(&bus).unwrap()[0]
            .id()
            .unwrap()
            .to_string();
        monitor_ddc::DdcBus::set_vcp(&bus, h, VCP_INPUT, Input::Hdmi.vcp()).unwrap();
        let plan = WatchPlan {
            monitor: id,
            restore_to: Input::DisplayPort,
        };
        let v = restore_by_id(Arc::new(bus.clone()), Arc::new(clock.clone()), &plan).unwrap();
        assert_eq!(
            v,
            Verdict::Confirmed {
                input: Some(Input::DisplayPort)
            }
        );
        clock.advance(Duration::from_secs(2));
        assert_eq!(bus.screen_input(h), Some(Input::DisplayPort));
    }

    /// A switch in progress that never gives a verdict does not leave the watchdog waiting forever:
    /// when the wait expires it returns anyway.
    #[test]
    fn a_pending_switch_without_verdict_still_ends_on_the_original() {
        let clock = FakeClock::new();
        let bus = FakeBus::new(clock.clone());
        let mut cfg = FakeMonitorConfig::new(Personality::Standard);
        cfg.switch = monitor_ddc::fake::SwitchBehavior::Silent;
        let h = bus.add(cfg);
        let id = monitor_ddc::DdcBus::monitors(&bus).unwrap()[0]
            .id()
            .unwrap()
            .to_string();
        monitor_ddc::DdcBus::set_vcp(&bus, h, VCP_INPUT, Input::Hdmi.vcp()).unwrap();
        let plan = WatchPlan {
            monitor: id,
            restore_to: Input::DisplayPort,
        };
        let start = clock.now();
        restore_by_id(Arc::new(bus.clone()), Arc::new(clock.clone()), &plan).unwrap();
        assert!(
            clock.now() - start >= RESTORE_TIMEOUT,
            "waited for the verdict"
        );
        clock.advance(Duration::from_secs(2));
        assert_eq!(bus.screen_input(h), Some(Input::DisplayPort));
    }

    /// The same inside the process: the guard is dropped with a half-done switch.
    #[test]
    fn the_guard_waits_for_a_switch_in_flight() {
        let (bus, h, m) = standard();
        let clock = bus.clock();
        {
            let _guard = arm(&bus, m);
            monitor_ddc::DdcBus::set_vcp(&bus, h, VCP_INPUT, Input::Hdmi.vcp()).unwrap();
        }
        clock.advance(Duration::from_secs(2));
        assert_eq!(bus.screen_input(h), Some(Input::DisplayPort));
    }

    #[test]
    fn the_watchdog_never_writes_to_a_factory_monitor() {
        let clock = FakeClock::new();
        let bus = FakeBus::new(clock.clone());
        let h = bus.add(FakeMonitorConfig::new(Personality::Factory));
        let id = monitor_ddc::DdcBus::monitors(&bus).unwrap()[0]
            .id()
            .unwrap()
            .to_string();
        let plan = WatchPlan {
            monitor: id,
            restore_to: Input::Hdmi,
        };
        assert!(restore_by_id(Arc::new(bus.clone()), Arc::new(clock), &plan).is_err());
        assert!(
            !bus.ops_for(h)
                .iter()
                .any(|op| matches!(op, BusOp::Set { .. }))
        );
        assert!(WatchPlan::from_args(&["only-one".into()]).is_err());
    }
}
