//! Los monitores en el agente (DESIGN §10, T-110).
//!
//! [`MonitorHub`] junta los monitores que ve esta máquina ([`LocalMonitors`], por su bus
//! DDC) con lo que anunció la otra, y de ahí sale el dueño de cada uno
//! ([`tandem_monitor::Directory`]). Solo el dueño lee eventos y estado; se los manda a la otra
//! por el plano de control ([`tandem_proto::ControlMsg::Monitors`]). Los cambios de entrada
//! los hace el orquestador con el dueño ([`MonitorControl`]).
//!
//! El DDC bloquea: en el agente real cada llamada va a un hilo aparte (`spawn_blocking`); en
//! la simulación, con el bus falso que no espera, se llama en el lugar (`inline`), así el reloj
//! pausado de tokio no avanza solo mientras tanto.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tandem_core::{Machine, MonitorInput};
use tandem_monitor::{Directory, LocalMonitors};
use tandem_proto::{MonitorChannel, MonitorReport, MonitorView};
use tokio::sync::{Notify, watch};

use crate::orchestrator::{MonitorControl, StepFuture};

/// Lo que dice el módulo de monitores en una máquina sin acceso a DDC.
pub const NO_DDC: &str = "Sin canal DDC en esta máquina";

/// Cada cuánto se mira el bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorTimes {
    /// Volver a sondear: canales nuevos o perdidos (un monitor que se apagó).
    pub scan_every: Duration,
    /// Leer eventos y estado de los monitores propios.
    pub poll_every: Duration,
}

impl Default for MonitorTimes {
    fn default() -> Self {
        MonitorTimes {
            scan_every: Duration::from_secs(5),
            poll_every: Duration::from_secs(1),
        }
    }
}

#[derive(Clone)]
struct Local {
    core: Arc<Mutex<LocalMonitors>>,
    inline: bool,
}

/// Lo que dejó una vuelta del bucle de monitores.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tick {
    /// Cambiaron los canales de esta máquina: hay que anunciarlo.
    pub channels_changed: bool,
    /// El sistema no dejó enumerar los monitores.
    pub scan_error: Option<String>,
    /// Lo leído de los monitores propios, si cambió desde el último envío.
    pub reports: Option<Vec<MonitorReport>>,
}

pub struct MonitorHub {
    dir: Mutex<Directory>,
    local: Mutex<Option<Local>>,
    views: watch::Sender<Vec<MonitorView>>,
    /// Lo último que se mandó a la otra máquina.
    sent: Mutex<Vec<MonitorReport>>,
    /// Despierta el bucle antes de tiempo (cambió el dueño, terminó un cambio).
    wake: Notify,
}

impl std::fmt::Debug for MonitorHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MonitorHub")
            .field("views", &*self.views.borrow())
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl MonitorHub {
    pub fn new(me: Machine) -> Self {
        MonitorHub {
            dir: Mutex::new(Directory::new(me)),
            local: Mutex::new(None),
            views: watch::Sender::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            wake: Notify::new(),
        }
    }

    /// El bus de esta máquina. `inline`: llamar en el lugar (solo con un bus que no bloquea).
    pub fn attach(&self, core: LocalMonitors, inline: bool) {
        *lock(&self.local) = Some(Local {
            core: Arc::new(Mutex::new(core)),
            inline,
        });
    }

    pub fn has_bus(&self) -> bool {
        lock(&self.local).is_some()
    }

    /// Esta máquina resultó ser el otro lado.
    pub fn set_me(&self, me: Machine) {
        lock(&self.dir).set_me(me);
        self.publish();
    }

    /// Los canales de esta máquina, para su anuncio.
    pub fn local_channels(&self) -> Vec<MonitorChannel> {
        lock(&self.dir).local().to_vec()
    }

    /// Lo que anunció la otra, mientras está prendida; `None` si no. Si cambian los dueños,
    /// el bucle se entera enseguida.
    pub fn set_peer(&self, channels: Option<Vec<MonitorChannel>>) {
        if lock(&self.dir).set_peer(channels) {
            self.publish();
            self.wake.notify_one();
        }
    }

    /// Lo que leyó la otra de los monitores que son suyos.
    pub fn receive(&self, from: Machine, reports: Vec<MonitorReport>) {
        {
            let mut dir = lock(&self.dir);
            for r in reports {
                dir.set_report(from, r);
            }
        }
        self.publish();
    }

    pub fn views(&self) -> Vec<MonitorView> {
        self.views.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Vec<MonitorView>> {
        self.views.subscribe()
    }

    /// Lo último que se leyó de los monitores propios (para mandarlo al aparecer un enlace).
    pub fn reports_here(&self) -> Vec<MonitorReport> {
        lock(&self.sent).clone()
    }

    /// Espera a que algo pida una vuelta antes de tiempo.
    pub async fn woken(&self) {
        self.wake.notified().await;
    }

    fn publish(&self) {
        let views = lock(&self.dir).views();
        self.views.send_if_modified(|v| {
            let changed = *v != views;
            *v = views;
            changed
        });
    }

    /// Corre `f` con el bus: en el lugar o en un hilo aparte. `None` sin bus.
    async fn with_core<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut LocalMonitors) -> T + Send + 'static,
    ) -> Option<T> {
        let local = lock(&self.local).clone()?;
        if local.inline {
            return Some(f(&mut lock(&local.core)));
        }
        tokio::task::spawn_blocking(move || f(&mut lock(&local.core)))
            .await
            .ok()
    }

    /// Una vuelta: sondear (si toca), tomar o soltar los monitores según el dueño y leer los
    /// propios.
    pub async fn tick(&self, scan: bool) -> Tick {
        let mut tick = Tick::default();
        if scan {
            match self.with_core(|c| c.scan()).await {
                Some(Ok(channels)) => tick.channels_changed = lock(&self.dir).set_local(channels),
                Some(Err(e)) => {
                    tick.channels_changed = lock(&self.dir).set_local(Vec::new());
                    tick.scan_error = Some(e.to_string());
                }
                None => return tick,
            }
        }
        let (me, owned) = {
            let dir = lock(&self.dir);
            (dir.me(), dir.owned_here())
        };
        let Some(reports) = self
            .with_core(move |c| {
                c.set_owned(&owned);
                c.poll()
            })
            .await
        else {
            return tick;
        };
        {
            let mut dir = lock(&self.dir);
            for r in &reports {
                dir.set_report(me, r.clone());
            }
        }
        self.publish();
        let mut sent = lock(&self.sent);
        if *sent != reports {
            *sent = reports.clone();
            tick.reports = Some(reports);
        }
        tick
    }
}

