//! Cambio de entrada con vuelta automática.
//!
//! Cambiar la entrada desde la PC puede dejar al usuario sin ver la PC, y el monitor no
//! dice en qué entrada está (`0x60` siempre devuelve `0x09` en el 34"). Así que esta
//! operación siempre vuelve sola: pase lo que pase con la escritura de ida, se manda la
//! vuelta, con reintentos, y solo se adelanta (nunca se salta) si el usuario interrumpe.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tandem_core::model::Input;

use super::{DdcBus, DdcError, MonitorInfo, VCP_INPUT_SOURCE};

pub const MIN_RETURN_AFTER: Duration = Duration::from_secs(1);
pub const MAX_RETURN_AFTER: Duration = Duration::from_secs(300);
/// Pausa entre dos códigos de la misma fase (el monitor necesita resincronizar).
pub const CODE_GAP: Duration = Duration::from_secs(3);
/// Reintentos por código de vuelta: los errores sueltos de DDC son frecuentes.
pub const RETURN_ATTEMPTS: u32 = 3;
pub const RETRY_GAP: Duration = Duration::from_secs(1);

/// Códigos de `0x60` que se mandan para llegar a cada entrada, en orden.
///
/// Es una lista porque hay monitores que no está claro cuál obedecen: el 34" vuelve a DP
/// con `0x0F` o con `0x09` (spike S8), así que la vuelta manda los dos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputCodes {
    pub hdmi: Vec<u8>,
    pub display_port: Vec<u8>,
}

impl InputCodes {
    pub fn for_input(&self, input: Input) -> &[u8] {
        match input {
            Input::Hdmi => &self.hdmi,
            Input::DisplayPort => &self.display_port,
        }
    }
}

/// Valores de MCCS 2.2 para HDMI-1 y DP-1. Muchos monitores (los Samsung) no los obedecen.
pub fn mccs_standard() -> InputCodes {
    InputCodes {
        hdmi: vec![0x11],
        display_port: vec![0x0F],
    }
}

/// Perfil probado de un monitor conocido, buscado por el nombre del EDID.
/// Devuelve el nombre del perfil y sus códigos.
pub fn known_profile(info: &MonitorInfo) -> Option<(&'static str, InputCodes)> {
    let name = info.edid.as_ref()?.name.as_deref()?.to_ascii_uppercase();
    if name.contains("LC34G55T") {
        // docs/research/ddc-34-LC34G55T.md: HDMI con 0x06 (o 0x03); DP con 0x0F/0x09.
        return Some((
            "LC34G55T",
            InputCodes {
                hdmi: vec![0x06],
                display_port: vec![0x0F, 0x09],
            },
        ));
    }
    None
}

/// Espera interrumpible. Permite probar la lógica sin relojes reales.
pub trait Waiter {
    /// Espera `d`. Si `interruptible` y el usuario interrumpe, vuelve antes con `true`.
    fn wait(&mut self, d: Duration, interruptible: bool) -> bool;
}

/// Espera real con `thread::sleep`, consultando una bandera de interrupción (Ctrl+C).
pub struct SleepWaiter {
    interrupt: Option<&'static AtomicBool>,
}

impl SleepWaiter {
    const SLICE: Duration = Duration::from_millis(100);

    pub fn new(interrupt: Option<&'static AtomicBool>) -> Self {
        Self { interrupt }
    }

    fn interrupted(&self) -> bool {
        self.interrupt.is_some_and(|f| f.load(Ordering::SeqCst))
    }
}

