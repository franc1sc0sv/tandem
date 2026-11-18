//! Network metrics per path, for the Network screen and the Desk summary (T-102,
//! DESIGN §6, mockups `V3-Network` and `V3-Desk`).
//!
//! Times are in whole microseconds (cable latencies are tenths of a
//! millisecond) and dates in Unix milliseconds, so the messages compare with
//! `Eq` and the UI can say "3 min ago".

use serde::{Deserialize, Serialize};

use crate::msg::Link;

/// Samples in each path's history (one per second).
pub const HISTORY_SECS: usize = 60;
/// Path changes that are kept.
pub const MAX_SWITCHES: usize = 50;
/// How long the bandwidth test lasts.
pub const MEASURE_SECS: u64 = 3;

/// A path's role right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathState {
    /// Carries the input and the requests.
    Primary,
    /// Answers, ready to take the traffic if the main one drops.
    Backup,
    /// Never answered since the agent started (or is not configured): "No cable".
    Unplugged,
    /// Answered and stopped doing so.
    Down,
}

/// The last bandwidth measured for a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bandwidth {
    pub bytes_per_sec: u64,
    /// When it was measured.
    pub at_ms: u64,
}

/// One second of a path's history. `None`: it did not answer or there was no input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencySample {
    pub rtt_us: Option<u32>,
    pub input_p99_us: Option<u32>,
}

/// A path with its numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathMetrics {
    pub link: Link,
    pub state: PathState,
    /// Round trip of the last ping; `None` if it does not answer.
    pub rtt_us: Option<u32>,
    /// Input latency in the last second; `None` without input on this path.
    pub input_p50_us: Option<u32>,
    pub input_p99_us: Option<u32>,
    /// The p99 tolerated for input on this path (DESIGN §7.1): the chart's
    /// "target" line.
    pub input_target_us: u32,
    pub bandwidth: Option<Bandwidth>,
    /// Up to [`HISTORY_SECS`] samples, oldest to newest.
    pub history: Vec<LatencySample>,
}

/// A change of the main path ("switched to Wi-Fi in 0.6 s").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathSwitch {
    pub at_ms: u64,
    pub from: Option<Link>,
    pub to: Option<Link>,
    /// If `from` went down: from its last answer until `to` took the traffic (how
    /// long the input was without a path, at most). `None` if it was a switch up to a path
    /// better, without a cut.
    pub took_ms: Option<u32>,
}

/// A network notice for the Network screen: something the agent could not do on its own and the
/// user can (for example, assigning a cable's backup IP without permissions).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkNotice {
    pub interface: String,
    pub message: String,
    /// The exact command that resolves the notice.
    pub command: String,
}

/// The network with the other machine as seen from this agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkStatus {
    /// The three paths, in priority order (USB-C, Ethernet, Wi-Fi).
    pub paths: Vec<PathMetrics>,
    /// Up to [`MAX_SWITCHES`] changes, newest to oldest.
    pub switches: Vec<PathSwitch>,
    /// The path being measured right now, if any.
    pub measuring: Option<Link>,
    /// What the user has to do by hand (T-094).
    pub notices: Vec<NetworkNotice>,
}

impl NetworkStatus {
    /// The main path, if there is one.
    pub fn primary(&self) -> Option<&PathMetrics> {
        self.paths.iter().find(|p| p.state == PathState::Primary)
    }

    pub fn path(&self, link: Link) -> Option<&PathMetrics> {
        self.paths.iter().find(|p| p.link == link)
    }
}

/// What the receiver of the bandwidth test answers (over the same stream, when the
/// end arrives): how much it received and in how long, from the first byte to the last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasureReport {
    pub bytes: u64,
    pub micros: u64,
}

