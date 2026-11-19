//! Monitor input switch with automatic rollback.
//!
//! Suite rule: **nothing changes an input without the two safety nets** of `tandem-monitor`:
//! the in-process [`InputGuard`] (restores on drop) and the separate watchdog
//! (`tandem-monitor-guard`), which restores if the process dies. If the watchdog is not next
//! to the executable, the check is skipped instead of risking the user not seeing the PC.
//!
//! Messages carry neither the monitor's identifier nor its EDID.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use monitor_ddc::{Clock, Input, Link, Probe, StandardMonitor, SystemClock, Verdict};
use tandem_monitor::guard::{InputGuard, WatchPlan, Watchdog};

use crate::report::CheckResult;

const SWITCH_TIMEOUT: Duration = Duration::from_secs(12);
/// How long to wait for the watchdog after killing the process that switched the input.
const WATCHDOG_WAIT: Duration = Duration::from_secs(45);

pub const ROUNDTRIP: &str = "monitor-roundtrip";
pub const KILL_MIDWAY: &str = "monitor-kill";
const COVERS: &[&str] = &["H3", "T-109", "T-110"];

/// The watchdog, next to the current executable.
pub fn guard_exe() -> PathBuf {
    let mut p = std::env::current_exe().unwrap_or_default();
    p.set_file_name(format!(
        "tandem-monitor-guard{}",
        std::env::consts::EXE_SUFFIX
    ));
    p
}

pub fn other_input(i: Input) -> Input {
    match i {
        Input::DisplayPort => Input::Hdmi,
        Input::Hdmi => Input::DisplayPort,
    }
}

/// A compatible monitor, opened, and its identifier (which does not appear in messages).
pub struct Found {
    id: String,
    monitor: StandardMonitor,
    clock: Arc<dyn Clock>,
}

pub fn find() -> Result<Found, String> {
    let bus = monitor_ddc::system_bus().map_err(|e| format!("no DDC channel: {e}"))?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
    let monitors = bus
        .monitors()
        .map_err(|e| format!("monitors were not enumerated: {e}"))?;
    for info in monitors {
        let link = Link::new(bus.clone(), clock.clone(), info.handle);
        if let Probe::Compatible(_) = link.probe()
            && let (Ok(monitor), Some(id)) = (link.open_standard(), info.id())
        {
            return Ok(Found {
                id: id.to_string(),
                monitor,
                clock,
            });
        }
    }
    Err("no compatible monitor on this machine".into())
}

/// What is required after the switch and the return: confirmed return to the original input.
/// The switch verdict can be either final one (the monitor may have no signal on the
/// other input), but there has to be one.
pub fn classify_roundtrip(
    original: Input,
    switched: &Result<Verdict, String>,
    back: &Result<Verdict, String>,
) -> Result<String, String> {
    let went = match switched {
        Ok(Verdict::Confirmed { .. }) => "switch confirmed",
        Ok(Verdict::Failed { .. }) => {
            "switch got no signal on the other input (the firmware said so)"
        }
        Ok(other) => {
            return Err(format!(
                "the switch did not give a final verdict: {other:?}"
            ));
        }
        Err(e) => return Err(format!("the switch could not be requested: {e}")),
    };
    match back {
        Ok(Verdict::Confirmed { input }) if *input == Some(original) => {
            Ok(format!("{went}; return confirmed to the original input"))
        }
        other => Err(format!(
            "THE RETURN WAS NOT CONFIRMED ({other:?}); check the monitor input by hand"
        )),
    }
}

