//! The monitors in the agent (DESIGN §10, T-110).
//!
//! [`MonitorHub`] brings together the monitors this machine sees ([`LocalMonitors`], over its DDC
//! bus) with what the other announced, and from there comes each one's owner
//! ([`tandem_monitor::Directory`]). Only the owner reads events and state; it sends them to the other
//! over the control plane ([`tandem_proto::ControlMsg::Monitors`]). Input changes
//! are done by the orchestrator with the owner ([`MonitorControl`]).
//!
//! DDC blocks: in the real agent each call goes to a separate thread (`spawn_blocking`); in
//! the simulation, with the fake bus that does not wait, it is called in place (`inline`), so tokio's
//! paused clock does not advance on its own meanwhile.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tandem_core::{Desk, DisplayKey, Inputs, Machine, MonitorInput, link_screens};
use tandem_monitor::{Directory, LocalMonitors};
use tandem_proto::{MonitorChannel, MonitorReport, MonitorView};
use tokio::sync::{Notify, watch};

use crate::orchestrator::{MonitorControl, StepFuture};

/// What the monitors module says on a machine without DDC access.
pub const NO_DDC: &str = "No DDC channel on this machine";

/// Which input of `monitor` each machine goes to: its `[[cables]]` of the desk or, if not, what
/// the EDID each one reads says (`view.via`).
pub fn inputs(desk: &Desk, monitor: &str, view: Option<&MonitorView>) -> Inputs {
    Inputs::resolve(desk.cable(monitor), |m| {
        view.and_then(|v| v.via.iter().find(|(w, _)| *w == m).map(|(_, i)| *i))
    })
}

/// Which machine each compatible monitor shows: the real input its owner read and which
/// machine that input goes to. One missing something does not appear.
pub fn shows(views: &[MonitorView], desk: &Desk) -> BTreeMap<String, Machine> {
    views
        .iter()
        .filter(|v| v.compat.is_compatible())
        .filter_map(|v| {
            let input = v.report.as_ref()?.input?;
            let machine = inputs(desk, &v.id, Some(v)).machine_on(input)?;
            Some((v.id.clone(), machine))
        })
        .collect()
}

/// How often the bus is looked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorTimes {
    /// Probe again: new or lost channels (a monitor that turned off).
    pub scan_every: Duration,
    /// Read events and state of the own monitors.
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

/// What a turn of the monitors loop left.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tick {
    /// This machine's channels changed: it has to be announced.
    pub channels_changed: bool,
    /// The system did not let the monitors be enumerated.
    pub scan_error: Option<String>,
    /// What was read from the own monitors, if it changed since the last send.
    pub reports: Option<Vec<MonitorReport>>,
}

