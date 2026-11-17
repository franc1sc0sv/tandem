//! Limitador de tasa de los datos de una conexión (cubeta de fichas).
//!
//! Garantía: en cualquier ventana de 1 s no se entregan más bytes que el tope. Por eso la
//! cubeta se llena a `tope − ráfaga` por segundo: lo que puede salir de golpe (la ráfaga)
//! más lo que se repone en un segundo nunca pasa del tope. La cuenta es entera, en
//! nanobytes, para que el redondeo no rompa la garantía.

use std::time::Duration;

use tokio::time::Instant;

/// El tope más bajo que se acepta: con menos, una trama tardaría segundos en salir.
pub const MIN_CAP: u64 = 64 * 1024;

/// La ráfaga más grande. Una ráfaga entra entera a la cola del punto de acceso, delante de
/// la próxima tecla: 16 KiB son 8 ms con un Wi-Fi que da 2 MB/s.
pub const MAX_BURST: u64 = 16 * 1024;

/// Lo menos que se entrega si no alcanza para todo lo pedido: un paquete de QUIC.
pub const MIN_GRANT: u64 = 1200;

const NANOS: u128 = 1_000_000_000;

#[derive(Debug, Clone)]
pub struct RateLimiter {
    cap: u64,
    burst: u64,
    /// Bytes por segundo que se reponen: `cap - burst`.
    refill: u64,
    /// Fichas en nanobytes (bytes × 10⁹).
    tokens: u128,
    last: Instant,
}

impl RateLimiter {
    /// Un limitador con la cubeta llena. `cap` en bytes por segundo; por debajo de
    /// [`MIN_CAP`] vale [`MIN_CAP`].
    pub fn new(cap: u64, now: Instant) -> Self {
        let mut l = RateLimiter {
            cap: 0,
            burst: 0,
            refill: 0,
            tokens: 0,
            last: now,
        };
        l.set_cap(cap, now);
        l.tokens = u128::from(l.burst) * NANOS;
        l
    }

    /// El tope en bytes por segundo.
    pub fn cap(&self) -> u64 {
        self.cap
    }

    /// Lo más que entrega [`Self::take`] de una vez.
    pub fn burst(&self) -> u64 {
        self.burst
    }

    /// Cambia el tope sin regalar fichas: las que sobran del tope anterior se pierden.
    pub fn set_cap(&mut self, cap: u64, now: Instant) {
        self.refill_to(now);
        let cap = cap.max(MIN_CAP);
        let burst = (cap / 100).clamp(1, MAX_BURST);
        self.cap = cap;
        self.burst = burst;
        self.refill = cap - burst;
        self.tokens = self.tokens.min(u128::from(burst) * NANOS);
    }

    fn refill_to(&mut self, now: Instant) {
        if now <= self.last {
            return;
        }
        let dt = now.duration_since(self.last).as_nanos();
        let full = u128::from(self.burst) * NANOS;
        self.tokens = self
            .tokens
            .saturating_add(dt.saturating_mul(u128::from(self.refill)))
            .min(full);
        self.last = now;
    }

