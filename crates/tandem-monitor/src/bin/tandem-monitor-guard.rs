//! Vigilante de la vuelta automática (ver `tandem_monitor::guard`).
//!
//! `tandem-monitor-guard <id-del-monitor> <dp|hdmi>`: espera en la entrada estándar. Si llega
//! `disarm`, termina sin tocar nada; si la entrada se cierra sin eso (el proceso que lo lanzó
//! murió), vuelve el monitor a la entrada indicada.

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
        "el proceso vigilado terminó sin volver: se pone {} en {}",
        plan.restore_to, plan.monitor
    );
    let bus = match monitor_ddc::system_bus() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("no se pudo volver: {e}");
            return ExitCode::from(2);
        }
    };
    match restore_by_id(bus, Arc::new(monitor_ddc::SystemClock::new()), &plan) {
        Ok(verdict) => {
            eprintln!("vuelta automática: {verdict:?}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("no se pudo volver: {e}");
            ExitCode::from(2)
        }
    }
}
