//! How much data may use depending on the path (DECISIONS A2) and how much input
//! latency is tolerated on each (DESIGN §7.1).

use std::time::Duration;

use tandem_core::DataPrefs;

use super::limiter::MIN_CAP;
use crate::manager::LinkKind;

/// What Wi-Fi is assumed to give while nobody measured it (T-102 measures it on request):
/// 20 MB/s (160 Mbit/s), what a modest Wi-Fi 5 gives in practice. With the 50 % by
/// default, 10 MB/s for data.
pub const UNMEASURED_WIFI: u64 = 20_000_000;

/// The data cap per `link`, in bytes per second; `None` means no cap (cable).
/// `measured` is the last bandwidth measured on that path.
pub fn data_cap(link: Option<LinkKind>, prefs: &DataPrefs, measured: Option<u64>) -> Option<u64> {
    match link? {
        LinkKind::Wifi => Some(wifi_cap(prefs, measured)),
        LinkKind::Usb4 | LinkKind::Ethernet => None,
    }
}

fn wifi_cap(prefs: &DataPrefs, measured: Option<u64>) -> u64 {
    let cap = prefs.fixed_cap_bytes().unwrap_or_else(|| {
        let base = measured.filter(|m| *m > 0).unwrap_or(UNMEASURED_WIFI);
        let percent = u64::from(prefs.wifi_cap_percent.clamp(1, 100));
        base / 100 * percent + base % 100 * percent / 100
    });
    cap.max(MIN_CAP)
}

/// The p99 tolerated for input per `link` (DESIGN §7.1). With no known path, the
/// Wi-Fi one, the most lax.
pub fn input_p99_target(link: Option<LinkKind>) -> Duration {
    match link {
        Some(LinkKind::Usb4 | LinkKind::Ethernet) => Duration::from_millis(4),
        Some(LinkKind::Wifi) | None => Duration::from_millis(20),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_cable_there_is_no_cap() {
        let p = DataPrefs::default();
        assert_eq!(data_cap(Some(LinkKind::Usb4), &p, Some(1000)), None);
        assert_eq!(data_cap(Some(LinkKind::Ethernet), &p, None), None);
        assert_eq!(data_cap(None, &p, None), None);
    }

    #[test]
    fn over_wifi_half_of_what_was_measured() {
        let p = DataPrefs::default();
        assert_eq!(
            data_cap(Some(LinkKind::Wifi), &p, Some(30_000_000)),
            Some(15_000_000)
        );
        // Without a measurement, half of what is assumed.
        assert_eq!(
            data_cap(Some(LinkKind::Wifi), &p, None),
            Some(UNMEASURED_WIFI / 2)
        );
        // A measurement of 0 does not leave data stopped forever.
        assert_eq!(
            data_cap(Some(LinkKind::Wifi), &p, Some(0)),
            Some(10_000_000)
        );
    }

    #[test]
    fn the_percentage_and_the_fixed_cap_are_respected() {
        let pct = DataPrefs {
            wifi_cap_percent: 30,
            wifi_cap_mbps: None,
        };
        assert_eq!(
            data_cap(Some(LinkKind::Wifi), &pct, Some(10_000_000)),
            Some(3_000_000)
        );
        let fixed = DataPrefs {
            wifi_cap_percent: 30,
            wifi_cap_mbps: Some(40),
        };
        assert_eq!(
            data_cap(Some(LinkKind::Wifi), &fixed, Some(10_000_000)),
            Some(5_000_000)
        );
        // Never below the minimum, and even with huge figures it does not overflow.
        assert_eq!(
            data_cap(Some(LinkKind::Wifi), &pct, Some(1000)),
            Some(MIN_CAP)
        );
        assert!(data_cap(Some(LinkKind::Wifi), &pct, Some(u64::MAX)).is_some());
    }

    #[test]
    fn design_latency_targets() {
        assert_eq!(
            input_p99_target(Some(LinkKind::Usb4)),
            Duration::from_millis(4)
        );
        assert_eq!(
            input_p99_target(Some(LinkKind::Ethernet)),
            Duration::from_millis(4)
        );
        assert_eq!(
            input_p99_target(Some(LinkKind::Wifi)),
            Duration::from_millis(20)
        );
    }
}
