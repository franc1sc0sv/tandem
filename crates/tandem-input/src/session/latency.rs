//! Medición de la latencia de la entrada (T-093, DESIGN §7.1).
//!
//! El servidor marca cada datagrama de movimiento y cada tecla o clic con su reloj
//! monotónico ([`now_us`]). El cliente pasa esa marca a su propio reloj con el desfase
//! que estiman los latidos ([`Beats`], como NTP: ida y vuelta / 2, quedándose con la
//! muestra de menor ida y vuelta) y guarda la diferencia con el momento de inyectar en
//! un [`Histogram`]. Cada 5 s manda p50/p99/máx al servidor y los publica.

use std::collections::VecDeque;
use std::sync::OnceLock;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tandem_proto::InputLatency;

/// Reloj monotónico del proceso, en microsegundos desde la primera lectura. Cada
/// máquina tiene el suyo: entre las dos solo vale con el desfase de [`ClockSync`].
pub fn now_us() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    let start = *START.get_or_init(Instant::now);
    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// Bits de la mantisa: 32 cubetas por potencia de dos, error relativo ≤ 1/32.
const SUB_BITS: u32 = 5;
const SUB: u64 = 1 << SUB_BITS;
/// Hasta `2 * SUB` cada valor tiene su cubeta.
const LINEAR: u64 = 2 * SUB;
/// Lo que entra en `u32` microsegundos (más de una hora) alcanza de sobra.
const BUCKETS: usize = (LINEAR + (32 - SUB_BITS as u64 - 1) * SUB) as usize;

/// Histograma log-lineal de microsegundos: memoria fija y registrar es O(1), así no
/// pesa en el camino caliente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histogram {
    buckets: Vec<u32>,
    count: u32,
    max: u32,
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

impl Histogram {
    pub fn new() -> Self {
        Self {
            buckets: vec![0; BUCKETS],
            count: 0,
            max: 0,
        }
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn record(&mut self, us: u64) {
        let v = u32::try_from(us).unwrap_or(u32::MAX);
        let i = index(v);
        self.buckets[i] = self.buckets[i].saturating_add(1);
        self.count = self.count.saturating_add(1);
        self.max = self.max.max(v);
    }

    /// El valor bajo el cual queda la fracción `q` de lo registrado (por arriba: el
    /// límite superior de su cubeta, sin pasar del máximo). `None` sin datos.
    pub fn quantile(&self, q: f64) -> Option<u32> {
        if self.count == 0 {
            return None;
        }
        let q = q.clamp(0.0, 1.0);
        // Rango 1-based del valor buscado.
        let rank = ((q * f64::from(self.count)).ceil() as u64).max(1);
        let mut seen = 0u64;
        for (i, n) in self.buckets.iter().enumerate() {
            seen += u64::from(*n);
            if seen >= rank {
                return Some(upper(i).min(self.max));
            }
        }
        Some(self.max)
    }

    /// p50/p99/máx de lo registrado; `None` sin datos.
    pub fn stats(&self) -> Option<InputLatency> {
        Some(InputLatency {
            link: None,
            count: self.count,
            p50_us: self.quantile(0.50)?,
            p99_us: self.quantile(0.99)?,
            max_us: self.max,
        })
    }

    /// Las estadísticas de la ventana y una ventana nueva.
    pub fn take(&mut self) -> Option<InputLatency> {
        let s = self.stats();
        *self = Self::new();
        s
    }
}

fn index(v: u32) -> usize {
    let v = u64::from(v);
    if v < LINEAR {
        return v as usize;
    }
    let exp = 63 - u64::from(v.leading_zeros());
    let top = v >> (exp - u64::from(SUB_BITS));
    (LINEAR + (exp - u64::from(SUB_BITS) - 1) * SUB + (top - SUB)) as usize
}

/// Mayor valor que cae en la cubeta `i`.
fn upper(i: usize) -> u32 {
    let i = i as u64;
    if i < LINEAR {
        return i as u32;
    }
    let k = i - LINEAR;
    let shift = k / SUB + 1;
    let top = SUB + k % SUB;
    let v = ((top + 1) << shift) - 1;
    u32::try_from(v).unwrap_or(u32::MAX)
}

/// El eco del último latido recibido: cuándo lo mandó el otro (en su reloj) y cuánto
/// lo retuvo quien contesta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Echo {
    pub sent_us: u64,
    pub held_us: u64,
}

/// Latido con marca de tiempo; `echo` devuelve el último del otro lado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Beat {
    pub sent_us: u64,
    pub echo: Option<Echo>,
}

/// Una muestra de la ida y vuelta: la ida y vuelta sin lo retenido y el desfase
/// (reloj del otro − reloj propio) suponiendo ida y vuelta simétricas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sample {
    rtt_us: u64,
    offset_us: i64,
}

/// Estimación del desfase con el reloj del otro lado. Se queda con la muestra de menor
/// ida y vuelta de las últimas [`ClockSync::WINDOW`]: la más corta es la que menos
/// asimetría puede tener.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClockSync {
    samples: VecDeque<Sample>,
}

