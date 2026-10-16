//! Detección del hub: qué dispositivos hay, con antirrebote, sondeando.

use std::fmt::Display;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::debounce::Debouncer;
use crate::id::UsbId;

/// Cuánto tiene que sostenerse un cambio para invertir los roles (DESIGN §7).
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_secs(3);
/// Cada cuánto se leen los dispositivos conectados.
pub const DEFAULT_POLL: Duration = Duration::from_millis(500);

/// El teclado y el mouse que viajan en el hub, por VID/PID.
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

/// Qué dispositivos del hub se ven en esta máquina.
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

    /// El hub está aquí si se ve cualquiera de los dos: con solo el mouse, esta
    /// máquina sigue siendo la que tiene que capturar. Para el chip de la UI está
    /// el detalle de cada uno.
    pub fn hub(&self) -> bool {
        self.keyboard || self.mouse
    }
}

/// Lista los dispositivos USB conectados en esta máquina. Lo implementa cada
/// plataforma; puede bloquear unos milisegundos.
pub trait DeviceScanner: Send {
    type Error: Display;

    fn scan(&mut self) -> Result<Vec<UsbId>, Self::Error>;
}

/// Presencia del hub con antirrebote, sin reloj ni E/S: recibe cada lectura con
/// su instante.
#[derive(Debug, Clone)]
pub struct HubTracker {
    cfg: HubConfig,
    debouncer: Debouncer<Presence>,
}

impl HubTracker {
    /// La primera lectura se acepta tal cual: al arrancar no hay nada que filtrar.
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

    /// Devuelve la presencia nueva cuando un cambio se sostuvo lo suficiente.
    pub fn observe(&mut self, devices: &[UsbId], now: Instant) -> Option<Presence> {
        let seen = Presence::from_devices(&self.cfg, devices);
        self.debouncer.observe(seen, now)
    }
}

/// Tiempos del sondeo.
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

/// Sondea el escáner y publica la presencia estable en un `watch`. Quien la
/// recibe se la pasa al [`RoleArbiter`](crate::RoleArbiter) local y la reporta
/// al otro agente.
pub struct HubWatcher<S> {
    scanner: S,
    tracker: HubTracker,
    poll: Duration,
    tx: watch::Sender<Presence>,
}

impl<S: DeviceScanner> HubWatcher<S> {
    /// Hace la primera lectura. Si falla, se arranca como si no hubiera nada
    /// conectado: el antirrebote se encarga cuando el escáner se recupere.
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

    /// Sondea hasta que se suelten todos los receptores. Una lectura fallida no
    /// cuenta como «no hay nada»: se ignora.
    pub async fn run(mut self) {
        let mut tick = tokio::time::interval(self.poll);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = self.tx.closed() => return,
                now = tick.tick() => {
                    let Some(devices) = scan_or_log(&mut self.scanner) else { continue };
                    if let Some(p) = self.tracker.observe(&devices, now.into_std()) {
                        tracing::info!(teclado = p.keyboard, mouse = p.mouse, "cambió el hub");
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
            tracing::warn!("no se pudieron leer los dispositivos USB: {e}");
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
        // El teclado se enumera tarde: es otro cambio, con su propia espera.
        assert_eq!(t.observe(&[MOUSE], at(4)), None);
        assert_eq!(t.observe(&[KB, MOUSE], at(5)), None);
        assert_eq!(t.presence(), p);
    }

    /// Avanza el reloj de tokio de a poco para que el sondeo corra.
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

        // Se conecta el hub.
        devices.set(vec![OTHER, KB, MOUSE]);
        advance(Duration::from_millis(2_500)).await;
        assert!(!rx.has_changed().unwrap(), "cambió antes de 3 s");
        advance(Duration::from_secs(1)).await;
        assert!(rx.has_changed().unwrap());
        assert!(rx.borrow_and_update().hub());

        // Un tirón de 1 s del cable no cambia nada.
        devices.set(vec![OTHER]);
        advance(Duration::from_secs(1)).await;
        devices.set(vec![OTHER, KB, MOUSE]);
        advance(Duration::from_secs(5)).await;
        assert!(!rx.has_changed().unwrap());

        // Lecturas fallidas en medio de una desconexión no la interrumpen ni la
        // adelantan.
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
        assert!(task.is_finished(), "el sondeo sigue sin receptores");
    }

    #[tokio::test(start_paused = true)]
    async fn failed_first_scan_starts_without_hub() {
        let scanner = FakeScanner::new(vec![KB, MOUSE]);
        scanner.handle().fail(true);
        let w = HubWatcher::new(scanner, cfg(), WatchConfig::default());
        assert!(!w.presence().hub());
    }
}
