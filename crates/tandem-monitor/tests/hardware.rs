//! Tests with the real monitor. Never in CI: `cargo test -p tandem-monitor -- --ignored`
//! on the machine with the compatible monitor.
//!
//! Every test that changes the input carries the two automatic-rollback nets: the
//! [`InputGuard`] in the process and the separate watchdog in case this process dies.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use monitor_ddc::{Link, Probe, SystemClock, Verdict};
use tandem_monitor::guard::{InputGuard, WatchPlan, Watchdog};

#[test]
#[ignore = "hardware"]
fn switch_to_the_other_input_and_come_back() {
    let bus = monitor_ddc::system_bus().expect("no DDC channel on this machine");
    let clock: Arc<dyn monitor_ddc::Clock> = Arc::new(SystemClock::new());
    let (info, monitor) = bus
        .monitors()
        .expect("the monitors could not be enumerated")
        .into_iter()
        .find_map(|info| {
            let link = Link::new(bus.clone(), clock.clone(), info.handle);
            match link.probe() {
                Probe::Compatible(_) => link.open_standard().ok().map(|m| (info.clone(), m)),
                _ => None,
            }
        })
        .expect("no compatible monitor");
    let id = info.id().expect("no EDID").to_string();
    let guard = InputGuard::arm(monitor, clock.clone()).expect("the real input was not read");
    let original = guard.original();
    let watchdog = Watchdog::spawn(
        Path::new(env!("CARGO_BIN_EXE_tandem-monitor-guard")),
        &WatchPlan {
            monitor: id,
            restore_to: original,
        },
    )
    .expect("the watchdog did not start");
    let other = match original {
        monitor_ddc::Input::DisplayPort => monitor_ddc::Input::Hdmi,
        monitor_ddc::Input::Hdmi => monitor_ddc::Input::DisplayPort,
    };
    let verdict = guard
        .monitor()
        .switch_input(other, Duration::from_secs(12))
        .expect("the switch could not be requested");
    // It returns whatever the verdict is; only afterwards is it inspected.
    let back = guard.restore();
    let returned = matches!(
        back,
        Ok(Verdict::Confirmed { input: Some(i) }) if i == original
    );
    // The watchdog is disarmed only if the return was confirmed. Otherwise it is released without
    // disarming: it retries on its own, and only afterwards is it checked.
    if returned {
        watchdog.disarm().expect("the watchdog did not finish");
    } else {
        let status = watchdog.abandon().expect("the watchdog did not finish");
        eprintln!("return inside the test unconfirmed ({back:?}); watchdog: {status:?}");
    }
    let back = back.expect("could not return");
    assert!(
        matches!(verdict, Verdict::Confirmed { .. } | Verdict::Failed { .. }),
        "no verdict: {verdict:?}"
    );
    assert_eq!(
        back,
        Verdict::Confirmed {
            input: Some(original)
        }
    );
}