impl ClockSync {
    pub const WINDOW: usize = 16;

    /// `t0`: cuándo mandamos el latido (nuestro reloj); `t2`: cuándo mandó el otro el
    /// suyo con el eco (su reloj); `held`: cuánto lo retuvo; `t3`: cuándo llegó (nuestro).
    pub fn sample(&mut self, t0: u64, t2: u64, held: u64, t3: u64) {
        let Some(rtt) = t3.checked_sub(t0).and_then(|r| r.checked_sub(held)) else {
            return;
        };
        // t1 = t2 − held es cuándo le llegó al otro el nuestro.
        let (t0, t2, held, t3) = (t0 as i128, t2 as i128, held as i128, t3 as i128);
        let offset = ((t2 - held - t0) + (t2 - t3)) / 2;
        let Ok(offset_us) = i64::try_from(offset) else {
            return;
        };
        if self.samples.len() == Self::WINDOW {
            self.samples.pop_front();
        }
        self.samples.push_back(Sample {
            rtt_us: rtt,
            offset_us,
        });
    }

    /// Reloj del otro − reloj propio, si ya hay una muestra.
    pub fn offset_us(&self) -> Option<i64> {
        self.best().map(|s| s.offset_us)
    }

    /// La menor ida y vuelta de la ventana.
    pub fn rtt_us(&self) -> Option<u64> {
        self.best().map(|s| s.rtt_us)
    }

    fn best(&self) -> Option<&Sample> {
        self.samples.iter().min_by_key(|s| s.rtt_us)
    }

    /// Cuánto pasó desde la marca `peer_us` del otro hasta `now_us` nuestro.
    pub fn elapsed_since(&self, peer_us: u64, now_us: u64) -> Option<u64> {
        let offset = self.offset_us()?;
        let local = i128::from(peer_us) - i128::from(offset);
        // Un error de estimación puede dar algo negativo: es cero.
        let d = (i128::from(now_us) - local).max(0);
        Some(u64::try_from(d).unwrap_or(u64::MAX))
    }
}

/// Los latidos de un lado: arma los que salen (con el eco del último que llegó) y saca
/// muestras de desfase de los que llegan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Beats {
    /// El último latido del otro: cuándo lo mandó (su reloj) y cuándo llegó (el nuestro).
    last_rx: Option<(u64, u64)>,
    sync: ClockSync,
}

