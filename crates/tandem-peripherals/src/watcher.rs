//! Hub detection: which devices are there, debounced, by polling.

use std::fmt::Display;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::debounce::Debouncer;
use crate::id::UsbId;

/// How long a change has to hold to invert the roles (DESIGN §7).
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_secs(3);
/// How often the connected devices are read.
pub const DEFAULT_POLL: Duration = Duration::from_millis(500);

/// The keyboard and mouse that travel on the hub, by VID/PID.
///
/// ```toml
/// keyboard = "046d:c33f"
/// mouse = "046d:c539"
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HubConfig {
    pub keyboard: UsbId,
    pub mouse: UsbId,
}

/// Which hub devices are seen on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Presence {
    pub keyboard: bool,
    pub mouse: bool,
}

impl Presence {
    pub fn from_devices(cfg: &HubConfig, devices: &[UsbId]) -> Self {
        Presence {
            keyboard: devices.contains(&cfg.keyboard),
            mouse: devices.contains(&cfg.mouse),
        }
    }

    /// The hub is here if either of the two is seen: with only the mouse, this
    /// machine is still the one that has to capture. For the UI chip there is
    /// the detail of each one.
    pub fn hub(&self) -> bool {
        self.keyboard || self.mouse
    }
}

/// Lists the USB devices connected on this machine. Each platform
/// implements it; it may block for a few milliseconds.
pub trait DeviceScanner: Send {
    type Error: Display;

    fn scan(&mut self) -> Result<Vec<UsbId>, Self::Error>;
}

/// Hub presence with debounce, without clock or I/O: receives each reading with
/// its instant.
#[derive(Debug, Clone)]
pub struct HubTracker {
    cfg: HubConfig,
    debouncer: Debouncer<Presence>,
}

impl HubTracker {
    /// The first reading is accepted as is: at startup there is nothing to filter.
    pub fn new(cfg: HubConfig, first: &[UsbId], debounce: Duration) -> Self {
        let initial = Presence::from_devices(&cfg, first);
        HubTracker {
            cfg,
            debouncer: Debouncer::new(initial, debounce),
        }
    }

    pub fn presence(&self) -> Presence {
        *self.debouncer.stable()
    }

    /// Returns the new presence when a change held long enough.
    pub fn observe(&mut self, devices: &[UsbId], now: Instant) -> Option<Presence> {
        let seen = Presence::from_devices(&self.cfg, devices);
        self.debouncer.observe(seen, now)
    }
}

/// Polling times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchConfig {
    pub poll: Duration,
    pub debounce: Duration,
}

impl Default for WatchConfig {
    fn default() -> Self {
        WatchConfig {
            poll: DEFAULT_POLL,
            debounce: DEFAULT_DEBOUNCE,
        }
    }
}

/// Polls the scanner and publishes the stable presence on a `watch`. Whoever
/// receives it passes it to the local [`RoleArbiter`](crate::RoleArbiter) and reports it
/// to the other agent.
pub struct HubWatcher<S> {
    scanner: S,
    tracker: HubTracker,
    poll: Duration,
    tx: watch::Sender<Presence>,
}

impl<S: DeviceScanner> HubWatcher<S> {
    /// Does the first reading. If it fails, it starts as if nothing were
    /// connected: the debounce takes care of it when the scanner recovers.
    pub fn new(mut scanner: S, cfg: HubConfig, timing: WatchConfig) -> Self {
        let first = scan_or_log(&mut scanner).unwrap_or_default();
        let tracker = HubTracker::new(cfg, &first, timing.debounce);
        let (tx, _) = watch::channel(tracker.presence());
        HubWatcher {
            scanner,
            tracker,
            poll: timing.poll,
            tx,
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<Presence> {
        self.tx.subscribe()
    }

    pub fn presence(&self) -> Presence {
        self.tracker.presence()
    }

    /// Polls until all receivers are dropped. A failed reading does not
    /// count as "nothing there": it is ignored.
    pub async fn run(mut self) {
        let mut tick = tokio::time::interval(self.poll);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = self.tx.closed() => return,
                now = tick.tick() => {
                    let Some(devices) = scan_or_log(&mut self.scanner) else { continue };
                    if let Some(p) = self.tracker.observe(&devices, now.into_std()) {
                        tracing::info!(keyboard = p.keyboard, mouse = p.mouse, "hub changed");
                        self.tx.send_replace(p);
                    }
                }
            }
        }
    }
}