pub struct MonitorHub {
    dir: Mutex<Directory>,
    /// The EDID identity of each monitor this machine sees, channel or not (T-340).
    keys: Mutex<BTreeMap<String, DisplayKey>>,
    /// This machine's screens (`screen-N`) with their EDID identity.
    screens: Mutex<Vec<(String, DisplayKey)>>,
    local: Mutex<Option<Local>>,
    views: watch::Sender<Vec<MonitorView>>,
    /// The last thing sent to the other machine.
    sent: Mutex<Vec<MonitorReport>>,
    /// The other's channels changed: the next turn sends what it reads even if it did not change.
    resend: AtomicBool,
    /// Wakes the loop early (the owner changed, a change finished).
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
            keys: Mutex::new(BTreeMap::new()),
            screens: Mutex::new(Vec::new()),
            local: Mutex::new(None),
            views: watch::Sender::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            resend: AtomicBool::new(false),
            wake: Notify::new(),
        }
    }

    /// This machine's bus. `inline`: call in place (only with a bus that does not block).
    pub fn attach(&self, core: LocalMonitors, inline: bool) {
        *lock(&self.local) = Some(Local {
            core: Arc::new(Mutex::new(core)),
            inline,
        });
    }

    pub fn has_bus(&self) -> bool {
        lock(&self.local).is_some()
    }

    /// This machine turned out to be the other side.
    pub fn set_me(&self, me: Machine) {
        lock(&self.dir).set_me(me);
        self.publish();
    }

    /// This machine's screens with the EDID identity the system gives for each one (the input
    /// engine reads them). `true` if they changed.
    pub fn set_screens(&self, screens: Vec<(String, DisplayKey)>) -> bool {
        let mut current = lock(&self.screens);
        let changed = *current != screens;
        *current = screens;
        changed
    }

    /// Which of this machine's screens each monitor it sees is (T-340).
    pub fn screen_links(&self) -> BTreeMap<String, String> {
        link_screens(&lock(&self.keys), &lock(&self.screens))
    }

    /// This machine's channels, for its announcement.
    pub fn local_channels(&self) -> Vec<MonitorChannel> {
        lock(&self.dir).local().to_vec()
    }

    /// What the other announced, while it is on; `None` if not. If the owners change,
    /// the loop finds out right away. The other may have stored a reading of its own before it
    /// knew this machine's channels (T-280), so this machine's reports go out again.
    pub fn set_peer(&self, channels: Option<Vec<MonitorChannel>>) {
        if lock(&self.dir).set_peer(channels) {
            self.resend.store(true, Ordering::Relaxed);
            self.publish();
            self.wake.notify_one();
        }
    }

    /// What the other read from the monitors that are its own: its whole list, so what is
    /// missing from it is no longer read there.
    pub fn receive(&self, from: Machine, reports: Vec<MonitorReport>) {
        lock(&self.dir).set_reports(from, reports);
        self.publish();
    }

    pub fn views(&self) -> Vec<MonitorView> {
        self.views.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Vec<MonitorView>> {
        self.views.subscribe()
    }

    /// The last thing read from the own monitors (to send it when a link appears).
    pub fn reports_here(&self) -> Vec<MonitorReport> {
        lock(&self.sent).clone()
    }

    /// Waits for something to request a turn early.
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

    /// Runs `f` with the bus: in place or on a separate thread. `None` without a bus.
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

    /// One turn: probe (if due), take or release the monitors according to the owner and read the
    /// own monitors.
    pub async fn tick(&self, scan: bool) -> Tick {
        let mut tick = Tick::default();
        if scan {
            match self.with_core(|c| c.scan().map(|ch| (ch, c.keys()))).await {
                Some(Ok((channels, keys))) => {
                    *lock(&self.keys) = keys;
                    tick.channels_changed = lock(&self.dir).set_local(channels);
                }
                Some(Err(e)) => {
                    lock(&self.keys).clear();
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
        lock(&self.dir).set_reports(me, reports.clone());
        self.publish();
        let resend = self.resend.swap(false, Ordering::Relaxed);
        let mut sent = lock(&self.sent);
        if *sent != reports || resend {
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
            // What happened is seen with the next read: make it now.
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
        // Nothing changed: it is not sent again.
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
            "without an owner here nothing is read: {:?}",
            bus.ops()
        );
        let e = hub.switch(&id, MonitorInput::Hdmi).await.unwrap_err();
        assert!(e.contains(tandem_monitor::NOT_OWNER), "{e}");
        assert!(
            !bus.ops().iter().any(|op| matches!(op, BusOp::Set { .. })),
            "{:?}",
            bus.ops()
        );
        // The other left: it returns here.
        hub.set_peer(None);
        assert_eq!(hub.owner(&id), Some(Machine::A));
        hub.tick(false).await;
        hub.switch(&id, MonitorInput::Hdmi).await.unwrap();
        let reports = hub.tick(false).await.reports.unwrap();
        assert_eq!(reports[0].input, Some(MonitorInput::Hdmi));
        assert_eq!(hub.input(&id), Some(MonitorInput::Hdmi));
    }

    #[tokio::test]
    async fn what_each_monitor_shows_comes_from_the_real_input_and_the_cables() {
        let (hub, _) = hub_with_bus(Machine::A);
        hub.tick(true).await;
        let views = hub.views();
        let id = views[0].id.clone();
        let mut desk = Desk::default();
        // Without a cable or an EDID that says so, it is not known.
        assert!(shows(&views, &desk).is_empty());
        // A's cable goes to DP (the input on screen): it shows A.
        let mut cable = tandem_core::MonitorCable::new(id.clone());
        cable.a.input = Some(MonitorInput::DisplayPort);
        desk.cables.push(cable);
        assert_eq!(shows(&views, &desk).get(&id), Some(&Machine::A));
        // With what B's EDID says (DP) and without a cable: it shows B.
        let mut seen = views.clone();
        seen[0].via = vec![(Machine::B, MonitorInput::DisplayPort)];
        assert_eq!(shows(&seen, &Desk::default()).get(&id), Some(&Machine::B));
        assert_eq!(
            inputs(&Desk::default(), &id, Some(&seen[0])).of(Machine::A),
            Some(MonitorInput::Hdmi)
        );
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

    /// T-280, with each hub's own loop: `A` reads first and sends its report; `B` reads the same
    /// monitor before it knows `A`'s channel (it believes it owns it) and sends its own; then the
    /// channels cross and `A` is elected. `A` reads nothing new, so it does not send again: `B`
    /// must keep showing `A`'s report.
    #[tokio::test]
    async fn the_owners_report_survives_an_early_claim_of_the_other() {
        let (a, _) = hub_with_bus(Machine::A);
        let (b, _) = hub_with_bus(Machine::B);
        let from_a = a.tick(true).await.reports.unwrap();
        b.receive(Machine::A, from_a);
        let from_b = b.tick(true).await.reports.unwrap();
        a.receive(Machine::B, from_b);
        a.set_peer(Some(b.local_channels()));
        b.set_peer(Some(a.local_channels()));
        for _ in 0..3 {
            for (hub, other, from) in [(&a, &b, Machine::A), (&b, &a, Machine::B)] {
                if let Some(r) = hub.tick(false).await.reports {
                    other.receive(from, r);
                }
            }
        }
        for hub in [&a, &b] {
            let views = hub.views();
            assert_eq!(views[0].owner, Some(Machine::A));
            assert_eq!(
                views[0].report.as_ref().and_then(|r| r.input),
                Some(MonitorInput::DisplayPort),
                "{:?}",
                hub.dir
            );
        }
    }

    /// Review of #183: `B` reports while it owns the monitor alone, `A` is elected, `B` stops
    /// reading and sends `[]`; when `A` then loses its channel, it shows no report until `B`
    /// reads and sends again.
    #[tokio::test]
    async fn a_former_reading_of_the_other_is_not_shown_after_the_owner_leaves() {
        let (a, _) = hub_with_bus(Machine::A);
        let (b, _) = hub_with_bus(Machine::B);
        let from_b = b.tick(true).await.reports.unwrap();
        a.tick(true).await;
        a.receive(Machine::B, from_b);
        a.set_peer(Some(b.local_channels()));
        b.set_peer(Some(a.local_channels()));
        let id = a.views()[0].id.clone();
        assert_eq!(a.owner(&id), Some(Machine::A));
        a.tick(false).await;
        let from_b = b.tick(false).await.reports.unwrap();
        assert!(from_b.is_empty(), "B no longer owns it: {from_b:?}");
        a.receive(Machine::B, from_b);
        // A's channel goes away (unplugged): B is the owner.
        lock(&a.dir).set_local(Vec::new());
        b.set_peer(Some(Vec::new()));
        a.publish();
        assert_eq!(a.owner(&id), Some(Machine::B));
        assert_eq!(a.input(&id), None, "{:?}", a.dir);
        assert_eq!(a.views()[0].report, None);
        let from_b = b.tick(false).await.reports.unwrap();
        a.receive(Machine::B, from_b);
        assert_eq!(a.input(&id), Some(MonitorInput::DisplayPort));
    }

    #[tokio::test]
    async fn new_channels_from_the_other_send_the_reports_again() {
        let (a, _) = hub_with_bus(Machine::A);
        let (b, _) = hub_with_bus(Machine::B);
        assert!(a.tick(true).await.reports.is_some());
        assert_eq!(a.tick(false).await.reports, None, "nothing changed");
        b.tick(true).await;
        a.set_peer(Some(b.local_channels()));
        let again = a.tick(false).await.reports.unwrap();
        assert_eq!(again[0].input, Some(MonitorInput::DisplayPort));
        assert_eq!(a.tick(false).await.reports, None, "only once");
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
