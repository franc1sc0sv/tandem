//! Automatic-rollback watchdog (see `tandem_monitor::guard`).
//!
//! `tandem-monitor-guard <monitor-id> <dp|hdmi>`: waits on standard input. If `disarm`
//! arrives, it exits without touching anything; if the input closes without it (the process
//! that launched it died), it puts the monitor back on the given input.

use std::process::ExitCode;
use std::sync::Arc;

use tandem_monitor::guard::{WatchOutcome, WatchPlan, restore_by_id, watch};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let plan = match WatchPlan::from_args(&args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(64);
        }
    };
    if watch(std::io::stdin().lock()) == WatchOutcome::Disarmed {
        return ExitCode::SUCCESS;
    }
    eprintln!(
        "the watched process ended without restoring: setting {} on {}",
        plan.restore_to, plan.monitor
    );
    let bus = match monitor_ddc::system_bus() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("could not restore: {e}");
            return ExitCode::from(2);
        }
    };
    match restore_by_id(bus, Arc::new(monitor_ddc::SystemClock::new()), &plan) {
        Ok(verdict) => {
            eprintln!("automatic rollback: {verdict:?}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("could not restore: {e}");
            ExitCode::from(2)
        }
    }
}