impl Beats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn outgoing(&self, now: u64) -> Beat {
        Beat {
            sent_us: now,
            echo: self.last_rx.map(|(sent, at)| Echo {
                sent_us: sent,
                held_us: now.saturating_sub(at),
            }),
        }
    }

    pub fn incoming(&mut self, beat: Beat, now: u64) {
        if let Some(e) = beat.echo {
            self.sync.sample(e.sent_us, beat.sent_us, e.held_us, now);
        }
        self.last_rx = Some((beat.sent_us, now));
    }

    pub fn sync(&self) -> &ClockSync {
        &self.sync
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn sin_datos_no_hay_estadisticas() {
        let mut h = Histogram::new();
        assert_eq!(h.stats(), None);
        assert_eq!(h.take(), None);
        assert_eq!(h.quantile(0.5), None);
    }

    #[test]
    fn percentiles_de_una_distribucion_conocida() {
        let mut h = Histogram::new();
        for v in 1..=100u64 {
            h.record(v);
        }
        let s = h.take().unwrap();
        assert_eq!(s.count, 100);
        assert_eq!(s.max_us, 100);
        // Debajo de 64 cada valor es exacto; arriba, a menos de 1/32.
        assert_eq!(s.p50_us, 50);
        assert!((99..=102).contains(&s.p99_us), "{}", s.p99_us);
        assert_eq!(h.count(), 0, "tomar abre una ventana nueva");
    }

    #[test]
    fn valores_enormes_no_rompen_nada() {
        let mut h = Histogram::new();
        h.record(u64::MAX);
        h.record(0);
        let s = h.stats().unwrap();
        assert_eq!(s.max_us, u32::MAX);
        assert_eq!(h.quantile(1.0), Some(u32::MAX));
        assert_eq!(h.quantile(0.0), Some(0));
    }

    #[test]
    fn las_cubetas_cubren_todo_u32_en_orden() {
        let mut values: Vec<u32> = (0..32)
            .flat_map(|shift| [1u32 << shift, (1u32 << shift).saturating_add(7)])
            .collect();
        values.sort_unstable();
        let mut last = 0;
        for v in values {
            let i = index(v);
            assert!(i < BUCKETS, "{v} → {i}");
            assert!(i >= last, "{v} → {i} < {last}");
            assert!(upper(i) >= v);
            assert!(i == 0 || upper(i - 1) < v, "{v} cabe en la cubeta anterior");
            last = i;
        }
        assert_eq!(index(u32::MAX), BUCKETS - 1);
        assert_eq!(upper(BUCKETS - 1), u32::MAX);
    }

    #[test]
    fn desfase_con_ida_y_vuelta_simetrica() {
        // El otro reloj va 1 s adelante; 400 µs de ida, 400 de vuelta, 10 ms retenido.
        let mut c = ClockSync::default();
        assert_eq!(c.offset_us(), None);
        let (t0, t1) = (5_000u64, 1_005_400u64);
        let t2 = t1 + 10_000;
        let t3 = t0 + 400 + 10_000 + 400;
        c.sample(t0, t2, 10_000, t3);
        assert_eq!(c.offset_us(), Some(1_000_000));
        assert_eq!(c.rtt_us(), Some(800));
        // Una marca del otro de hace 2 ms, vista desde aquí.
        assert_eq!(c.elapsed_since(1_100_000, 102_000), Some(2_000));
        // Una estimación que da negativo es cero.
        assert_eq!(c.elapsed_since(1_100_000, 99_000), Some(0));
    }

    #[test]
    fn manda_la_muestra_de_menor_ida_y_vuelta() {
        let mut c = ClockSync::default();
        // Ida lenta (cola): 5 ms de ida y 100 µs de vuelta, desfase real 0.
        c.sample(0, 5_000, 0, 5_100);
        assert_eq!(c.offset_us(), Some(2_450));
        // Una muestra limpia gana.
        c.sample(10_000, 10_050, 0, 10_100);
        assert_eq!(c.offset_us(), Some(0));
        // Muestras imposibles (relojes que vuelven atrás) se descartan.
        c.sample(10, 0, 50, 20);
        assert_eq!(c.rtt_us(), Some(100));
    }

    #[test]
    fn los_latidos_con_eco_estiman_el_desfase() {
        // A y B con relojes separados por 7 ms; 300 µs por sentido.
        let mut a = Beats::new();
        let mut b = Beats::new();
        let skew = 7_000u64;
        let ab = a.outgoing(1_000);
        assert_eq!(ab.echo, None);
        b.incoming(ab, 1_300 + skew);
        // B contesta 1 s después, con el eco.
        let ba = b.outgoing(1_001_300 + skew);
        assert_eq!(
            ba.echo,
            Some(Echo {
                sent_us: 1_000,
                held_us: 1_000_000
            })
        );
        a.incoming(ba, 1_001_600);
        assert_eq!(a.sync().offset_us(), Some(7_000));
        assert_eq!(a.sync().rtt_us(), Some(600));
        assert_eq!(b.sync().offset_us(), None, "B todavía no recibió un eco");
    }

    proptest! {
        /// El percentil informado nunca queda por debajo del real y lo pasa a lo sumo
        /// en 1/32 (más un microsegundo de redondeo); el máximo es exacto.
        #[test]
        fn el_percentil_acota_al_real(
            mut values in prop::collection::vec(0u64..5_000_000, 1..400),
            q in 0.0f64..=1.0,
        ) {
            let mut h = Histogram::new();
            for v in &values {
                h.record(*v);
            }
            values.sort_unstable();
            let rank = ((q * values.len() as f64).ceil() as usize).max(1);
            let real = values[rank - 1] as f64;
            let got = f64::from(h.quantile(q).unwrap());
            prop_assert!(got >= real, "{got} < {real}");
            prop_assert!(got <= real * (1.0 + 1.0 / 32.0) + 1.0, "{got} ≫ {real}");
            prop_assert_eq!(h.stats().unwrap().max_us as u64, *values.last().unwrap());
        }

        /// Los percentiles crecen con `q`.
        #[test]
        fn los_percentiles_son_monotonos(
            values in prop::collection::vec(any::<u32>(), 1..200),
            a in 0.0f64..=1.0,
            b in 0.0f64..=1.0,
        ) {
            let mut h = Histogram::new();
            for v in values {
                h.record(u64::from(v));
            }
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            prop_assert!(h.quantile(lo) <= h.quantile(hi));
        }

        /// Con ida y vuelta simétricas el desfase se recupera exacto (a ±1 µs de
        /// redondeo), sea cual sea el desfase real y lo retenido.
        #[test]
        fn desfase_simetrico_exacto(
            t0 in 0u64..1_000_000_000,
            skew in -1_000_000_000i64..1_000_000_000,
            one_way in 0u64..50_000,
            held in 0u64..2_000_000,
        ) {
            let t1 = (t0 as i64 + skew + one_way as i64).max(0) as u64;
            prop_assume!(t1 as i64 == t0 as i64 + skew + one_way as i64);
            let t2 = t1 + held;
            let t3 = t0 + one_way + held + one_way;
            let mut c = ClockSync::default();
            c.sample(t0, t2, held, t3);
            let off = c.offset_us().unwrap();
            prop_assert!((off - skew).abs() <= 1, "{off} vs {skew}");
            prop_assert_eq!(c.rtt_us(), Some(2 * one_way));
        }
    }
}