/// Switches to the other input and back, with the guard and the watchdog.
pub fn roundtrip(label: &str) -> CheckResult {
    let r = CheckResult::new(ROUNDTRIP, label, COVERS);
    let started = Instant::now();
    let guard_path = guard_exe();
    if !guard_path.exists() {
        return r.skip(
            "tandem-monitor-guard is missing next to the executable: the input is not touched",
        );
    }
    let found = match find() {
        Ok(f) => f,
        Err(e) => return r.skip(e),
    };
    let guard = match InputGuard::arm(found.monitor, found.clock.clone()) {
        Ok(g) => g,
        Err(e) => return r.skip(format!("the real input was not read: {e}")),
    };
    let original = guard.original();
    let watchdog = match Watchdog::spawn(
        &guard_path,
        &WatchPlan {
            monitor: found.id,
            restore_to: original,
        },
    ) {
        Ok(w) => w,
        // `guard` is dropped here without having changed anything.
        Err(e) => return r.skip(format!("the watchdog did not start: {e}")),
    };
    let switched = guard
        .monitor()
        .switch_input(other_input(original), SWITCH_TIMEOUT)
        .map_err(|e| e.to_string());
    // The return happens whatever the switch does; only afterwards is it inspected.
    let back = guard.restore().map_err(|e| e.to_string());
    let confirmed = matches!(
        &back,
        Ok(Verdict::Confirmed { input }) if *input == Some(original)
    );
    // Disarmed only with the return confirmed; otherwise it drops without disarming and retries.
    let watch = if confirmed {
        watchdog.disarm()
    } else {
        watchdog.abandon()
    };
    let mut out = match classify_roundtrip(original, &switched, &back) {
        Ok(d) => r.pass(d),
        Err(d) => r.fail(d),
    };
    if let Err(e) = watch {
        out = out.fail(format!("the watchdog did not finish: {e}"));
    }
    out.millis = started.elapsed().as_millis() as u64;
    out
}

/// The child process of [`kill_midway`]: arms both nets, switches, signals and waits
/// to be killed. If nobody kills it, it returns on its own after 120 s.
pub fn hold() -> anyhow::Result<()> {
    let guard_path = guard_exe();
    anyhow::ensure!(guard_path.exists(), "tandem-monitor-guard is missing");
    let found = find().map_err(anyhow::Error::msg)?;
    let guard = InputGuard::arm(found.monitor, found.clock.clone())?;
    let original = guard.original();
    let _watchdog = Watchdog::spawn(
        &guard_path,
        &WatchPlan {
            monitor: found.id,
            restore_to: original,
        },
    )?;
    println!("armed");
    std::io::stdout().flush()?;
    let _ = guard
        .monitor()
        .switch_input(other_input(original), SWITCH_TIMEOUT);
    println!("switched");
    std::io::stdout().flush()?;
    std::thread::sleep(Duration::from_secs(120));
    // `guard` restores on drop.
    Ok(())
}

/// Current monitor input (`dp`/`hdmi`), so `run.sh` knows what to return to. Read only.
pub fn current_input_name() -> Result<String, String> {
    let found = find()?;
    live_input(&found)
        .map(|i| i.as_str().to_owned())
        .ok_or_else(|| "the real input was not read".to_owned())
}

/// Returns to `target` with the watchdog: it is launched with the input closed, which for it means "the
/// process died" and restores. It is the explicit restore of `run.sh`'s cleanup.
pub fn restore_with_guard(target: &str) -> Result<(), String> {
    let want: Input = target.parse().map_err(|e| format!("invalid input: {e}"))?;
    let guard_path = guard_exe();
    if !guard_path.exists() {
        return Err("tandem-monitor-guard is missing next to the executable".into());
    }
    let found = find()?;
    let plan = WatchPlan {
        monitor: found.id.clone(),
        restore_to: want,
    };
    let status = Command::new(&guard_path)
        .args(plan.to_args())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("the watchdog did not start: {e}"))?;
    if !status.success() {
        return Err(format!("the watchdog ended with {status}"));
    }
    if live_input(&found) == Some(want) {
        Ok(())
    } else {
        Err("after the watchdog the input is not the expected one".into())
    }
}

fn live_input(found: &Found) -> Option<Input> {
    found.monitor.live().ok().and_then(|l| l.input())
}

/// Result of [`kill_midway`] according to what was observed. With no switch there is nothing to test:
/// the "returned" input would be the one that never moved.
#[derive(Debug, PartialEq, Eq)]
pub enum KillVerdict {
    Pass,
    NeverSwitched,
    NotRestored,
}

pub fn classify_kill(switched: bool, restored: bool) -> KillVerdict {
    match (switched, restored) {
        (false, _) => KillVerdict::NeverSwitched,
        (true, true) => KillVerdict::Pass,
        (true, false) => KillVerdict::NotRestored,
    }
}

