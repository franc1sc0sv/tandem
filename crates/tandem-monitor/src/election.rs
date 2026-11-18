//! El dueño de un monitor (DESIGN §10, DECISIONS D13).
//!
//! Las dos máquinas calculan lo mismo con los mismos datos: los canales que anunció cada una
//! ([`MonitorChannel`]). No hay votación ni mensajes propios: si las dos tienen los dos
//! anuncios, coinciden. El dueño es la que tiene canal; si las dos, la que contestó más
//! rápido al sondeo, y ante una diferencia chica, `A` (la latencia se mide una vez al abrir el
//! canal, así que no oscila). Si el dueño se va o pierde el canal, su anuncio deja de
//! nombrar al monitor y queda la otra.

use tandem_core::Machine;
use tandem_proto::MonitorChannel;

/// Lo que tiene que ganar `B` en latencia para quitarle el monitor a `A`: por debajo, el
/// ruido de una medición decidiría.
pub const LATENCY_MARGIN_MS: u32 = 10;

/// El dueño del monitor con los canales de `a` y de `b` (`None` si esa máquina no lo ve o
/// no se sabe nada de ella). Solo un monitor compatible tiene dueño: uno que no habla el
/// estándar no se lee ni se controla.
pub fn elect(a: Option<&MonitorChannel>, b: Option<&MonitorChannel>) -> Option<Machine> {
    let usable = |c: Option<&MonitorChannel>| c.filter(|c| c.compat.is_compatible()).cloned();
    match (usable(a), usable(b)) {
        (None, None) => None,
        (Some(_), None) => Some(Machine::A),
        (None, Some(_)) => Some(Machine::B),
        (Some(a), Some(b)) => {
            if b.latency_ms.saturating_add(LATENCY_MARGIN_MS) < a.latency_ms {
                Some(Machine::B)
            } else {
                Some(Machine::A)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tandem_proto::MonitorCompat;

    use super::*;

    fn channel(compatible: bool, latency_ms: u32) -> MonitorChannel {
        MonitorChannel {
            id: "m".into(),
            name: "M".into(),
            via: None,
            compat: if compatible {
                MonitorCompat::Compatible { protocol: 2 }
            } else {
                MonitorCompat::NotCompatible {
                    reason: "fábrica".into(),
                }
            },
            latency_ms,
        }
    }

    #[test]
    fn the_one_with_a_channel_owns_it() {
        let c = channel(true, 50);
        assert_eq!(elect(Some(&c), None), Some(Machine::A));
        assert_eq!(elect(None, Some(&c)), Some(Machine::B));
        assert_eq!(elect(None, None), None);
    }

    #[test]
    fn with_both_the_faster_wins_and_ties_go_to_a() {
        let (slow, fast) = (channel(true, 40), channel(true, 8));
        assert_eq!(elect(Some(&slow), Some(&fast)), Some(Machine::B));
        assert_eq!(elect(Some(&fast), Some(&slow)), Some(Machine::A));
        // 5 ms de diferencia es ruido.
        assert_eq!(
            elect(Some(&channel(true, 20)), Some(&channel(true, 15))),
            Some(Machine::A)
        );
    }

    #[test]
    fn a_monitor_that_is_not_compatible_has_no_owner() {
        let factory = channel(false, 1);
        assert_eq!(elect(Some(&factory), Some(&factory)), None);
        // Si una lo ve compatible y la otra no (otra versión del protocolo), la que puede.
        assert_eq!(
            elect(Some(&factory), Some(&channel(true, 90))),
            Some(Machine::B)
        );
    }

    fn any_channel() -> impl Strategy<Value = Option<MonitorChannel>> {
        proptest::option::of((any::<bool>(), 0u32..500).prop_map(|(c, l)| channel(c, l)))
    }

    proptest! {
        /// El dueño siempre tiene un canal compatible, y si alguna lo tiene, hay dueño.
        #[test]
        fn the_owner_always_has_a_compatible_channel(a in any_channel(), b in any_channel()) {
            let usable = |c: &Option<MonitorChannel>| c.as_ref().is_some_and(|c| c.compat.is_compatible());
            match elect(a.as_ref(), b.as_ref()) {
                Some(Machine::A) => prop_assert!(usable(&a)),
                Some(Machine::B) => prop_assert!(usable(&b)),
                None => prop_assert!(!usable(&a) && !usable(&b)),
            }
        }

        /// Cambiar los lados cambia al dueño, salvo en un empate (que es de `A` en las dos).
        #[test]
        fn swapping_sides_swaps_the_owner_outside_a_tie(la in 0u32..500, lb in 0u32..500) {
            let (a, b) = (channel(true, la), channel(true, lb));
            let here = elect(Some(&a), Some(&b));
            let swapped = elect(Some(&b), Some(&a));
            if la.abs_diff(lb) > LATENCY_MARGIN_MS {
                prop_assert_eq!(here.map(Machine::other), swapped);
            } else {
                prop_assert_eq!(here, Some(Machine::A));
                prop_assert_eq!(swapped, Some(Machine::A));
            }
        }
    }
}
