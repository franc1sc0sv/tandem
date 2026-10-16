//! Antirrebote: un valor nuevo cuenta solo si se sostiene un tiempo.

use std::time::{Duration, Instant};

/// Valor estable de una señal ruidosa. Un cambio se acepta cuando la señal lleva
/// `delay` seguido con el valor nuevo; si vuelve al estable antes, se olvida.
/// No tiene reloj propio: quien lo usa pasa el instante de cada observación.
#[derive(Debug, Clone)]
pub struct Debouncer<T> {
    stable: T,
    /// Valor distinto del estable y desde cuándo se ve sin interrupción.
    pending: Option<(T, Instant)>,
    delay: Duration,
}

impl<T: PartialEq + Clone> Debouncer<T> {
    pub fn new(initial: T, delay: Duration) -> Self {
        Debouncer {
            stable: initial,
            pending: None,
            delay,
        }
    }

    pub fn stable(&self) -> &T {
        &self.stable
    }

    pub fn delay(&self) -> Duration {
        self.delay
    }

    /// Registra lo que se ve en `now`. Devuelve el valor estable nuevo si cambió.
    pub fn observe(&mut self, value: T, now: Instant) -> Option<T> {
        if value == self.stable {
            self.pending = None;
            return None;
        }
        let since = match &self.pending {
            Some((pending, since)) if *pending == value => *since,
            _ => {
                self.pending = Some((value.clone(), now));
                now
            }
        };
        if now.saturating_duration_since(since) >= self.delay {
            self.pending = None;
            self.stable = value.clone();
            return Some(value);
        }
        None
    }

    /// Cuándo se aceptaría el cambio pendiente si nada más cambia.
    pub fn deadline(&self) -> Option<Instant> {
        self.pending.as_ref().map(|(_, since)| *since + self.delay)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const DELAY: Duration = Duration::from_secs(3);

    fn secs(t0: Instant, s: f64) -> Instant {
        t0 + Duration::from_secs_f64(s)
    }

    #[test]
    fn accepts_change_held_for_the_delay() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(false, DELAY);
        assert_eq!(d.observe(true, t0), None);
        assert_eq!(d.deadline(), Some(secs(t0, 3.0)));
        assert_eq!(d.observe(true, secs(t0, 2.9)), None);
        assert!(!d.stable());
        assert_eq!(d.observe(true, secs(t0, 3.0)), Some(true));
        assert!(d.stable());
        assert_eq!(d.deadline(), None);
        assert_eq!(d.observe(true, secs(t0, 10.0)), None);
    }

    #[test]
    fn brief_glitch_is_ignored_and_restarts_the_count() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(true, DELAY);
        // Desconexión de 2 s: no cuenta.
        assert_eq!(d.observe(false, t0), None);
        assert_eq!(d.observe(false, secs(t0, 2.0)), None);
        assert_eq!(d.observe(true, secs(t0, 2.5)), None);
        assert_eq!(d.deadline(), None);
        // La cuenta empieza de cero con la siguiente desconexión.
        assert_eq!(d.observe(false, secs(t0, 4.0)), None);
        assert_eq!(d.observe(false, secs(t0, 6.9)), None);
        assert_eq!(d.observe(false, secs(t0, 7.0)), Some(false));
    }

    #[test]
    fn switching_pending_value_restarts_the_count() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(0u8, DELAY);
        assert_eq!(d.observe(1, t0), None);
        assert_eq!(d.observe(2, secs(t0, 2.0)), None);
        assert_eq!(d.observe(2, secs(t0, 4.0)), None);
        assert_eq!(d.observe(2, secs(t0, 5.0)), Some(2));
    }

    #[test]
    fn zero_delay_follows_the_signal() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(false, Duration::ZERO);
        assert_eq!(d.observe(true, t0), Some(true));
        assert_eq!(d.observe(false, t0), Some(false));
    }

    proptest! {
        /// Cada cambio del valor estable lo justifica una racha del valor nuevo de
        /// al menos `delay`, sin interrupciones, que termina en esa observación.
        #[test]
        fn every_change_is_backed_by_a_long_enough_run(
            samples in proptest::collection::vec((any::<bool>(), 0u64..2_000), 1..80),
        ) {
            let t0 = Instant::now();
            let mut d = Debouncer::new(false, DELAY);
            let mut now = t0;
            let mut history: Vec<(bool, Instant)> = Vec::new();
            for (value, step_ms) in samples {
                now += Duration::from_millis(step_ms);
                history.push((value, now));
                let before = *d.stable();
                if let Some(new) = d.observe(value, now) {
                    prop_assert_eq!(new, value);
                    prop_assert_ne!(new, before);
                    // Inicio de la racha: la primera muestra seguida con `value`.
                    let start = history
                        .iter()
                        .rev()
                        .take_while(|(v, _)| *v == value)
                        .last()
                        .map(|(_, t)| *t)
                        .unwrap();
                    prop_assert!(now.duration_since(start) >= DELAY);
                } else {
                    prop_assert_eq!(*d.stable(), before);
                }
            }
        }
    }
}
