//! Vuelta automática de la entrada de un monitor (DESIGN §10, DECISIONS A8).
//!
//! Toda prueba de hardware que cambia la entrada puede dejar al usuario sin ver la máquina.
//! Hay dos redes:
//!
//! - [`InputGuard`], en el mismo proceso: anota la entrada real al armarse y vuelve a ella al
//!   soltarse (también si la prueba entra en pánico).
//! - [`Watchdog`], un proceso aparte (`tandem-monitor-guard`): si el proceso de la prueba
//!   muere sin desarmarlo (se lo mata, se cuelga y lo corta el CI), su entrada estándar se
//!   cierra sin la orden [`DISARM`] y el vigilante restaura la entrada él solo. Funciona igual
//!   en macOS y Windows: el sistema cierra la tubería al morir el padre.
//!
//! Las dos, antes de mirar la entrada, esperan el veredicto de un cambio que esté en curso:
//! si la prueba muere justo después del `Set 0x60`, el monitor todavía muestra la entrada
//! original y terminaría en la otra después de que el vigilante la diera por buena.

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use monitor_ddc::{Clock, DdcBus, DdcError, EventKind, Input, Link, StandardMonitor, Verdict};

/// La orden que desarma al vigilante: la prueba ya dejó la entrada como estaba.
pub const DISARM: &str = "disarm";

/// Cuánto se espera el veredicto al volver, y lo más que se espera el de un cambio en curso.
pub const RESTORE_TIMEOUT: Duration = Duration::from_secs(12);

/// Cuántos eventos hacia atrás se busca un cambio pedido que todavía no tiene veredicto.
const LOOKBACK: u16 = 16;

/// Cada cuánto se vuelve a mirar mientras un cambio sigue en curso.
const SETTLE_POLL: Duration = Duration::from_millis(100);

/// Vuelve a la entrada que había al armarse, al soltarse.
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
    /// Anota la entrada real (la de `0xEE`, no la del puerto por el que se pregunta). `clock`
    /// es el mismo del monitor: con él se espera un cambio en curso al volver.
    pub fn arm(monitor: StandardMonitor, clock: Arc<dyn Clock>) -> Result<Self, DdcError> {
        let original = monitor
            .live()?
            .input()
            .ok_or_else(|| DdcError::Protocol("entrada real desconocida".into()))?;
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

    /// Vuelve ya, con el veredicto del firmware.
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
            tracing::error!("no se pudo volver a la entrada {}: {e}", self.original);
        }
    }
}

/// Pone `to` si no es la que está en pantalla, después de que termine cualquier cambio en curso.
fn restore(monitor: &StandardMonitor, clock: &dyn Clock, to: Input) -> Result<Verdict, DdcError> {
    settle(monitor, clock);
    if monitor.live().ok().and_then(|l| l.input()) == Some(to) {
        return Ok(Verdict::Confirmed { input: Some(to) });
    }
    monitor.switch_input(to, RESTORE_TIMEOUT)
}

/// Espera, hasta [`RESTORE_TIMEOUT`], a que un cambio pedido por DDC tenga su veredicto. Si
/// vence, se sigue igual: volver es lo que importa.
fn settle(monitor: &StandardMonitor, clock: &dyn Clock) {
    let deadline = clock.now() + RESTORE_TIMEOUT;
    while switch_pending(monitor) {
        if clock.now() >= deadline {
            tracing::warn!("el cambio en curso no dio veredicto: se vuelve igual");
            return;
        }
        clock.sleep(SETTLE_POLL);
    }
}

/// Hay un `ddc-switch-request` sin su `switch-done`/`switch-failed`. Si no se puede leer, se
/// supone que sí: mientras cambia de entrada el enlace se reentrena y no contesta.
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

/// Lo que el vigilante tiene que restaurar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchPlan {
    /// El id del monitor (`<modelo>-<hash>`): los handles no sobreviven entre procesos.
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
            _ => Err("uso: tandem-monitor-guard <id-del-monitor> <dp|hdmi>".into()),
        }
    }
}

/// Lo que pasó con el proceso vigilado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchOutcome {
    /// La prueba terminó bien y lo desarmó.
    Disarmed,
    /// La entrada se cerró sin la orden: el proceso de la prueba murió.
    ParentGone,
}

/// Espera en `control` (la entrada estándar del vigilante) hasta la orden o el cierre.
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

/// Restaura la entrada del monitor `plan.monitor` con un bus nuevo (el del vigilante). Solo
/// a un monitor compatible: el sondeo va antes del Set.
pub fn restore_by_id(
    bus: Arc<dyn DdcBus>,
    clock: Arc<dyn Clock>,
    plan: &WatchPlan,
) -> Result<Verdict, String> {
    let monitors = bus.monitors().map_err(|e| e.to_string())?;
    let info = monitors
        .into_iter()
        .find(|m| m.id().is_some_and(|id| id.as_str() == plan.monitor))
        .ok_or_else(|| format!("no se ve el monitor {}", plan.monitor))?;
    let monitor = Link::new(bus, clock.clone(), info.handle)
        .open_standard()
        .map_err(|e| e.to_string())?;
    restore(&monitor, clock.as_ref(), plan.restore_to).map_err(|e| e.to_string())
}

/// El vigilante en marcha, visto desde la prueba.
#[derive(Debug)]
pub struct Watchdog {
    child: Child,
}

impl Watchdog {
    /// Lanza `program` (el binario `tandem-monitor-guard`) con el plan.
    pub fn spawn(program: &Path, plan: &WatchPlan) -> io::Result<Self> {
        let child = Command::new(program)
            .args(plan.to_args())
            .stdin(Stdio::piped())
            .spawn()?;
        Ok(Watchdog { child })
    }

    /// La prueba ya volvió sola: el vigilante termina sin tocar nada.
    pub fn disarm(mut self) -> io::Result<std::process::ExitStatus> {
        if let Some(mut stdin) = self.child.stdin.take() {
            writeln!(stdin, "{DISARM}")?;
        }
        self.child.wait()
    }

    /// Cierra la entrada sin desarmar, como si la prueba hubiera muerto (para probar el
    /// vigilante).
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
            panic!("la prueba falló a mitad");
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
            watch(io::Cursor::new(format!("algo\n{DISARM}\n"))),
            WatchOutcome::Disarmed
        );
        assert_eq!(watch(io::Cursor::new("algo\n")), WatchOutcome::ParentGone);
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

    /// Revisión de #80: la prueba muere justo después del Set a HDMI y el vigilante despierta
    /// cuando el monitor todavía muestra DP. No puede darlo por hecho: el cambio en curso
    /// termina después y el usuario se queda sin ver la PC.
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

    /// Un cambio en curso que nunca da veredicto no deja al vigilante esperando para siempre:
    /// al vencer la espera vuelve igual.
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
            "esperó el veredicto"
        );
        clock.advance(Duration::from_secs(2));
        assert_eq!(bus.screen_input(h), Some(Input::DisplayPort));
    }

    /// Lo mismo dentro del proceso: el guardia se suelta con un cambio a medias.
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
        assert!(WatchPlan::from_args(&["solo-uno".into()]).is_err());
    }
}
