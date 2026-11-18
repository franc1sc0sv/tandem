//! Presupuestos de latencia de la entrada (T-093) y percentiles del ping de control.

use tandem_proto::{InputLatency, Link};

/// (p50, p99) máximos en microsegundos para el enlace.
pub fn budget(link: Link) -> (u32, u32) {
    match link {
        Link::Usb4 | Link::Ethernet => (1_000, 4_000),
        Link::Wifi => (5_000, 20_000),
    }
}

/// Eventos mínimos para que el percentil 99 signifique algo.
pub const MIN_SAMPLES: u32 = 20;

#[derive(Debug, PartialEq, Eq)]
pub enum Judgement {
    Pass(String),
    Fail(String),
    /// Muy pocos eventos en la ventana: no se puede juzgar.
    TooFew(u32),
}

pub fn judge(l: &InputLatency) -> Judgement {
    if l.count < MIN_SAMPLES {
        return Judgement::TooFew(l.count);
    }
    let Some(link) = l.link else {
        return Judgement::Fail("la medición no dice por qué enlace fue".into());
    };
    let (p50, p99) = budget(link);
    let said = format!(
        "{link:?}: p50 {} us (tope {p50}), p99 {} us (tope {p99}), {} eventos",
        l.p50_us, l.p99_us, l.count
    );
    if l.p50_us <= p50 && l.p99_us <= p99 {
        Judgement::Pass(said)
    } else {
        Judgement::Fail(said)
    }
}

/// p50 y p99 de una lista de tiempos (rango más cercano); `None` si está vacía.
pub fn percentiles(samples: &mut [u32]) -> Option<(u32, u32)> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    let len = samples.len();
    let at = |p: f64| samples[((p * len as f64).ceil() as usize).clamp(1, len) - 1];
    Some((at(0.50), at(0.99)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lat(link: Option<Link>, count: u32, p50: u32, p99: u32) -> InputLatency {
        InputLatency {
            link,
            count,
            p50_us: p50,
            p99_us: p99,
            max_us: p99,
        }
    }

    #[test]
    fn wifi_budget_is_looser_than_cable() {
        let wifi = lat(Some(Link::Wifi), 100, 4_000, 19_000);
        assert!(matches!(judge(&wifi), Judgement::Pass(_)));
        let usb = lat(Some(Link::Usb4), 100, 4_000, 19_000);
        assert!(matches!(judge(&usb), Judgement::Fail(_)));
        let eth = lat(Some(Link::Ethernet), 100, 900, 3_900);
        assert!(matches!(judge(&eth), Judgement::Pass(_)));
    }

    #[test]
    fn either_percentile_over_budget_fails() {
        let a = lat(Some(Link::Wifi), 100, 5_001, 1);
        let b = lat(Some(Link::Wifi), 100, 1, 20_001);
        assert!(matches!(judge(&a), Judgement::Fail(_)));
        assert!(matches!(judge(&b), Judgement::Fail(_)));
    }

    #[test]
    fn few_samples_or_unknown_link_are_not_a_pass() {
        assert_eq!(judge(&lat(Some(Link::Wifi), 3, 1, 1)), Judgement::TooFew(3));
        assert!(matches!(judge(&lat(None, 100, 1, 1)), Judgement::Fail(_)));
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let mut v: Vec<u32> = (1..=100).rev().collect();
        assert_eq!(percentiles(&mut v), Some((50, 99)));
        assert_eq!(percentiles(&mut [7]), Some((7, 7)));
        assert_eq!(percentiles(&mut []), None);
    }
}