impl MonitorControl for MonitorHub {
    fn owner(&self, monitor: &str) -> Option<Machine> {
        lock(&self.dir).owner(monitor)
    }

    fn input(&self, monitor: &str) -> Option<MonitorInput> {
        lock(&self.dir).input(monitor)
    }

    fn switch<'a>(&'a self, monitor: &'a str, input: MonitorInput) -> StepFuture<'a> {
        Box::pin(async move {
            let id = monitor.to_owned();
            let r = self
                .with_core(move |c| c.switch(&id, input))
                .await
                .unwrap_or_else(|| Err(NO_DDC.to_owned()));
            // Lo que pasó se ve con la próxima lectura: que sea ya.
            self.wake.notify_one();
            r
        })
    }
}

#[cfg(test)]
mod tests {
    use monitor_ddc::fake::{BusOp, FakeBus, FakeClock, FakeMonitorConfig, Personality};
    use tandem_monitor::LocalConfig;
    use tandem_monitor::local::system_wall;

    use super::*;

    fn hub_with_bus(me: Machine) -> (MonitorHub, FakeBus) {
        let clock = FakeClock::new();
        let bus = FakeBus::new(clock.clone());
        bus.add(FakeMonitorConfig::new(Personality::Standard));
        let hub = MonitorHub::new(me);
        hub.attach(
            LocalMonitors::new(
                Arc::new(bus.clone()),
                Arc::new(clock),
                system_wall(),
                LocalConfig::default(),
            ),
            true,
        );
        (hub, bus)
    }

    #[tokio::test]
    async fn alone_this_machine_owns_what_it_sees_and_reads_it() {
        let (hub, _) = hub_with_bus(Machine::A);
        let tick = hub.tick(true).await;
        assert!(tick.channels_changed);
        let reports = tick.reports.unwrap();
        assert_eq!(reports[0].input, Some(MonitorInput::DisplayPort));
        let views = hub.views();
        assert_eq!(views[0].owner, Some(Machine::A));
        // Nada cambió: no se vuelve a mandar.
        assert_eq!(hub.tick(false).await, Tick::default());
    }

    #[tokio::test]
    async fn a_faster_peer_takes_the_monitor_and_this_one_stops_reading() {
        let (hub, bus) = hub_with_bus(Machine::A);
        hub.tick(true).await;
        let mut theirs = hub.local_channels();
        theirs[0].latency_ms = 0;
        let mut mine = theirs[0].clone();
        mine.latency_ms = 100;
        lock(&hub.dir).set_local(vec![mine]);
        hub.set_peer(Some(theirs));
        let id = hub.views()[0].id.clone();
        assert_eq!(hub.owner(&id), Some(Machine::B));
        bus.clear_ops();
        hub.tick(false).await;
        assert!(
            bus.ops().is_empty(),
            "sin dueño aquí no se lee: {:?}",
            bus.ops()
        );
        let e = hub.switch(&id, MonitorInput::Hdmi).await.unwrap_err();
        assert!(e.contains(tandem_monitor::NOT_OWNER), "{e}");
        assert!(
            !bus.ops().iter().any(|op| matches!(op, BusOp::Set { .. })),
            "{:?}",
            bus.ops()
        );
        // La otra se fue: vuelve aquí.
        hub.set_peer(None);
        assert_eq!(hub.owner(&id), Some(Machine::A));
        hub.tick(false).await;
        hub.switch(&id, MonitorInput::Hdmi).await.unwrap();
        let reports = hub.tick(false).await.reports.unwrap();
        assert_eq!(reports[0].input, Some(MonitorInput::Hdmi));
        assert_eq!(hub.input(&id), Some(MonitorInput::Hdmi));
    }

    #[tokio::test]
    async fn without_a_bus_nothing_is_seen() {
        let hub = MonitorHub::new(Machine::B);
        assert!(!hub.has_bus());
        assert_eq!(hub.tick(true).await, Tick::default());
        let e = hub.switch("x", MonitorInput::Hdmi).await.unwrap_err();
        assert_eq!(e, NO_DDC);
        assert!(hub.views().is_empty());
    }

    #[tokio::test]
    async fn reports_from_the_peer_show_while_it_owns_the_monitor() {
        let hub = MonitorHub::new(Machine::A);
        let (theirs, _) = hub_with_bus(Machine::B);
        let reports = theirs.tick(true).await.reports.unwrap();
        hub.set_peer(Some(theirs.local_channels()));
        hub.receive(Machine::B, reports);
        let views = hub.views();
        assert_eq!(views[0].owner, Some(Machine::B));
        assert_eq!(
            views[0].report.as_ref().and_then(|r| r.input),
            Some(MonitorInput::DisplayPort)
        );
    }
}