fn scan_or_log<S: DeviceScanner>(scanner: &mut S) -> Option<Vec<UsbId>> {
    match scanner.scan() {
        Ok(devices) => Some(devices),
        Err(e) => {
            tracing::warn!("could not read the USB devices: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeScanner;

    const KB: UsbId = UsbId::new(0x046d, 0xc33f);
    const MOUSE: UsbId = UsbId::new(0x046d, 0xc539);
    const OTHER: UsbId = UsbId::new(0x05ac, 0x8104);

    fn cfg() -> HubConfig {
        HubConfig {
            keyboard: KB,
            mouse: MOUSE,
        }
    }

    #[test]
    fn config_from_toml() {
        let c: HubConfig =
            toml::from_str("keyboard = \"046d:c33f\"\nmouse = \"046d:c539\"").unwrap();
        assert_eq!(c, cfg());
        assert!(toml::from_str::<HubConfig>("keyboard = \"046d:c33f\"").is_err());
        assert!(
            toml::from_str::<HubConfig>(
                "keyboard = \"046d:c33f\"\nmouse = \"046d:c539\"\nhub = \"1:2\""
            )
            .is_err()
        );
    }

    #[test]
    fn presence_counts_only_configured_devices() {
        let p = Presence::from_devices(&cfg(), &[OTHER, MOUSE]);
        assert_eq!(
            p,
            Presence {
                keyboard: false,
                mouse: true
            }
        );
        assert!(p.hub());
        assert!(!Presence::from_devices(&cfg(), &[OTHER]).hub());
    }

    #[test]
    fn tracker_debounces_plug_and_unplug() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut t = HubTracker::new(cfg(), &[OTHER], DEFAULT_DEBOUNCE);
        assert!(!t.presence().hub());
        assert_eq!(t.observe(&[KB, MOUSE], at(0)), None);
        assert_eq!(t.observe(&[KB, MOUSE], at(2)), None);
        let p = t.observe(&[KB, MOUSE], at(3)).unwrap();
        assert!(p.keyboard && p.mouse);
        // The keyboard enumerates late: it is another change, with its own wait.
        assert_eq!(t.observe(&[MOUSE], at(4)), None);
        assert_eq!(t.observe(&[KB, MOUSE], at(5)), None);
        assert_eq!(t.presence(), p);
    }

    /// Advances the tokio clock in small steps so the polling runs.
    async fn advance(d: Duration) {
        let step = Duration::from_millis(100);
        let mut left = d;
        while left > Duration::ZERO {
            let s = left.min(step);
            tokio::time::advance(s).await;
            tokio::task::yield_now().await;
            left -= s;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn watcher_publishes_only_sustained_changes() {
        let scanner = FakeScanner::new(vec![OTHER]);
        let devices = scanner.handle();
        let w = HubWatcher::new(scanner, cfg(), WatchConfig::default());
        let mut rx = w.subscribe();
        assert!(!rx.borrow().hub());
        let task = tokio::spawn(w.run());

        // The hub connects.
        devices.set(vec![OTHER, KB, MOUSE]);
        advance(Duration::from_millis(2_500)).await;
        assert!(!rx.has_changed().unwrap(), "changed before 3 s");
        advance(Duration::from_secs(1)).await;
        assert!(rx.has_changed().unwrap());
        assert!(rx.borrow_and_update().hub());

        // A 1 s tug on the cable changes nothing.
        devices.set(vec![OTHER]);
        advance(Duration::from_secs(1)).await;
        devices.set(vec![OTHER, KB, MOUSE]);
        advance(Duration::from_secs(5)).await;
        assert!(!rx.has_changed().unwrap());

        // Failed readings in the middle of a disconnection neither interrupt nor
        // advance it.
        devices.set(vec![OTHER]);
        advance(Duration::from_secs(1)).await;
        devices.fail(true);
        advance(Duration::from_secs(1)).await;
        devices.fail(false);
        assert!(!rx.has_changed().unwrap());
        advance(Duration::from_millis(1_600)).await;
        assert!(rx.has_changed().unwrap());
        assert!(!rx.borrow_and_update().hub());

        drop(rx);
        advance(Duration::from_secs(1)).await;
        assert!(task.is_finished(), "polling continues without receivers");
    }

    #[tokio::test(start_paused = true)]
    async fn failed_first_scan_starts_without_hub() {
        let scanner = FakeScanner::new(vec![KB, MOUSE]);
        scanner.handle().fail(true);
        let w = HubWatcher::new(scanner, cfg(), WatchConfig::default());
        assert!(!w.presence().hub());
    }
}