impl Waiter for SleepWaiter {
    fn wait(&mut self, d: Duration, interruptible: bool) -> bool {
        let mut left = d;
        while !left.is_zero() {
            if interruptible && self.interrupted() {
                return true;
            }
            let step = left.min(Self::SLICE);
            std::thread::sleep(step);
            left -= step;
        }
        interruptible && self.interrupted()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("la vuelta debe ser entre {min} y {max} s (pediste {got} s)", min = MIN_RETURN_AFTER.as_secs(), max = MAX_RETURN_AFTER.as_secs(), got = .0.as_secs_f32())]
    ReturnAfterOutOfRange(Duration),
    #[error("no hay códigos para la ida")]
    NoTargetCodes,
    #[error("no hay códigos para la vuelta")]
    NoReturnCodes,
    /// Ida y vuelta a la misma entrada no es una vuelta: el monitor se quedaría donde lo
    /// dejó la ida, como un `set 0x60` suelto.
    #[error("la vuelta debe ir a otra entrada que la ida (pediste {} en ambas)", input_name(*.0))]
    SameInput(Input),
}

pub(crate) fn input_name(input: Input) -> &'static str {
    match input {
        Input::Hdmi => "hdmi",
        Input::DisplayPort => "dp",
    }
}

/// La otra entrada: con dos, es la única vuelta posible si no se pide otra cosa.
pub fn other_input(input: Input) -> Input {
    match input {
        Input::Hdmi => Input::DisplayPort,
        Input::DisplayPort => Input::Hdmi,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchPlan {
    pub monitor: usize,
    pub target: Input,
    pub target_codes: Vec<u8>,
    pub return_to: Input,
    pub return_codes: Vec<u8>,
    pub return_after: Duration,
}

impl SwitchPlan {
    pub fn validate(&self) -> Result<(), PlanError> {
        if !(MIN_RETURN_AFTER..=MAX_RETURN_AFTER).contains(&self.return_after) {
            return Err(PlanError::ReturnAfterOutOfRange(self.return_after));
        }
        if self.target == self.return_to {
            return Err(PlanError::SameInput(self.target));
        }
        if self.target_codes.is_empty() {
            return Err(PlanError::NoTargetCodes);
        }
        if self.return_codes.is_empty() {
            return Err(PlanError::NoReturnCodes);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Target,
    Return,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchEvent {
    Sent {
        phase: Phase,
        code: u8,
        attempt: u32,
        result: Result<(), DdcError>,
    },
    Waiting(Duration),
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SwitchError {
    #[error("no se pudo mandar la vuelta: el monitor puede haberse quedado en la otra entrada")]
    ReturnFailed,
    #[error("ninguna escritura de ida funcionó (la vuelta sí se mandó)")]
    TargetFailed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SwitchReport {
    /// Alguna escritura de ida fue aceptada.
    pub target_sent: bool,
    /// Alguna escritura de vuelta fue aceptada.
    pub returned: bool,
    pub interrupted: bool,
}

impl SwitchReport {
    /// La vuelta pesa más que la ida: es lo que deja al usuario viendo la PC.
    pub fn outcome(&self) -> Result<(), SwitchError> {
        if !self.returned {
            Err(SwitchError::ReturnFailed)
        } else if !self.target_sent {
            Err(SwitchError::TargetFailed)
        } else {
            Ok(())
        }
    }
}

/// Ejecuta el plan: ida, espera (interrumpible), vuelta (con reintentos, no interrumpible).
pub fn switch_with_return(
    bus: &mut dyn DdcBus,
    plan: &SwitchPlan,
    waiter: &mut dyn Waiter,
    on_event: &mut dyn FnMut(&SwitchEvent),
) -> Result<SwitchReport, DdcError> {
    // Validar antes de escribir: sin vuelta válida, o con un índice malo, la ida podría
    // dejar al monitor en la otra entrada.
    plan.validate()?;
    if plan.monitor >= bus.monitors().len() {
        return Err(DdcError::NoSuchMonitor(plan.monitor));
    }
    let mut report = SwitchReport::default();

    for (i, &code) in plan.target_codes.iter().enumerate() {
        if i > 0 && waiter.wait(CODE_GAP, true) {
            report.interrupted = true;
            break;
        }
        let result = bus.set_vcp(plan.monitor, VCP_INPUT_SOURCE, code.into());
        report.target_sent |= result.is_ok();
        on_event(&SwitchEvent::Sent {
            phase: Phase::Target,
            code,
            attempt: 1,
            result,
        });
    }

    // Si ninguna ida se aceptó no tiene sentido esperar; la vuelta va igual por si el
    // monitor obedeció aunque la llamada reportara error.
    if report.target_sent && !report.interrupted {
        on_event(&SwitchEvent::Waiting(plan.return_after));
        report.interrupted = waiter.wait(plan.return_after, true);
    }
    if report.interrupted {
        on_event(&SwitchEvent::Interrupted);
    }

    for (i, &code) in plan.return_codes.iter().enumerate() {
        if i > 0 {
            waiter.wait(CODE_GAP, false);
        }
        for attempt in 1..=RETURN_ATTEMPTS {
            if attempt > 1 {
                waiter.wait(RETRY_GAP, false);
            }
            let result = bus.set_vcp(plan.monitor, VCP_INPUT_SOURCE, code.into());
            let ok = result.is_ok();
            on_event(&SwitchEvent::Sent {
                phase: Phase::Return,
                code,
                attempt,
                result,
            });
            if ok {
                report.returned = true;
                break;
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ddc::edid;
    use crate::ddc::fake::{FakeDdc, FakeMonitor};
    use proptest::prelude::*;

    /// Espera simulada: registra lo pedido e interrumpe en la espera número `interrupt_at`.
    #[derive(Default)]
    struct FakeWaiter {
        waits: Vec<(Duration, bool)>,
        interrupt_at: Option<usize>,
    }

    impl Waiter for FakeWaiter {
        fn wait(&mut self, d: Duration, interruptible: bool) -> bool {
            self.waits.push((d, interruptible));
            interruptible && self.interrupt_at == Some(self.waits.len() - 1)
        }
    }

    fn plan() -> SwitchPlan {
        SwitchPlan {
            monitor: 0,
            target: Input::Hdmi,
            target_codes: vec![0x06],
            return_to: Input::DisplayPort,
            return_codes: vec![0x0F, 0x09],
            return_after: Duration::from_secs(10),
        }
    }

    fn bus() -> FakeDdc {
        FakeDdc::new(vec![FakeMonitor::default()])
    }

    fn written(bus: &FakeDdc) -> Vec<(u32, bool)> {
        bus.writes.iter().map(|w| (w.2, w.3)).collect()
    }

    fn run(bus: &mut FakeDdc, plan: &SwitchPlan, waiter: &mut FakeWaiter) -> SwitchReport {
        switch_with_return(bus, plan, waiter, &mut |_| {}).unwrap()
    }

    #[test]
    fn happy_path_goes_waits_and_returns_with_every_code() {
        let mut bus = bus();
        let mut waiter = FakeWaiter::default();
        let mut events = Vec::new();
        let report = switch_with_return(&mut bus, &plan(), &mut waiter, &mut |e| {
            events.push(e.clone())
        })
        .unwrap();
        assert_eq!(
            written(&bus),
            vec![(0x06, true), (0x0F, true), (0x09, true)]
        );
        assert!(bus.writes.iter().all(|w| w.1 == VCP_INPUT_SOURCE));
        assert_eq!(
            waiter.waits,
            vec![(Duration::from_secs(10), true), (CODE_GAP, false)]
        );
        assert_eq!(report.outcome(), Ok(()));
        assert!(!report.interrupted);
        assert_eq!(events.len(), 4);
        assert_eq!(events[1], SwitchEvent::Waiting(Duration::from_secs(10)));
    }

    #[test]
    fn interrupt_returns_early_but_still_returns() {
        let mut bus = bus();
        let mut waiter = FakeWaiter {
            interrupt_at: Some(0),
            ..Default::default()
        };
        let report = run(&mut bus, &plan(), &mut waiter);
        assert!(report.interrupted);
        assert!(report.returned);
        assert_eq!(
            written(&bus),
            vec![(0x06, true), (0x0F, true), (0x09, true)]
        );
    }

    #[test]
    fn interrupt_between_target_codes_skips_rest_of_target() {
        let mut bus = bus();
        let mut p = plan();
        p.target_codes = vec![0x06, 0x03];
        let mut waiter = FakeWaiter {
            interrupt_at: Some(0),
            ..Default::default()
        };
        let report = run(&mut bus, &p, &mut waiter);
        assert!(report.interrupted);
        assert_eq!(
            written(&bus),
            vec![(0x06, true), (0x0F, true), (0x09, true)]
        );
    }

    #[test]
    fn failed_target_skips_wait_but_sends_return() {
        let mut bus = bus();
        bus.write_script = [false].into();
        let mut waiter = FakeWaiter::default();
        let report = run(&mut bus, &plan(), &mut waiter);
        assert_eq!(
            written(&bus),
            vec![(0x06, false), (0x0F, true), (0x09, true)]
        );
        assert_eq!(waiter.waits, vec![(CODE_GAP, false)]);
        assert_eq!(report.outcome(), Err(SwitchError::TargetFailed));
    }

    #[test]
    fn return_is_retried_and_not_interruptible() {
        let mut bus = bus();
        bus.write_script = [true, false, false, true].into();
        let mut p = plan();
        p.return_codes = vec![0x0F];
        let mut waiter = FakeWaiter::default();
        let report = run(&mut bus, &p, &mut waiter);
        assert_eq!(
            written(&bus),
            vec![(0x06, true), (0x0F, false), (0x0F, false), (0x0F, true)]
        );
        assert_eq!(
            waiter.waits,
            vec![
                (Duration::from_secs(10), true),
                (RETRY_GAP, false),
                (RETRY_GAP, false)
            ]
        );
        assert_eq!(report.outcome(), Ok(()));
    }

    #[test]
    fn return_that_never_works_is_reported() {
        let mut bus = bus();
        bus.write_script = std::iter::once(true)
            .chain(std::iter::repeat_n(false, 6))
            .collect();
        let report = run(&mut bus, &plan(), &mut FakeWaiter::default());
        assert_eq!(bus.writes.len(), 1 + 2 * RETURN_ATTEMPTS as usize);
        assert_eq!(report.outcome(), Err(SwitchError::ReturnFailed));
    }

    #[test]
    fn bad_monitor_writes_nothing() {
        let mut bus = bus();
        let mut p = plan();
        p.monitor = 3;
        let err = switch_with_return(&mut bus, &p, &mut FakeWaiter::default(), &mut |_| {});
        assert_eq!(err, Err(DdcError::NoSuchMonitor(3)));
        assert!(bus.writes.is_empty());
    }

    // QA ronda 1: `switch_with_return` es público y solo la CLI validaba el plan. Un
    // plan sin códigos de vuelta mandaba la ida y dejaba al monitor en la otra entrada.
    #[test]
    fn invalid_plan_writes_nothing() {
        let cases: [fn(&mut SwitchPlan); 4] = [
            |p| p.return_codes.clear(),
            |p| p.target_codes.clear(),
            |p| p.return_to = p.target,
            |p| p.return_after = Duration::ZERO,
        ];
        for break_plan in cases {
            let mut bus = bus();
            let mut p = plan();
            break_plan(&mut p);
            let expected = p.validate().unwrap_err();
            let mut waiter = FakeWaiter::default();
            let err = switch_with_return(&mut bus, &p, &mut waiter, &mut |_| {});
            assert_eq!(err, Err(DdcError::InvalidPlan(expected)));
            assert!(bus.writes.is_empty(), "{:?}", bus.writes);
            assert!(waiter.waits.is_empty());
        }
    }

    #[test]
    fn plan_validation() {
        assert_eq!(plan().validate(), Ok(()));
        let mut p = plan();
        p.return_after = Duration::ZERO;
        assert!(matches!(
            p.validate(),
            Err(PlanError::ReturnAfterOutOfRange(_))
        ));
        p.return_after = MAX_RETURN_AFTER + Duration::from_secs(1);
        assert!(p.validate().is_err());
        let mut p = plan();
        p.target_codes.clear();
        assert_eq!(p.validate(), Err(PlanError::NoTargetCodes));
        let mut p = plan();
        p.return_codes.clear();
        assert_eq!(p.validate(), Err(PlanError::NoReturnCodes));
        for input in [Input::Hdmi, Input::DisplayPort] {
            let mut p = plan();
            p.target = input;
            p.return_to = input;
            assert_eq!(p.validate(), Err(PlanError::SameInput(input)));
        }
    }

    #[test]
    fn known_profile_matches_lc34g55t_by_edid_name() {
        let info = |name: &str| MonitorInfo {
            index: 0,
            description: "Monitor PnP genérico".into(),
            device: String::new(),
            edid: Some(edid::parse(&edid::tests::sample(name, "S")).unwrap()),
        };
        let (name, codes) = known_profile(&info("LC34G55T")).unwrap();
        assert_eq!(name, "LC34G55T");
        assert_eq!(codes.for_input(Input::Hdmi), &[0x06]);
        assert_eq!(codes.for_input(Input::DisplayPort), &[0x0F, 0x09]);
        assert!(known_profile(&info("U2720Q")).is_none());
        let mut no_edid = info("LC34G55T");
        no_edid.edid = None;
        assert!(known_profile(&no_edid).is_none());
        assert_eq!(mccs_standard().for_input(Input::Hdmi), &[0x11]);
    }

    #[test]
    fn sleep_waiter_honours_interrupt_flag() {
        static FLAG: AtomicBool = AtomicBool::new(true);
        let mut w = SleepWaiter::new(Some(&FLAG));
        // Interrumpible y con la bandera puesta: vuelve sin dormir.
        assert!(w.wait(Duration::from_secs(60), true));
        // No interrumpible: espera completa y no reporta interrupción.
        assert!(!w.wait(Duration::ZERO, false));
        assert!(!SleepWaiter::new(None).wait(Duration::ZERO, true));
    }

    proptest! {
        /// Invariante: con cualquier patrón de fallos e interrupción, si el monitor existe
        /// cada código de vuelta se intenta, la última escritura es de vuelta y `returned`
        /// refleja si alguna se aceptó.
        #[test]
        fn always_attempts_return(
            script in proptest::collection::vec(any::<bool>(), 0..12),
            target in proptest::collection::vec(any::<u8>(), 1..3),
            returns in proptest::collection::vec(any::<u8>(), 1..3),
            interrupt_at in proptest::option::of(0usize..4),
        ) {
            let mut bus = bus();
            bus.write_script = script.into();
            let mut p = plan();
            p.target_codes = target;
            p.return_codes = returns.clone();
            prop_assert_ne!(p.target, p.return_to);
            prop_assert_eq!(p.validate(), Ok(()));
            let mut waiter = FakeWaiter { interrupt_at, ..Default::default() };
            let mut sent = Vec::new();
            let report = switch_with_return(&mut bus, &p, &mut waiter, &mut |e| {
                if let SwitchEvent::Sent { phase, code, attempt, result } = e {
                    sent.push((*phase, *code, *attempt, result.is_ok()));
                }
            })
            .unwrap();
            prop_assert_eq!(sent.len(), bus.writes.len());
            prop_assert_eq!(sent.last().map(|s| s.0), Some(Phase::Return));
            let back: Vec<_> = sent.iter().filter(|s| s.0 == Phase::Return).collect();
            // Cada código de vuelta, en orden: reintentos hasta el primer éxito o el tope.
            let mut i = 0;
            for code in &returns {
                let mut attempts = 0;
                while i < back.len() && back[i].1 == *code && back[i].2 == attempts + 1 {
                    attempts += 1;
                    i += 1;
                    if back[i - 1].3 {
                        break;
                    }
                }
                prop_assert!((1..=RETURN_ATTEMPTS).contains(&attempts));
            }
            prop_assert_eq!(i, back.len());
            prop_assert_eq!(report.returned, back.iter().any(|s| s.3));
            // Las esperas no interrumpibles son solo pausas cortas de la vuelta.
            for (d, interruptible) in &waiter.waits {
                if !interruptible {
                    prop_assert!(*d <= CODE_GAP);
                }
            }
        }
    }
}
