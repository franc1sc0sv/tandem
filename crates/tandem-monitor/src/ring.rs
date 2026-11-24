//! The monitor clock and the time of events.
//!
//! The `0xEF` clock counts in 10 ms units with 32 bits: it wraps around at
//! 2³² × 10 ms ≈ 497 days. `monitor-ddc` takes "the clock went backwards" as a restart of the
//! monitor and reads the ring again from the start; after the clock wraps that would give
//! repeated events. Here the two cases are told apart with our own clock: if between two
//! reads exactly what was missing to the limit went by, it is the wraparound.
//!
//! Event times have 24 bits (≈ 46.6 h): the age of an event is computed with
//! the low 24 bits of the clock.

/// How many 10 ms units the clock counts before going back to 0.
pub const CLOCK_SPAN_10MS: u64 = 1 << 32;
/// The bits of an event's time.
pub const EVENT_TIME_MASK: u32 = 0x00FF_FFFF;
/// Slack between the monitor's clock and the machine's (60 s): the two do not advance the same, and
/// reads take time.
pub const SLACK_10MS: u64 = 6_000;

/// Whether the monitor's clock, which read `before`, could have wrapped in `elapsed_10ms`: only
/// then is it worth reading it before reading the events.
pub fn may_wrap(before: u32, elapsed_10ms: u64) -> bool {
    u64::from(before) + elapsed_10ms + SLACK_10MS >= CLOCK_SPAN_10MS
}

/// `now < before` because the clock wrapped (and not because the monitor restarted): `now`
/// is what it would read after `elapsed_10ms` went by from `before` and back to 0, with slack.
///
/// A restart right in the minute of the wraparound cannot be told apart; it happens once every 497 days and
/// the worst it leaves is reading too little (it is marked as lost events).
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

/// How long ago an event with time `event_10ms` happened if the clock reads `clock_10ms` (both
/// in 10 ms; only the 24 bits of the event time count).
pub fn event_age_10ms(clock_10ms: u32, event_10ms: u32) -> u32 {
    (clock_10ms & EVENT_TIME_MASK).wrapping_sub(event_10ms & EVENT_TIME_MASK) & EVENT_TIME_MASK
}

/// The time of an event in Unix milliseconds, knowing that when the clock was read (`clock_10ms`)
/// it was `wall_now_ms`.
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
        // Powered on a day ago, restarted and has been running for 10 s.
        let before = DAY_10MS as u32;
        assert!(!may_wrap(before, 500));
        assert!(!wrapped(before, 1_000, 500));
    }

    #[test]
    fn the_wrap_after_497_days_is_not_a_reboot() {
        let before = u32::MAX - 99; // one second before the limit
        assert!(may_wrap(before, 300));
        // Three seconds later it reads 2 s.
        assert!(wrapped(before, 199, 300));
    }

    #[test]
    fn event_time_uses_the_low_24_bits() {
        // The clock already passed 46 h: its low 24 bits wrapped, the event's did not.
        let clock = (1u32 << 24) + 50;
        let event = EVENT_TIME_MASK - 49;
        assert_eq!(event_age_10ms(clock, event), 100);
        assert_eq!(event_at_ms(10_000, clock, event), 9_000);
        assert_eq!(event_at_ms(500, clock, event), 0, "never before the epoch");
    }

    proptest! {
        /// Any real wraparound is recognized, with the monitor's clock out of sync with
        /// ours within the slack.
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

        /// A restart far from the limit is never taken for the wraparound.
        #[test]
        fn a_reboot_far_from_the_top_is_never_a_wrap(
            before in 0u32..(u32::MAX / 2),
            elapsed in 0u64..(30 * DAY_10MS),
            uptime in 0u64..(30 * DAY_10MS),
        ) {
            let now = uptime.min(elapsed) as u32;
            prop_assert!(!wrapped(before, now, elapsed));
        }

        /// The age of an event is exact as long as it is less than what 24 bits cover.
        #[test]
        fn event_age_is_exact_within_24_bits(clock in any::<u32>(), age in 0u32..EVENT_TIME_MASK) {
            let event = clock.wrapping_sub(age) & EVENT_TIME_MASK;
            prop_assert_eq!(event_age_10ms(clock, event), age);
        }
    }
}
