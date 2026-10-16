//! Watchdog de los hooks.
//!
//! Windows quita un hook de bajo nivel sin avisar si una llamada tarda más que
//! `LowLevelHooksTimeout`. Mientras llegan eventos, el hook está vivo; tras un rato
//! sin nada, se manda una sonda que solo nuestro hook reconoce. Si no llega a
//! tiempo, se reinstalan los dos hooks.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookKind {
    Keyboard,
    Mouse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchdogConfig {
    /// Silencio de un hook tras el que se lo prueba.
    pub idle_before_probe: Duration,
    /// Cuánto se espera la sonda antes de darlo por perdido.
    pub probe_timeout: Duration,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            idle_before_probe: Duration::from_secs(3),
            probe_timeout: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogAction {
    Idle,
    Probe { keyboard: bool, mouse: bool },
    Reinstall,
}

#[derive(Debug, Clone, Copy)]
struct Liveness {
    last_seen: Instant,
    probe_sent: Option<Instant>,
}

impl Liveness {
    fn new(now: Instant) -> Self {
        Self {
            last_seen: now,
            probe_sent: None,
        }
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
    pub fn new(now: Instant, config: WatchdogConfig) -> Self {
        Self {
            config,
            keyboard: Liveness::new(now),
            mouse: Liveness::new(now),
            reinstalls: 0,
        }
    }

    /// Llegó algo de este hook (un evento o la sonda).
    pub fn activity(&mut self, kind: HookKind, now: Instant) {
        let l = match kind {
            HookKind::Keyboard => &mut self.keyboard,
            HookKind::Mouse => &mut self.mouse,
        };
        l.last_seen = now;
        l.probe_sent = None;
    }

    pub fn reinstalls(&self) -> u32 {
        self.reinstalls
    }

    pub fn tick(&mut self, now: Instant) -> WatchdogAction {
        let timeout = self.config.probe_timeout;
        let lost = |l: &Liveness| {
            l.probe_sent
                .is_some_and(|sent| now.saturating_duration_since(sent) >= timeout)
        };
        if lost(&self.keyboard) || lost(&self.mouse) {
            // Se reinstalan los dos; el plazo vuelve a contar desde ahora.
            self.keyboard = Liveness::new(now);
            self.mouse = Liveness::new(now);
            self.reinstalls += 1;
            return WatchdogAction::Reinstall;
        }
        let idle = self.config.idle_before_probe;
        let probe = |l: &mut Liveness| {
            let due = l.probe_sent.is_none() && now.saturating_duration_since(l.last_seen) >= idle;
            if due {
                l.probe_sent = Some(now);
            }
            due
        };
        let keyboard = probe(&mut self.keyboard);
        let mouse = probe(&mut self.mouse);
        if keyboard || mouse {
            WatchdogAction::Probe { keyboard, mouse }
        } else {
            WatchdogAction::Idle
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const S: Duration = Duration::from_secs(1);

    fn cfg() -> WatchdogConfig {
        WatchdogConfig {
            idle_before_probe: 3 * S,
            probe_timeout: S,
        }
    }

    #[test]
    fn con_actividad_no_prueba_nada() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(t0, cfg());
        for i in 1..20 {
            let now = t0 + i * S;
            w.activity(HookKind::Keyboard, now);
            w.activity(HookKind::Mouse, now);
            assert_eq!(w.tick(now), WatchdogAction::Idle);
        }
    }

    #[test]
    fn prueba_tras_el_silencio_y_sigue_si_contesta() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(t0, cfg());
        assert_eq!(w.tick(t0 + 2 * S), WatchdogAction::Idle);
        assert_eq!(
            w.tick(t0 + 3 * S),
            WatchdogAction::Probe {
                keyboard: true,
                mouse: true
            }
        );
        // Mientras espera, no repite la sonda.
        assert_eq!(w.tick(t0 + 3 * S), WatchdogAction::Idle);
        w.activity(HookKind::Keyboard, t0 + 3 * S);
        w.activity(HookKind::Mouse, t0 + 3 * S);
        assert_eq!(w.tick(t0 + 5 * S), WatchdogAction::Idle);
        assert_eq!(
            w.tick(t0 + 6 * S),
            WatchdogAction::Probe {
                keyboard: true,
                mouse: true
            }
        );
        assert_eq!(w.reinstalls(), 0);
    }

    #[test]
    fn reinstala_si_un_hook_no_contesta() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(t0, cfg());
        // El mouse se usa, el teclado no.
        w.activity(HookKind::Mouse, t0 + 3 * S);
        assert_eq!(
            w.tick(t0 + 3 * S),
            WatchdogAction::Probe {
                keyboard: true,
                mouse: false
            }
        );
        w.activity(HookKind::Mouse, t0 + 4 * S);
        assert_eq!(w.tick(t0 + 4 * S), WatchdogAction::Reinstall);
        assert_eq!(w.reinstalls(), 1);
        // Tras reinstalar, el plazo empieza de nuevo.
        assert_eq!(w.tick(t0 + 6 * S), WatchdogAction::Idle);
        assert_eq!(
            w.tick(t0 + 7 * S),
            WatchdogAction::Probe {
                keyboard: true,
                mouse: true
            }
        );
    }

    proptest! {
        /// Nunca se reinstala si cada sonda recibe respuesta antes del plazo.
        #[test]
        fn sondas_contestadas_nunca_reinstalan(steps in prop::collection::vec((0u64..5000, any::<bool>(), any::<bool>()), 1..60)) {
            let t0 = Instant::now();
            let mut w = Watchdog::new(t0, cfg());
            let mut now = t0;
            for (ms, kb, mouse) in steps {
                now += Duration::from_millis(ms.min(999));
                if kb { w.activity(HookKind::Keyboard, now); }
                if mouse { w.activity(HookKind::Mouse, now); }
                match w.tick(now) {
                    WatchdogAction::Reinstall => {
                        prop_assert!(false, "reinstaló con hooks que contestan");
                    }
                    WatchdogAction::Probe { keyboard, mouse } => {
                        // El sistema contesta enseguida.
                        if keyboard { w.activity(HookKind::Keyboard, now); }
                        if mouse { w.activity(HookKind::Mouse, now); }
                    }
                    WatchdogAction::Idle => {}
                }
            }
        }

        /// Un hook muerto se detecta siempre a más tardar en silencio + plazo.
        #[test]
        fn hook_muerto_se_detecta_a_tiempo(tick_ms in 10u64..500) {
            let t0 = Instant::now();
            let c = cfg();
            let mut w = Watchdog::new(t0, c);
            let limit = c.idle_before_probe + c.probe_timeout + Duration::from_millis(tick_ms);
            let mut now = t0;
            loop {
                now += Duration::from_millis(tick_ms);
                // El mouse sigue vivo; el teclado ya no contesta.
                w.activity(HookKind::Mouse, now);
                if w.tick(now) == WatchdogAction::Reinstall {
                    break;
                }
                prop_assert!(now - t0 <= limit);
            }
        }
    }
}
