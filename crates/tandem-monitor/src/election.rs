//! The owner of a monitor (DESIGN §10, DECISIONS D13).
//!
//! Both machines compute the same thing from the same data: the channels each one announced
//! ([`MonitorChannel`]). There is no voting and no messages of its own: if both have both
//! announcements, they agree. The owner is the one that has a channel; if both do, the one
//! that answered the probe faster, and on a small difference, `A` (latency is measured once
//! when the channel opens, so it does not oscillate). If the owner leaves or loses the
//! channel, its announcement stops naming the monitor and the other one remains.

use tandem_core::Machine;
use tandem_proto::MonitorChannel;

/// What `B` has to win in latency to take the monitor from `A`: below that, the
/// noise of a measurement would decide.
pub const LATENCY_MARGIN_MS: u32 = 10;

/// The owner of the monitor given the channels of `a` and of `b` (`None` if that machine does
/// not see it or nothing is known about it). Only a compatible monitor has an owner: one that
/// does not speak the standard is neither read nor controlled.
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
                    reason: "factory".into(),
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
        // 5 ms of difference is noise.
        assert_eq!(
            elect(Some(&channel(true, 20)), Some(&channel(true, 15))),
            Some(Machine::A)
        );
    }

    #[test]
    fn a_monitor_that_is_not_compatible_has_no_owner() {
        let factory = channel(false, 1);
        assert_eq!(elect(Some(&factory), Some(&factory)), None);
        // If one sees it as compatible and the other does not (another protocol version), the one that can.
        assert_eq!(
            elect(Some(&factory), Some(&channel(true, 90))),
            Some(Machine::B)
        );
    }

    fn any_channel() -> impl Strategy<Value = Option<MonitorChannel>> {
        proptest::option::of((any::<bool>(), 0u32..500).prop_map(|(c, l)| channel(c, l)))
    }

    proptest! {
        /// The owner always has a compatible channel, and if anyone has one, there is an owner.
        #[test]
        fn the_owner_always_has_a_compatible_channel(a in any_channel(), b in any_channel()) {
            let usable = |c: &Option<MonitorChannel>| c.as_ref().is_some_and(|c| c.compat.is_compatible());
            match elect(a.as_ref(), b.as_ref()) {
                Some(Machine::A) => prop_assert!(usable(&a)),
                Some(Machine::B) => prop_assert!(usable(&b)),
                None => prop_assert!(!usable(&a) && !usable(&b)),
            }
        }

        /// Swapping the sides swaps the owner, except in a tie (which belongs to `A` on both).
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
