//! Pruebas con el monitor de verdad. Nunca en CI: `cargo test -p tandem-monitor -- --ignored`
//! en la máquina con el monitor compatible.
//!
//! Toda prueba que cambia la entrada lleva las dos redes de la vuelta automática: el
//! [`InputGuard`] en el proceso y el vigilante aparte por si este proceso muere.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use monitor_ddc::{Link, Probe, SystemClock, Verdict};
use tandem_monitor::guard::{InputGuard, WatchPlan, Watchdog};

#[test]
#[ignore = "hardware"]
fn switch_to_the_other_input_and_come_back() {
    let bus = monitor_ddc::system_bus().expect("sin canal DDC en esta máquina");
    let clock: Arc<dyn monitor_ddc::Clock> = Arc::new(SystemClock::new());
    let (info, monitor) = bus
        .monitors()
        .expect("no se pudieron enumerar los monitores")
        .into_iter()
        .find_map(|info| {
            let link = Link::new(bus.clone(), clock.clone(), info.handle);
            match link.probe() {
                Probe::Compatible(_) => link.open_standard().ok().map(|m| (info.clone(), m)),
                _ => None,
            }
        })
        .expect("ningún monitor compatible");
    let id = info.id().expect("sin EDID").to_string();
    let guard = InputGuard::arm(monitor).expect("no se leyó la entrada real");
    let original = guard.original();
    let watchdog = Watchdog::spawn(
        Path::new(env!("CARGO_BIN_EXE_tandem-monitor-guard")),
        &WatchPlan {
            monitor: id,
            restore_to: original,
        },
    )
    .expect("no arrancó el vigilante");
    let other = match original {
        monitor_ddc::Input::DisplayPort => monitor_ddc::Input::Hdmi,
        monitor_ddc::Input::Hdmi => monitor_ddc::Input::DisplayPort,
    };
    let verdict = guard
        .monitor()
        .switch_input(other, Duration::from_secs(12))
        .expect("no se pudo pedir el cambio");
    // Se vuelve pase lo que pase con el veredicto; después se mira.
    let back = guard.restore().expect("no se pudo volver");
    watchdog.disarm().expect("el vigilante no terminó");
    assert!(
        matches!(verdict, Verdict::Confirmed { .. } | Verdict::Failed { .. }),
        "sin veredicto: {verdict:?}"
    );
    assert_eq!(
        back,
        Verdict::Confirmed {
            input: Some(original)
        }
    );
}