impl MeasureReport {
    /// Bytes per second; `None` if nothing arrived to measure.
    pub fn bytes_per_sec(&self) -> Option<u64> {
        if self.bytes == 0 || self.micros == 0 {
            return None;
        }
        let rate = u128::from(self.bytes) * 1_000_000 / u128::from(self.micros);
        Some(u64::try_from(rate).unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
pub(crate) mod samples {
    use super::*;

    /// The `V3-Network` mockup example: USB-C main, Ethernet without cable, Wi-Fi as
    /// backup, and the cable cut.
    pub fn status() -> NetworkStatus {
        let sample = LatencySample {
            rtt_us: Some(300),
            input_p99_us: Some(1900),
        };
        NetworkStatus {
            paths: vec![
                PathMetrics {
                    link: Link::Usb4,
                    state: PathState::Primary,
                    rtt_us: Some(300),
                    input_p50_us: Some(400),
                    input_p99_us: Some(1900),
                    input_target_us: 4000,
                    bandwidth: Some(Bandwidth {
                        bytes_per_sec: 1_175_000_000,
                        at_ms: 1_763_400_000_000,
                    }),
                    history: vec![sample, LatencySample::default(), sample],
                },
                PathMetrics {
                    link: Link::Ethernet,
                    state: PathState::Unplugged,
                    rtt_us: None,
                    input_p50_us: None,
                    input_p99_us: None,
                    input_target_us: 4000,
                    bandwidth: None,
                    history: vec![],
                },
                PathMetrics {
                    link: Link::Wifi,
                    state: PathState::Backup,
                    rtt_us: Some(3800),
                    input_p50_us: None,
                    input_p99_us: None,
                    input_target_us: 20_000,
                    bandwidth: None,
                    history: vec![LatencySample {
                        rtt_us: Some(3800),
                        input_p99_us: None,
                    }],
                },
            ],
            notices: vec![],
            switches: vec![
                PathSwitch {
                    at_ms: 1_763_400_060_000,
                    from: Some(Link::Wifi),
                    to: Some(Link::Usb4),
                    took_ms: None,
                },
                PathSwitch {
                    at_ms: 1_763_400_000_000,
                    from: Some(Link::Usb4),
                    to: Some(Link::Wifi),
                    took_ms: Some(600),
                },
                PathSwitch {
                    at_ms: 1_763_399_000_000,
                    from: None,
                    to: Some(Link::Usb4),
                    took_ms: None,
                },
            ],
            measuring: Some(Link::Wifi),
        }
    }

    pub fn path_state_arm(s: PathState) -> usize {
        match s {
            PathState::Primary => 0,
            PathState::Backup => 1,
            PathState::Unplugged => 2,
            PathState::Down => 3,
        }
    }

    /// One state of each kind.
    pub fn path_states() -> Vec<PathState> {
        let mut v: Vec<_> = status().paths.iter().map(|p| p.state).collect();
        v.push(PathState::Down);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::samples::*;
    use super::*;
    use crate::{decode, encode};

    #[test]
    fn network_state_round_trips() {
        let s = status();
        assert_eq!(decode::<NetworkStatus>(&encode(&s).unwrap()).unwrap(), s);
        let r = MeasureReport {
            bytes: 3,
            micros: 1,
        };
        assert_eq!(decode::<MeasureReport>(&encode(&r).unwrap()).unwrap(), r);
        let seen: std::collections::BTreeSet<_> =
            path_states().into_iter().map(path_state_arm).collect();
        assert_eq!(seen.len(), 4, "samples missing for some state");
    }

    #[test]
    fn the_main_path_is_the_one_carrying_the_traffic() {
        let s = status();
        assert_eq!(s.primary().map(|p| p.link), Some(Link::Usb4));
        assert_eq!(
            s.path(Link::Ethernet).map(|p| p.state),
            Some(PathState::Unplugged)
        );
        assert_eq!(NetworkStatus::default().primary(), None);
    }

    #[test]
    fn bandwidth_comes_from_what_was_received_and_its_time() {
        let r = |bytes, micros| MeasureReport { bytes, micros }.bytes_per_sec();
        assert_eq!(r(30_000_000, 3_000_000), Some(10_000_000));
        assert_eq!(r(1, 1), Some(1_000_000));
        assert_eq!(r(0, 3_000_000), None);
        assert_eq!(r(10, 0), None);
        assert_eq!(r(u64::MAX, 1), Some(u64::MAX));
    }
}
