//! Watchdog de los hooks.
//!
//! Windows quita un hook de bajo nivel sin avisar si una llamada tarda más que
//! `LowLevelHooksTimeout`. El watchdog es pasivo: nunca inyecta nada, porque la
//! entrada inyectada reinicia el contador de inactividad de Windows y la PC no se
//! bloquearía ni se suspendería nunca. La referencia es Raw Input de dispositivos
//! reales, que no depende de los hooks: si Raw Input ve teclado (o mouse) real y
//! el hook de ese tipo no ve nada durante `hook_timeout`, el hook se perdió y se
//! reinstalan los dos. Sin entrada real no hay nada que comprobar.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookKind {
    Keyboard,
    Mouse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchdogConfig {
    /// Cuánto puede haber entrada real sin que el hook la vea antes de darlo por
    /// perdido. También es la tolerancia entre el hook y Raw Input del mismo
    /// evento, que no llegan en un orden fijo.
    pub hook_timeout: Duration,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            hook_timeout: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogAction {
    Idle,
    Reinstall,
}

#[derive(Debug, Clone, Copy, Default)]
struct Liveness {
    /// Último evento que vio el hook.
    hook_seen: Option<Instant>,
    /// Primera entrada real que el hook todavía no confirmó.
    unanswered: Option<Instant>,
}

impl Liveness {
    fn hook(&mut self, now: Instant) {
        self.hook_seen = Some(now);
        self.unanswered = None;
    }

    fn raw(&mut self, now: Instant, timeout: Duration) {
        // El evento del hook pudo llegar justo antes que el de Raw Input.
        let recent = self
            .hook_seen
            .is_some_and(|seen| now.saturating_duration_since(seen) <= timeout);
        if !recent && self.unanswered.is_none() {
            self.unanswered = Some(now);
        }
    }

    fn lost(&self, now: Instant, timeout: Duration) -> bool {
        self.unanswered
            .is_some_and(|since| now.saturating_duration_since(since) >= timeout)
    }
}

#[derive(Debug)]
pub struct Watchdog {
    config: WatchdogConfig,
    keyboard: Liveness,
    mouse: Liveness,
    reinstalls: u32,
}

impl Watchdog {
    pub fn new(config: WatchdogConfig) -> Self {
        Self {
            config,
            keyboard: Liveness::default(),
            mouse: Liveness::default(),
            reinstalls: 0,
        }
    }

    fn liveness(&mut self, kind: HookKind) -> &mut Liveness {
        match kind {
            HookKind::Keyboard => &mut self.keyboard,
            HookKind::Mouse => &mut self.mouse,
        }
    }

    /// El hook de este tipo vio un evento: está instalado.
    pub fn hook_activity(&mut self, kind: HookKind, now: Instant) {
        self.liveness(kind).hook(now);
    }

    /// Raw Input vio entrada real (`hDevice != 0`) de este tipo.
    pub fn real_activity(&mut self, kind: HookKind, now: Instant) {
        let timeout = self.config.hook_timeout;
        self.liveness(kind).raw(now, timeout);
    }

    pub fn reinstalls(&self) -> u32 {
        self.reinstalls
    }

    pub fn tick(&mut self, now: Instant) -> WatchdogAction {
        let timeout = self.config.hook_timeout;
        if self.keyboard.lost(now, timeout) || self.mouse.lost(now, timeout) {
            // Se reinstalan los dos; la entrada que siga se compara con los nuevos.
            self.keyboard = Liveness::default();
            self.mouse = Liveness::default();
            self.reinstalls += 1;
            return WatchdogAction::Reinstall;
        }
        WatchdogAction::Idle
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const S: Duration = Duration::from_secs(1);
    const MS: Duration = Duration::from_millis(1);

    fn cfg() -> WatchdogConfig {
        WatchdogConfig { hook_timeout: S }
    }

    #[test]
    fn inactivo_durante_minutos_no_hace_nada() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(cfg());
        // Diez minutos de ticks cada 100 ms sin ningún evento.
        for i in 0..6000u32 {
            assert_eq!(w.tick(t0 + i * 100 * MS), WatchdogAction::Idle);
        }
        assert_eq!(w.reinstalls(), 0);
    }

    #[test]
    fn hooks_sin_raw_input_no_hacen_nada() {
        // Un hook que ve eventos sin Raw Input (p. ej. inyectados) nunca es motivo
        // para actuar.
        let t0 = Instant::now();
        let mut w = Watchdog::new(cfg());
        for i in 0..100u32 {
            let now = t0 + i * S;
            w.hook_activity(HookKind::Mouse, now);
            assert_eq!(w.tick(now), WatchdogAction::Idle);
        }
    }

    #[test]
    fn raw_input_de_teclado_sin_hook_reinstala() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(cfg());
        // El mouse sigue vivo; del teclado solo llega Raw Input.
        for i in 0..10u32 {
            let now = t0 + i * 100 * MS;
            w.hook_activity(HookKind::Mouse, now);
            w.real_activity(HookKind::Mouse, now);
            w.real_activity(HookKind::Keyboard, now);
            assert_eq!(w.tick(now), WatchdogAction::Idle);
        }
        assert_eq!(w.tick(t0 + S), WatchdogAction::Reinstall);
        assert_eq!(w.reinstalls(), 1);
        // Tras reinstalar, el plazo empieza de nuevo y el hook nuevo contesta.
        assert_eq!(w.tick(t0 + S + 500 * MS), WatchdogAction::Idle);
        w.real_activity(HookKind::Keyboard, t0 + 2 * S);
        w.hook_activity(HookKind::Keyboard, t0 + 2 * S + 5 * MS);
        assert_eq!(w.tick(t0 + 4 * S), WatchdogAction::Idle);
        assert_eq!(w.reinstalls(), 1);
    }

    #[test]
    fn raw_input_de_mouse_sin_hook_reinstala() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(cfg());
        w.real_activity(HookKind::Mouse, t0);
        assert_eq!(w.tick(t0 + 999 * MS), WatchdogAction::Idle);
        assert_eq!(w.tick(t0 + S), WatchdogAction::Reinstall);
    }

    #[test]
    fn el_hook_puede_llegar_antes_o_despues_que_raw_input() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(cfg());
        // Hook primero.
        w.hook_activity(HookKind::Keyboard, t0);
        w.real_activity(HookKind::Keyboard, t0 + 2 * MS);
        // Raw primero.
        w.real_activity(HookKind::Mouse, t0);
        w.hook_activity(HookKind::Mouse, t0 + 2 * MS);
        for i in 0..50u32 {
            assert_eq!(w.tick(t0 + i * 100 * MS), WatchdogAction::Idle);
        }
    }

    #[test]
    fn hook_que_murio_tras_su_ultimo_evento_se_detecta() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(cfg());
        w.hook_activity(HookKind::Keyboard, t0);
        w.real_activity(HookKind::Keyboard, t0);
        // El hook se pierde; la entrada real sigue un rato después.
        let later = t0 + 5 * S;
        w.real_activity(HookKind::Keyboard, later);
        assert_eq!(w.tick(later + 500 * MS), WatchdogAction::Idle);
        assert_eq!(w.tick(later + S), WatchdogAction::Reinstall);
    }

    proptest! {
        /// Con hooks vivos (cada entrada real también llega al hook, antes o después
        /// y con un desfase menor que el plazo), nunca se reinstala.
        #[test]
        fn con_hooks_vivos_nunca_reinstala(
            steps in prop::collection::vec(
                (0u64..5000, any::<bool>(), any::<bool>(), 0u64..900, 0u64..1200),
                1..80,
            )
        ) {
            let t0 = Instant::now();
            let mut w = Watchdog::new(cfg());
            let mut now = t0;
            for (gap, keyboard, raw_first, skew, tick_after) in steps {
                now += Duration::from_millis(gap);
                let kind = if keyboard { HookKind::Keyboard } else { HookKind::Mouse };
                let first = now;
                let second = now + Duration::from_millis(skew);
                if raw_first {
                    w.real_activity(kind, first);
                    // Un tick entre los dos todavía está dentro del plazo.
                    prop_assert_eq!(w.tick(first + Duration::from_millis(skew / 2)), WatchdogAction::Idle);
                    w.hook_activity(kind, second);
                } else {
                    w.hook_activity(kind, first);
                    w.real_activity(kind, second);
                }
                now = second;
                prop_assert_eq!(w.tick(now + Duration::from_millis(tick_after)), WatchdogAction::Idle);
            }
            prop_assert_eq!(w.reinstalls(), 0);
        }

        /// Sin ningún evento, por largo que sea, nunca se actúa.
        #[test]
        fn sin_eventos_nunca_actua(ticks in prop::collection::vec(0u64..600_000, 1..50)) {
            let t0 = Instant::now();
            let mut w = Watchdog::new(cfg());
            for ms in ticks {
                prop_assert_eq!(w.tick(t0 + Duration::from_millis(ms)), WatchdogAction::Idle);
            }
        }

        /// Un hook muerto se detecta a más tardar un plazo después de que empiece
        /// la entrada real.
        #[test]
        fn hook_muerto_se_detecta_a_tiempo(tick_ms in 10u64..500) {
            let t0 = Instant::now();
            let c = cfg();
            let mut w = Watchdog::new(c);
            let limit = c.hook_timeout + Duration::from_millis(tick_ms);
            let mut now = t0;
            loop {
                now += Duration::from_millis(tick_ms);
                // El mouse sigue vivo; el hook de teclado ya no ve nada.
                w.hook_activity(HookKind::Mouse, now);
                w.real_activity(HookKind::Mouse, now);
                w.real_activity(HookKind::Keyboard, now);
                if w.tick(now) == WatchdogAction::Reinstall {
                    break;
                }
                prop_assert!(now - t0 <= limit);
            }
        }
    }
}
