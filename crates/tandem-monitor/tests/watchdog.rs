//! The watchdog as a separate process: disarmed it touches nothing; if the one that launched it leaves without
//! disarming it, it tries to return (there is no such monitor here, so it fails with its reason).

use std::path::Path;

use monitor_ddc::Input;
use tandem_monitor::guard::{WatchPlan, Watchdog};

fn plan() -> WatchPlan {
    WatchPlan {
        // An id that no machine has: the test never touches a real monitor.
        monitor: "prueba-00000000".into(),
        restore_to: Input::DisplayPort,
    }
}

fn program() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_tandem-monitor-guard"))
}

#[test]
fn disarmed_it_ends_cleanly() {
    let status = Watchdog::spawn(program(), &plan())
        .unwrap()
        .disarm()
        .unwrap();
    assert!(status.success(), "{status:?}");
}

#[test]
fn abandoned_it_tries_to_go_back() {
    let status = Watchdog::spawn(program(), &plan())
        .unwrap()
        .abandon()
        .unwrap();
    assert_eq!(status.code(), Some(2), "{status:?}");
}
