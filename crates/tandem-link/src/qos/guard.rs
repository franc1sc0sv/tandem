//! Pausa de los datos cuando la entrada se atrasa (DESIGN §6).
//!
//! Lógica pura: las muestras de latencia de la entrada y el tiempo llegan como argumentos.
//! El p99 se calcula sobre una ventana deslizante; si pasa el objetivo durante
//! [`GuardConfig::trip`] seguidos, los datos se pausan. Vuelven cuando el p99 lleva
//! [`GuardConfig::hold`] por debajo de una fracción del objetivo (histéresis: sin ella,
//! reanudar haría subir la latencia otra vez y los datos irían a los tirones). Sin entrada
//! no hay nada que proteger: una ventana vacía cuenta como latencia baja.

use std::collections::VecDeque;
use std::time::Duration;

use tokio::time::Instant;

/// Tiempos y umbrales de la pausa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuardConfig {
    /// Ventana del p99.
    pub window: Duration,
    /// Cuánto tiene que estar el p99 por encima del objetivo para pausar (T-101: 2 s).
    pub trip: Duration,
    /// Cuánto tiene que estar por debajo de `resume_percent` del objetivo para reanudar.
    pub hold: Duration,
    /// Por debajo de este % del objetivo se considera que la latencia bajó.
    pub resume_percent: u32,
}

impl Default for GuardConfig {
    fn default() -> Self {
        GuardConfig {
            window: Duration::from_secs(1),
            trip: Duration::from_secs(2),
            hold: Duration::from_secs(1),
            resume_percent: 75,
        }
    }
}

/// Un cambio de estado de la pausa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// El p99 pasó el objetivo durante `trip`.
    Pause { p99: Duration },
    /// El p99 volvió a bajar. `p99` es `None` si no hubo entrada en la ventana.
    Resume { p99: Option<Duration> },
}

#[derive(Debug, Clone)]
pub struct LatencyGuard {
    config: GuardConfig,
    target: Duration,
    samples: VecDeque<(Instant, Duration)>,
    paused: bool,
    /// Desde cuándo el p99 está del lado que lleva a cambiar de estado.
    since: Option<Instant>,
    /// El p99 de la última evaluación.
    last_p99: Option<Duration>,
}

impl LatencyGuard {
    pub fn new(config: GuardConfig, target: Duration) -> Self {
        LatencyGuard {
            config,
            target,
            samples: VecDeque::new(),
            paused: false,
            since: None,
            last_p99: None,
        }
    }

    /// El p99 que vio la última evaluación (`None` sin entrada en la ventana).
    pub fn last_p99(&self) -> Option<Duration> {
        self.last_p99
    }

    pub fn target(&self) -> Duration {
        self.target
    }