/// Kills the process that switched the input midway: the watchdog has to restore on its
/// own. Last resort: if it did not return, this function does it and the check fails.
pub fn kill_midway(label: &str) -> CheckResult {
    let r = CheckResult::new(KILL_MIDWAY, label, COVERS);
    let started = Instant::now();
    if !guard_exe().exists() {
        return r.skip(
            "tandem-monitor-guard is missing next to the executable: the input is not touched",
        );
    }
    let found = match find() {
        Ok(f) => f,
        Err(e) => return r.skip(e),
    };
    let Some(original) = live_input(&found) else {
        return r.skip("the real input was not read");
    };
    let me = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return r.skip(format!("no executable path: {e}")),
    };
    let mut child = match Command::new(me)
        .arg("monitor-hold")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return r.skip(format!("the test process did not start: {e}")),
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return r.skip("no stdout from the test process");
    };
    let mut lines = BufReader::new(stdout).lines();
    let mut switched = false;
    while let Some(Ok(l)) = lines.next() {
        if l == "switched" {
            switched = true;
            break;
        }
    }
    // A moment with the switch in progress or done, then the process is killed.
    std::thread::sleep(Duration::from_millis(1500));
    let _ = child.kill();
    let _ = child.wait();
    let deadline = Instant::now() + WATCHDOG_WAIT;
    let restored = loop {
        if live_input(&found) == Some(original) {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    let verdict = classify_kill(switched, restored);
    if !restored {
        // Third net: the user is never left without seeing the PC.
        let _ = found.monitor.switch_input(original, SWITCH_TIMEOUT);
    }
    let mut out = match verdict {
        KillVerdict::Pass => {
            r.pass("the watchdog returned to the original input after killing the process")
        }
        KillVerdict::NeverSwitched => {
            r.skip("the test process never switched the input: the watchdog was not tested")
        }
        KillVerdict::NotRestored => {
            r.fail("the watchdog did NOT return in time; this test restored it. Check the watchdog")
        }
    };
    out.millis = started.elapsed().as_millis() as u64;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn confirmed(i: Input) -> Result<Verdict, String> {
        Ok(Verdict::Confirmed { input: Some(i) })
    }

    #[test]
    fn the_other_input_is_an_involution() {
        for i in [Input::DisplayPort, Input::Hdmi] {
            assert_ne!(other_input(i), i);
            assert_eq!(other_input(other_input(i)), i);
        }
    }

    #[test]
    fn a_confirmed_return_to_the_original_passes() {
        let ok = classify_roundtrip(
            Input::DisplayPort,
            &confirmed(Input::Hdmi),
            &confirmed(Input::DisplayPort),
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn returning_to_the_wrong_input_fails() {
        let bad = classify_roundtrip(
            Input::DisplayPort,
            &confirmed(Input::Hdmi),
            &confirmed(Input::Hdmi),
        );
        assert!(bad.unwrap_err().contains("NOT CONFIRMED"));
    }

    #[test]
    fn a_failed_switch_is_acceptable_only_if_the_return_is_confirmed() {
        let failed = Ok(Verdict::Failed {
            target: Some(Input::Hdmi),
            current: Some(Input::DisplayPort),
        });
        assert!(
            classify_roundtrip(Input::DisplayPort, &failed, &confirmed(Input::DisplayPort)).is_ok()
        );
        assert!(classify_roundtrip(Input::DisplayPort, &failed, &Err("x".into())).is_err());
    }

    #[test]
    fn a_request_error_fails_even_if_the_return_worked() {
        let r = classify_roundtrip(
            Input::Hdmi,
            &Err("no channel".into()),
            &confirmed(Input::Hdmi),
        );
        assert!(r.is_err());
    }

    #[test]
    fn kill_never_passes_if_the_child_never_switched() {
        assert_eq!(classify_kill(false, true), KillVerdict::NeverSwitched);
        assert_eq!(classify_kill(false, false), KillVerdict::NeverSwitched);
    }

    #[test]
    fn kill_passes_only_with_switch_and_restore() {
        assert_eq!(classify_kill(true, true), KillVerdict::Pass);
        assert_eq!(classify_kill(true, false), KillVerdict::NotRestored);
    }

    #[test]
    fn the_guard_is_looked_up_next_to_the_executable() {
        let p = guard_exe();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("tandem-monitor-guard"));
        assert_eq!(p.parent(), std::env::current_exe().unwrap().parent());
    }
}