    /// Pide hasta `want` bytes. `Ok(n)`: pueden salir `n` (como mucho la ráfaga) ya; si no
    /// alcanza para todo pero sí para un paquete, sale lo que hay. `Err(espera)`: cuánto
    /// falta para un paquete; quien pide espera y vuelve a pedir.
    ///
    /// Que salga lo que hay importa: quien espera se despierta tarde (el reloj de tokio
    /// redondea al milisegundo), y si esperara la ráfaga entera, lo que se juntó de más
    /// rebalsaría la cubeta y se perdería.
    pub fn take(&mut self, now: Instant, want: usize) -> Result<usize, Duration> {
        self.refill_to(now);
        let n = u64::try_from(want).unwrap_or(u64::MAX).min(self.burst);
        if n == 0 {
            return Ok(0);
        }
        let have = u64::try_from(self.tokens / NANOS).unwrap_or(u64::MAX);
        let least = n.min(MIN_GRANT);
        if have >= least {
            let n = n.min(have);
            self.tokens -= u128::from(n) * NANOS;
            // `n` ≤ `want`, que ya era usize.
            return Ok(usize::try_from(n).unwrap_or(want));
        }
        let missing = u128::from(least) * NANOS - self.tokens;
        let ns = missing.div_ceil(u128::from(self.refill));
        Err(Duration::from_nanos(u64::try_from(ns).unwrap_or(u64::MAX)))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const SEC: Duration = Duration::from_secs(1);

    /// Lo más entregado en una ventana [t, t + 1 s] que empiece en una entrega (las
    /// entregas van en orden de tiempo).
    fn worst_window(grants: &[(Duration, usize)]) -> u64 {
        let (mut worst, mut sum, mut end) = (0, 0u64, 0);
        for (start, n) in grants {
            while end < grants.len() && grants[end].0 <= *start + SEC {
                sum += grants[end].1 as u64;
                end += 1;
            }
            worst = worst.max(sum);
            sum -= *n as u64;
        }
        worst
    }

    #[test]
    fn el_tope_se_respeta_y_se_aprovecha() {
        let t0 = Instant::now();
        let mut l = RateLimiter::new(1_000_000, t0);
        assert_eq!(l.burst(), 10_000);
        let mut now = t0;
        let mut grants = Vec::new();
        while now < t0 + Duration::from_secs(5) {
            match l.take(now, 16 * 1024) {
                Ok(n) => grants.push((now - t0, n)),
                Err(wait) => now += wait,
            }
        }
        let total: u64 = grants.iter().map(|(_, n)| *n as u64).sum();
        assert!(worst_window(&grants) <= 1_000_000);
        // Lo que se repone por segundo (99 % del tope) más la ráfaga del principio.
        assert!(total >= 4 * 990_000, "solo salieron {total}");
    }

    #[test]
    fn un_tope_bajo_sube_al_mínimo() {
        let l = RateLimiter::new(10, Instant::now());
        assert_eq!(l.cap(), MIN_CAP);
        assert!(l.burst() > 0);
    }

    #[test]
    fn bajar_el_tope_no_deja_fichas_de_más() {
        let t0 = Instant::now();
        let mut l = RateLimiter::new(10_000_000, t0);
        l.set_cap(MIN_CAP, t0);
        assert_eq!(l.take(t0, usize::MAX), Ok(l.burst() as usize));
        assert!(l.take(t0, 1).is_err());
    }

    #[test]
    fn si_no_alcanza_para_todo_sale_lo_que_hay_desde_un_paquete() {
        let t0 = Instant::now();
        let mut l = RateLimiter::new(1_000_000, t0);
        assert_eq!(l.take(t0, 10_000), Ok(10_000));
        // Se reponen 990 kB/s: en 1 ms, 990 bytes, menos que un paquete.
        let wait = l.take(t0 + Duration::from_millis(1), 10_000).unwrap_err();
        assert!(wait > Duration::ZERO);
        // En 5 ms hay 4950: salen esos, no los 10 000.
        assert_eq!(l.take(t0 + Duration::from_millis(5), 10_000), Ok(4950));
    }

    #[test]
    fn la_espera_alcanza_justo() {
        let t0 = Instant::now();
        let mut l = RateLimiter::new(1_000_000, t0);
        assert_eq!(l.take(t0, 10_000), Ok(10_000));
        let wait = l.take(t0, 1000).unwrap_err();
        assert!(l.take(t0 + wait - Duration::from_nanos(1), 1000).is_err());
        assert_eq!(l.take(t0 + wait, 1000), Ok(1000));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Pedidos de cualquier tamaño en cualquier momento: nunca más que el tope en una
        /// ventana de 1 s.
        #[test]
        fn nunca_supera_el_tope_en_una_ventana_de_1_s(
            cap in MIN_CAP..50_000_000u64,
            steps in prop::collection::vec((0u64..400_000, 1usize..300_000), 1..400),
        ) {
            let t0 = Instant::now();
            let mut l = RateLimiter::new(cap, t0);
            let mut at = Duration::ZERO;
            let mut grants = Vec::new();
            for (dt_us, want) in steps {
                at += Duration::from_micros(dt_us);
                if let Ok(n) = l.take(t0 + at, want) {
                    prop_assert!(n <= want && n as u64 <= l.burst());
                    grants.push((at, n));
                }
            }
            prop_assert!(worst_window(&grants) <= cap);
        }

        /// Quien espera lo que dice el limitador y vuelve a pedir tampoco lo supera, y
        /// aprovecha lo que se repone.
        #[test]
        fn quien_espera_lo_justo_no_lo_supera_y_lo_aprovecha(
            cap in MIN_CAP..20_000_000u64,
            want in 4096usize..200_000,
        ) {
            let t0 = Instant::now();
            let mut l = RateLimiter::new(cap, t0);
            let mut now = t0;
            let mut grants = Vec::new();
            let end = t0 + Duration::from_secs(3);
            while now < end {
                match l.take(now, want) {
                    Ok(n) => grants.push((now - t0, n)),
                    Err(wait) => now += wait,
                }
            }
            prop_assert!(worst_window(&grants) <= cap);
            let total: u64 = grants.iter().map(|(_, n)| *n as u64).sum();
            let refill = cap - l.burst();
            // Tres segundos de reposición, menos lo que pueda quedar a medio juntar.
            prop_assert!(total + l.burst() >= 3 * refill, "{} < {}", total, 3 * refill);
        }
    }
}
