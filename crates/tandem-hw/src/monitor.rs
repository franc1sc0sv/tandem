//! Cambio de entrada del monitor con vuelta automática.
//!
//! Regla de la suite: **nada cambia una entrada sin las dos redes** de `tandem-monitor`:
//! el [`InputGuard`] dentro del proceso (vuelve al soltarse) y el vigilante aparte
//! (`tandem-monitor-guard`), que vuelve si el proceso muere. Si el vigilante no está junto
//! al ejecutable, la comprobación se omite en vez de arriesgar al usuario sin ver la PC.
//!
//! Los mensajes no llevan el identificador ni el EDID del monitor.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use monitor_ddc::{Clock, Input, Link, Probe, StandardMonitor, SystemClock, Verdict};
use tandem_monitor::guard::{InputGuard, WatchPlan, Watchdog};

use crate::report::CheckResult;

const SWITCH_TIMEOUT: Duration = Duration::from_secs(12);
/// Cuánto se espera al vigilante tras matar al proceso que cambió la entrada.
const WATCHDOG_WAIT: Duration = Duration::from_secs(45);

pub const ROUNDTRIP: &str = "monitor-roundtrip";
pub const KILL_MIDWAY: &str = "monitor-kill";
const COVERS: &[&str] = &["H3", "T-109", "T-110"];

/// El vigilante, junto al ejecutable actual.
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

/// Un monitor compatible abierto y su identificador (que no sale en los mensajes).
pub struct Found {
    id: String,
    monitor: StandardMonitor,
    clock: Arc<dyn Clock>,
}

pub fn find() -> Result<Found, String> {
    let bus = monitor_ddc::system_bus().map_err(|e| format!("sin canal DDC: {e}"))?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
    let monitors = bus
        .monitors()
        .map_err(|e| format!("no se enumeraron los monitores: {e}"))?;
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
    Err("ningún monitor compatible en esta máquina".into())
}

/// Lo que se exige tras el cambio y la vuelta: la vuelta confirmada a la entrada original.
/// El veredicto del cambio puede ser cualquiera de los dos finales (el monitor puede no
/// tener señal en la otra entrada), pero tiene que haberlo.
pub fn classify_roundtrip(
    original: Input,
    switched: &Result<Verdict, String>,
    back: &Result<Verdict, String>,
) -> Result<String, String> {
    let went = match switched {
        Ok(Verdict::Confirmed { .. }) => "cambio confirmado",
        Ok(Verdict::Failed { .. }) => "cambio sin señal en la otra entrada (el firmware lo dijo)",
        Ok(other) => return Err(format!("el cambio no dio un veredicto final: {other:?}")),
        Err(e) => return Err(format!("no se pudo pedir el cambio: {e}")),
    };
    match back {
        Ok(Verdict::Confirmed { input }) if *input == Some(original) => {
            Ok(format!("{went}; vuelta confirmada a la entrada original"))
        }
        other => Err(format!(
            "LA VUELTA NO QUEDÓ CONFIRMADA ({other:?}); revisar la entrada del monitor a mano"
        )),
    }
}

/// Cambia a la otra entrada y vuelve, con el guardia y el vigilante.
pub fn roundtrip(label: &str) -> CheckResult {
    let r = CheckResult::new(ROUNDTRIP, label, COVERS);
    let started = Instant::now();
    let guard_path = guard_exe();
    if !guard_path.exists() {
        return r.skip("falta tandem-monitor-guard junto al ejecutable: no se toca la entrada");
    }
    let found = match find() {
        Ok(f) => f,
        Err(e) => return r.skip(e),
    };
    let guard = match InputGuard::arm(found.monitor, found.clock.clone()) {
        Ok(g) => g,
        Err(e) => return r.skip(format!("no se leyó la entrada real: {e}")),
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
        // `guard` se suelta aquí sin haber cambiado nada.
        Err(e) => return r.skip(format!("no arrancó el vigilante: {e}")),
    };
    let switched = guard
        .monitor()
        .switch_input(other_input(original), SWITCH_TIMEOUT)
        .map_err(|e| e.to_string());
    // Se vuelve pase lo que pase con el cambio; después se mira.
    let back = guard.restore().map_err(|e| e.to_string());
    let confirmed = matches!(
        &back,
        Ok(Verdict::Confirmed { input }) if *input == Some(original)
    );
    // Desarmado solo con la vuelta confirmada; si no, lo suelta sin desarmar y reintenta él.
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
        out = out.fail(format!("el vigilante no terminó: {e}"));
    }
    out.millis = started.elapsed().as_millis() as u64;
    out
}

/// El proceso hijo de [`kill_midway`]: arma las dos redes, cambia, avisa y se queda
/// esperando a que lo maten. Si nadie lo mata, vuelve solo a los 120 s.
pub fn hold() -> anyhow::Result<()> {
    let guard_path = guard_exe();
    anyhow::ensure!(guard_path.exists(), "falta tandem-monitor-guard");
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
    // `guard` vuelve al soltarse.
    Ok(())
}

fn live_input(found: &Found) -> Option<Input> {
    found.monitor.live().ok().and_then(|l| l.input())
}

/// Mata el proceso que cambió la entrada a mitad de camino: el vigilante tiene que volver
/// solo. Último recurso: si no volvió, esta función lo hace y la comprobación falla.
pub fn kill_midway(label: &str) -> CheckResult {
    let r = CheckResult::new(KILL_MIDWAY, label, COVERS);
    let started = Instant::now();
    if !guard_exe().exists() {
        return r.skip("falta tandem-monitor-guard junto al ejecutable: no se toca la entrada");
    }
    let found = match find() {
        Ok(f) => f,
        Err(e) => return r.skip(e),
    };
    let Some(original) = live_input(&found) else {
        return r.skip("no se leyó la entrada real");
    };
    let me = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return r.skip(format!("sin ruta del ejecutable: {e}")),
    };
    let mut child = match Command::new(me)
        .arg("monitor-hold")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return r.skip(format!("no arrancó el proceso de prueba: {e}")),
    };
    let mut lines = BufReader::new(child.stdout.take().expect("stdout pedido")).lines();
    let mut switched = false;
    while let Some(Ok(l)) = lines.next() {
        if l == "switched" {
            switched = true;
            break;
        }
    }
    // Un momento con el cambio en curso o ya hecho, y se mata al proceso.
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
    let mut out = if restored {
        r.pass(format!(
            "el vigilante volvió a la entrada original tras matar al proceso (cambio hecho: {switched})"
        ))
    } else {
        // Tercera red: no se deja al usuario sin ver la PC.
        let _ = found.monitor.switch_input(original, SWITCH_TIMEOUT);
        r.fail("el vigilante NO volvió a tiempo; la restauró esta prueba. Revisar el vigilante")
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
        assert!(bad.unwrap_err().contains("NO QUEDÓ CONFIRMADA"));
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
            &Err("sin canal".into()),
            &confirmed(Input::Hdmi),
        );
        assert!(r.is_err());
    }

    #[test]
    fn the_guard_is_looked_up_next_to_the_executable() {
        let p = guard_exe();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("tandem-monitor-guard"));
        assert_eq!(p.parent(), std::env::current_exe().unwrap().parent());
    }
}