    pub fn window(&self) -> Duration {
        self.config.window
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Otro objetivo (cambió el camino): lo medido por el anterior ya no dice nada.
    /// Devuelve [`Transition::Resume`] si estaba en pausa.
    pub fn reset(&mut self, target: Duration) -> Option<Transition> {
        self.target = target;
        self.samples.clear();
        self.since = None;
        self.last_p99 = None;
        std::mem::take(&mut self.paused).then_some(Transition::Resume { p99: None })
    }

    /// Una latencia medida de la entrada.
    pub fn record(&mut self, now: Instant, latency: Duration) {
        self.samples.push_back((now, latency));
    }

    /// El p99 de la ventana que termina en `now`; `None` sin muestras.
    pub fn p99(&mut self, now: Instant) -> Option<Duration> {
        self.forget_before(now);
        if self.samples.is_empty() {
            return None;
        }
        let mut lat: Vec<Duration> = self.samples.iter().map(|(_, l)| *l).collect();
        // El menor valor que deja al 99 % de las muestras en o por debajo.
        let rank = (lat.len() * 99).div_ceil(100).max(1) - 1;
        let (_, p99, _) = lat.select_nth_unstable(rank);
        Some(*p99)
    }

    fn forget_before(&mut self, now: Instant) {
        while let Some((t, _)) = self.samples.front() {
            if now.saturating_duration_since(*t) > self.config.window {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// Revisa el estado con la ventana que termina en `now`.
    pub fn evaluate(&mut self, now: Instant) -> Option<Transition> {
        let p99 = self.p99(now);
        self.last_p99 = p99;
        if self.paused {
            let threshold = self.target * self.config.resume_percent / 100;
            if p99.is_some_and(|p| p > threshold) {
                self.since = None;
                return None;
            }
            let since = *self.since.get_or_insert(now);
            if now.saturating_duration_since(since) < self.config.hold {
                return None;
            }
            self.paused = false;
            self.since = None;
            Some(Transition::Resume { p99 })
        } else {
            let Some(p99) = p99.filter(|p| *p > self.target) else {
                self.since = None;
                return None;
            };
            let since = *self.since.get_or_insert(now);
            if now.saturating_duration_since(since) < self.config.trip {
                return None;
            }
            self.paused = true;
            self.since = None;
            Some(Transition::Pause { p99 })
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const TARGET: Duration = Duration::from_millis(20);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Una muestra por milisegundo con la latencia que diga `lat`, evaluando cada 10 ms.
    fn run(
        g: &mut LatencyGuard,
        t0: Instant,
        from_ms: u64,
        to_ms: u64,
        lat: impl Fn(u64) -> Duration,
    ) -> Vec<(u64, Transition)> {
        let mut out = Vec::new();
        for t in from_ms..to_ms {
            let now = t0 + ms(t);
            g.record(now, lat(t));
            if t % 10 == 0
                && let Some(tr) = g.evaluate(now)
            {
                out.push((t, tr));
            }
        }
        out
    }

    #[test]
    fn el_p99_es_el_del_uno_por_ciento_más_lento() {
        let t0 = Instant::now();
        let mut g = LatencyGuard::new(GuardConfig::default(), TARGET);
        assert_eq!(g.p99(t0), None);
        for i in 0..100u64 {
            g.record(t0, ms(i + 1));
        }
        assert_eq!(g.p99(t0), Some(ms(99)));
        g.record(t0, ms(1000));
        assert_eq!(g.p99(t0), Some(ms(100)));
        // Fuera de la ventana no cuenta.
        assert_eq!(g.p99(t0 + ms(1001)), None);
    }

    #[test]
    fn pausa_tras_2_s_por_encima_y_vuelve_con_histéresis() {
        let t0 = Instant::now();
        let mut g = LatencyGuard::new(GuardConfig::default(), TARGET);
        assert!(run(&mut g, t0, 0, 3000, |_| ms(5)).is_empty());
        // Desde 3 s la latencia es de 30 ms. El p99 pasa el objetivo cuando las lentas
        // son más del 1 % de la ventana (11 de 1001, a los 3010 ms), y 2 s después, pausa.
        let out = run(&mut g, t0, 3000, 6000, |_| ms(30));
        assert_eq!(out, vec![(5010, Transition::Pause { p99: ms(30) })]);
        assert!(g.is_paused());
        // A 18 ms está bajo el objetivo pero no bajo el 75 %: sigue en pausa.
        assert!(run(&mut g, t0, 6000, 9000, |_| ms(18)).is_empty());
        // A 10 ms: el p99 baja cuando quedan 10 lentas en la ventana (a los 9990 ms), y
        // hace falta 1 s más.
        let out = run(&mut g, t0, 9000, 12000, |_| ms(10));
        assert_eq!(out, vec![(10990, Transition::Resume { p99: Some(ms(10)) })]);
    }

    #[test]
    fn un_pico_corto_no_pausa() {
        let t0 = Instant::now();
        let mut g = LatencyGuard::new(GuardConfig::default(), TARGET);
        // 1,5 s malos en medio de buenos: el p99 sube y baja antes de los 2 s.
        let out = run(&mut g, t0, 0, 6000, |t| {
            if (2000..2500).contains(&t) {
                ms(80)
            } else {
                ms(3)
            }
        });
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn sin_entrada_la_pausa_termina_sola() {
        let t0 = Instant::now();
        let mut g = LatencyGuard::new(GuardConfig::default(), TARGET);
        run(&mut g, t0, 0, 2100, |_| ms(50));
        assert!(g.is_paused());
        assert_eq!(g.evaluate(t0 + ms(2500)), None);
        assert_eq!(g.evaluate(t0 + ms(3200)), None);
        assert_eq!(
            g.evaluate(t0 + ms(4200)),
            Some(Transition::Resume { p99: None })
        );
    }

    #[test]
    fn cambiar_de_camino_reanuda_y_olvida_lo_medido() {
        let t0 = Instant::now();
        let mut g = LatencyGuard::new(GuardConfig::default(), TARGET);
        run(&mut g, t0, 0, 2100, |_| ms(50));
        assert!(g.is_paused());
        assert_eq!(g.reset(ms(4)), Some(Transition::Resume { p99: None }));
        assert_eq!(g.target(), ms(4));
        assert_eq!(g.p99(t0 + ms(2100)), None);
        assert_eq!(g.reset(ms(4)), None);
    }

    proptest! {
        /// Con la entrada siempre dentro del objetivo, nunca hay pausa.
        #[test]
        fn dentro_del_objetivo_nunca_pausa(lats in prop::collection::vec(0u64..=20_000, 1..3000)) {
            let t0 = Instant::now();
            let mut g = LatencyGuard::new(GuardConfig::default(), TARGET);
            for (i, us) in lats.iter().enumerate() {
                let now = t0 + ms(i as u64 * 3);
                g.record(now, Duration::from_micros(*us));
                prop_assert_eq!(g.evaluate(now), None);
            }
        }

        /// Si toda la entrada pasa el objetivo, la pausa llega entre los 2 s y los 2 s más
        /// el paso de evaluación, y una sola vez.
        #[test]
        fn por_encima_pausa_a_los_2_s(
            over in 21u64..500,
            step in 1u64..100,
        ) {
            let t0 = Instant::now();
            let mut g = LatencyGuard::new(GuardConfig::default(), TARGET);
            let mut pauses = Vec::new();
            let mut t = 0;
            while t <= 4000 {
                let now = t0 + ms(t);
                g.record(now, ms(over));
                if let Some(tr) = g.evaluate(now) {
                    pauses.push((t, tr));
                }
                t += step;
            }
            prop_assert_eq!(pauses.len(), 1);
            let (at, tr) = pauses[0];
            prop_assert!((2000..2000 + step).contains(&at), "pausa a {} ms", at);
            prop_assert_eq!(tr, Transition::Pause { p99: ms(over) });
        }
    }
}
