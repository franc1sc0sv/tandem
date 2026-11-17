//! El vigilante como proceso aparte: desarmado no toca nada; si el que lo lanzó se va sin
//! desarmarlo, intenta volver (aquí no hay ese monitor, así que falla con su motivo).

use std::path::Path;

use monitor_ddc::Input;
use tandem_monitor::guard::{WatchPlan, Watchdog};

fn plan() -> WatchPlan {
    WatchPlan {
        // Un id que ningún equipo tiene: la prueba nunca toca un monitor de verdad.
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
