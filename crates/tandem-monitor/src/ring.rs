//! El reloj del monitor y la hora de los eventos.
//!
//! El reloj `0xEF` cuenta en unidades de 10 ms con 32 bits: da la vuelta a los
//! 2³² × 10 ms ≈ 497 días. `monitor-ddc` toma «el reloj volvió atrás» como un reinicio del
//! monitor y vuelve a leer el anillo desde el principio; tras la vuelta del reloj eso daría
//! eventos repetidos. Aquí se distingue una cosa de la otra con el reloj propio: si entre dos
//! lecturas pasó justo lo que faltaba para el tope, es la vuelta.
//!
//! Las horas de los eventos tienen 24 bits (≈ 46,6 h): la edad de un evento se calcula con
//! los 24 bits bajos del reloj.

/// Cuántas unidades de 10 ms cuenta el reloj antes de volver a 0.
pub const CLOCK_SPAN_10MS: u64 = 1 << 32;
/// Los bits de la hora de un evento.
pub const EVENT_TIME_MASK: u32 = 0x00FF_FFFF;
/// Holgura entre el reloj del monitor y el de la máquina (60 s): los dos no avanzan igual, y
/// las lecturas tardan.
pub const SLACK_10MS: u64 = 6_000;

/// Si el reloj del monitor, que marcaba `before`, pudo dar la vuelta en `elapsed_10ms`: solo
/// entonces vale la pena leerlo antes de leer los eventos.
pub fn may_wrap(before: u32, elapsed_10ms: u64) -> bool {
    u64::from(before) + elapsed_10ms + SLACK_10MS >= CLOCK_SPAN_10MS
}

/// `now < before` porque el reloj dio la vuelta (y no porque el monitor se reinició): `now`
/// es lo que marcaría tras pasar `elapsed_10ms` desde `before` y volver a 0, con holgura.
///
/// Un reinicio justo en el minuto de la vuelta no se distingue; pasa una vez cada 497 días y
/// lo peor que deja es leer de menos (se marca como eventos perdidos).
pub fn wrapped(before: u32, now: u32, elapsed_10ms: u64) -> bool {
    if now >= before {
        return false;
    }
    let expected = u64::from(before) + elapsed_10ms;
    if expected + SLACK_10MS < CLOCK_SPAN_10MS {
        return false;
    }
    let expected_now = expected.saturating_sub(CLOCK_SPAN_10MS);
    u64::from(now).abs_diff(expected_now) <= SLACK_10MS
}

/// Hace cuánto pasó un evento con hora `event_10ms` si el reloj marca `clock_10ms` (los dos
/// en 10 ms; solo cuentan los 24 bits de la hora de los eventos).
pub fn event_age_10ms(clock_10ms: u32, event_10ms: u32) -> u32 {
    (clock_10ms & EVENT_TIME_MASK).wrapping_sub(event_10ms & EVENT_TIME_MASK) & EVENT_TIME_MASK
}

/// La hora de un evento en milisegundos Unix, sabiendo que al leer el reloj (`clock_10ms`)
/// eran `wall_now_ms`.
pub fn event_at_ms(wall_now_ms: u64, clock_10ms: u32, event_10ms: u32) -> u64 {
    wall_now_ms.saturating_sub(u64::from(event_age_10ms(clock_10ms, event_10ms)) * 10)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const DAY_10MS: u64 = 8_640_000;

    #[test]
    fn a_reboot_is_not_a_wrap() {
        // Encendido hace un día, se reinició y lleva 10 s.
        let before = DAY_10MS as u32;
        assert!(!may_wrap(before, 500));
        assert!(!wrapped(before, 1_000, 500));
    }

    #[test]
    fn the_wrap_after_497_days_is_not_a_reboot() {
        let before = u32::MAX - 99; // un segundo antes del tope
        assert!(may_wrap(before, 300));
        // Tres segundos después marca 2 s.
        assert!(wrapped(before, 199, 300));
    }

    #[test]
    fn event_time_uses_the_low_24_bits() {
        // El reloj ya pasó las 46 h: sus 24 bits bajos dieron la vuelta, el evento no.
        let clock = (1u32 << 24) + 50;
        let event = EVENT_TIME_MASK - 49;
        assert_eq!(event_age_10ms(clock, event), 100);
        assert_eq!(event_at_ms(10_000, clock, event), 9_000);
        assert_eq!(event_at_ms(500, clock, event), 0, "nunca antes de la época");
    }

    proptest! {
        /// Cualquier vuelta de verdad se reconoce, con el reloj del monitor desfasado del
        /// nuestro dentro de la holgura.
        #[test]
        fn every_real_wrap_is_recognised(
            left in 1u64..DAY_10MS,
            past in 0u64..DAY_10MS,
            drift in -(SLACK_10MS as i64)..=(SLACK_10MS as i64),
        ) {
            let before = (CLOCK_SPAN_10MS - left) as u32;
            let now = past as u32;
            let elapsed = (left + past) as i64 + drift;
            prop_assume!(elapsed >= 0);
            prop_assert!(may_wrap(before, elapsed as u64));
            if now < before {
                prop_assert!(wrapped(before, now, elapsed as u64));
            }
        }

        /// Un reinicio lejos del tope nunca se toma por la vuelta.
        #[test]
        fn a_reboot_far_from_the_top_is_never_a_wrap(
            before in 0u32..(u32::MAX / 2),
            elapsed in 0u64..(30 * DAY_10MS),
            uptime in 0u64..(30 * DAY_10MS),
        ) {
            let now = uptime.min(elapsed) as u32;
            prop_assert!(!wrapped(before, now, elapsed));
        }

        /// La edad de un evento es exacta mientras sea menor que lo que cubren 24 bits.
        #[test]
        fn event_age_is_exact_within_24_bits(clock in any::<u32>(), age in 0u32..EVENT_TIME_MASK) {
            let event = clock.wrapping_sub(age) & EVENT_TIME_MASK;
            prop_assert_eq!(event_age_10ms(clock, event), age);
        }
    }
}
