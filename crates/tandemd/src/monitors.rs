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
//! paused clock does not advance on its own meanwhile. A simulation that wants the bus to cost
//! time hands its clock to [`MonitorHub::charge_bus_time`]: what each call took on the double's
//! clock then passes on tokio's, with the bus held, as it would on a real one (T-342).
//!
//! An input change with the firmware's verdict is a reading of the owner: the input it confirmed is
//! shown at once ([`MonitorControl::confirmed`]) instead of waiting for the next full read of
//! the bus, which re-reads the event ring over DDC and takes seconds on the real 34" (T-342).
//!
//! The 34"'s DP and HDMI share one DDC/CI request buffer: two hosts talking at once can get each
//! other's replies. So a machine with a channel to a monitor the other owns sends nothing to it
//! while the owner is reachable. While the owner is unreachable ([`MonitorHub::set_peer_reachable`])
//! but not yet given up for lost, it reads the live state itself (`0xEE` and `0x60`, never the
//! events) and can switch the input itself; when the owner is lost, the monitor passes to it in
//! the same turn and its reading carries on from the owner's last reported cursor (T-346).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use monitor_ddc::Clock;
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
    /// Whether the other machine's agent is reachable over the control plane. Until told
    /// otherwise it may be talking to the monitors it owns, so nothing is read directly (T-346).
    reachable: AtomicBool,
    /// Wakes the loop early (the owner changed, a change finished).
    wake: Notify,
    /// Only in the simulation: the double's clock, whose time each in-place call pays on tokio's.
    bus_clock: Mutex<Option<Arc<dyn Clock>>>,
    /// The bus is used by one call at a time, also while an in-place call pays its time.
    bus_turn: tokio::sync::Mutex<()>,
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
            reachable: AtomicBool::new(true),
            wake: Notify::new(),
            bus_clock: Mutex::new(None),
            bus_turn: tokio::sync::Mutex::new(()),
        }
    }

    /// Simulation: each in-place call takes on tokio's clock what it took on `clock`, the
    /// double's (paused clock only).
    pub fn charge_bus_time(&self, clock: Arc<dyn Clock>) {
        *lock(&self.bus_clock) = Some(clock);
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

    /// The EDID identity of each monitor this machine sees, channel or not.
    pub fn keys(&self) -> BTreeMap<String, DisplayKey> {
        lock(&self.keys).clone()
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

    /// Whether the other machine's agent is reachable (T-346). While it is not, this machine
    /// reads by itself the monitors the other owns and that it sees with a channel; when it is
    /// again, it stops and the owner's next report replaces that reading.
    pub fn set_peer_reachable(&self, reachable: bool) {
        if self.reachable.swap(reachable, Ordering::Relaxed) != reachable {
            self.wake.notify_one();
        }
    }

    /// The monitors this machine reads directly now: the other owns them and is unreachable.
    fn direct_here(&self) -> BTreeSet<String> {
        if self.reachable.load(Ordering::Relaxed) {
            return BTreeSet::new();
        }
        lock(&self.dir).readable_here()
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
            let clock = lock(&self.bus_clock).clone();
            let Some(clock) = clock else {
                return Some(f(&mut lock(&local.core)));
            };
            let _turn = self.bus_turn.lock().await;
            let started = clock.now();
            let out = f(&mut lock(&local.core));
            tokio::time::sleep(clock.now().saturating_sub(started)).await;
            return Some(out);
        }
        tokio::task::spawn_blocking(move || f(&mut lock(&local.core)))
            .await
            .ok()
    }

    /// One turn: probe (if due), take or release the monitors according to the owner and read the
    /// own monitors (a monitor just taken carries on from the cursor the previous owner reported).
    /// With the other unreachable, it also reads the live state of the ones it owns (T-346).
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
        let direct = self.direct_here();
        let (me, owned, handover) = {
            let mut dir = lock(&self.dir);
            (dir.me(), dir.owned_here(), dir.take_handover())
        };
        let Some((reports, readings)) = self
            .with_core(move |c| {
                c.hand_over(handover);
                c.set_owned(&owned);
                (c.poll(), c.read_direct(&direct))
            })
            .await
        else {
            return tick;
        };
        {
            let mut dir = lock(&self.dir);
            dir.set_reports(me, reports.clone());
            // The owner may have become reachable while reading: its next report is newer.
            if !self.reachable.load(Ordering::Relaxed) {
                dir.set_direct(readings);
            }
        }
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

    fn direct(&self, monitor: &str) -> bool {
        self.direct_here().contains(monitor)
    }

    fn switch<'a>(&'a self, monitor: &'a str, input: MonitorInput) -> StepFuture<'a> {
        Box::pin(async move {
            let id = monitor.to_owned();
            let direct = self.direct(monitor);
            let r = self
                .with_core(move |c| {
                    if direct {
                        c.switch_unowned(&id, input)
                    } else {
                        c.switch(&id, input)
                    }
                })
                .await
                .unwrap_or_else(|| Err(NO_DDC.to_owned()));
            // What happened is seen with the next read: make it now.
            self.wake.notify_one();
            r
        })
    }

    fn confirmed(&self, monitor: &str, input: MonitorInput) -> Option<Vec<MonitorReport>> {
        let mine = {
            let mut dir = lock(&self.dir);
            let owner = dir.owner(monitor)?;
            if dir.input(monitor) == Some(input) {
                return None;
            }
            // Confirmed here without being the owner (T-346): what this machine read moves too.
            dir.set_direct_input(monitor, input);
            if let Some(mut report) = dir.owner_report(monitor).cloned() {
                report.input = Some(input);
                dir.set_report(owner, report);
            }
            owner == dir.me()
        };
        self.publish();
        if !mine {
            return None;
        }
        // The other gets the whole list, as with every reading; the next one that differs goes too.
        let mut sent = lock(&self.sent);
        let entry = sent.iter_mut().find(|r| r.id == monitor)?;
        entry.input = Some(input);
        Some(sent.clone())
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

    /// T-342: the input of a confirmed verdict is shown at once, without another reading of the bus.
    /// The owner also gets back its whole list, with that input, to send it to the other; on the
    /// other machine only what it shows changes.
    #[tokio::test]
    async fn a_confirmed_input_is_shown_without_reading_the_bus() {
        let (a, bus) = hub_with_bus(Machine::A);
        let (b, _) = hub_with_bus(Machine::B);
        let from_a = a.tick(true).await.reports.unwrap();
        b.tick(true).await;
        a.set_peer(Some(b.local_channels()));
        b.set_peer(Some(a.local_channels()));
        b.receive(Machine::A, from_a);
        let id = a.views()[0].id.clone();
        assert_eq!(b.owner(&id), Some(Machine::A));
        bus.clear_ops();

        let sent = a.confirmed(&id, MonitorInput::Hdmi).unwrap();
        assert_eq!(sent[0].input, Some(MonitorInput::Hdmi));
        assert_eq!(a.input(&id), Some(MonitorInput::Hdmi));
        assert_eq!(a.reports_here(), sent);
        assert_eq!(b.confirmed(&id, MonitorInput::Hdmi), None);
        assert_eq!(b.input(&id), Some(MonitorInput::Hdmi));
        assert!(bus.ops().is_empty(), "{:?}", bus.ops());
        // Already shown: nothing to send again.
        assert_eq!(a.confirmed(&id, MonitorInput::Hdmi), None);
        // A monitor nobody reads stays as it is.
        assert_eq!(a.confirmed("other", MonitorInput::Hdmi), None);
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

    /// A hub on a bus another hub may share: the same physical monitor seen by both machines.
    fn hub_on(me: Machine, bus: &FakeBus) -> MonitorHub {
        let hub = MonitorHub::new(me);
        hub.attach(
            LocalMonitors::new(
                Arc::new(bus.clone()),
                Arc::new(bus.clock()),
                system_wall(),
                LocalConfig::default(),
            ),
            true,
        );
        hub
    }

    /// T-346, with both hubs on one monitor: `B` has a channel but `A` owns it (a tie goes to `A`).
    /// While `A` is reachable `B` sends nothing; while it is not, `B` reads the live state itself
    /// (never the events) and switches by itself; when `A` is lost, `B` carries on `A`'s cursor.
    #[tokio::test]
    async fn the_non_owner_reads_and_switches_only_while_the_owner_is_unreachable() {
        let clock = FakeClock::new();
        let bus = FakeBus::new(clock);
        let h = bus.add(FakeMonitorConfig::new(Personality::Standard));
        let (a, b) = (hub_on(Machine::A, &bus), hub_on(Machine::B, &bus));
        a.tick(true).await;
        b.tick(true).await;
        a.set_peer(Some(b.local_channels()));
        b.set_peer(Some(a.local_channels()));
        let id = a.views()[0].id.clone();
        let from_a = a.tick(false).await.reports.unwrap();
        assert!(from_a[0].cursor.is_some(), "{from_a:?}");
        b.receive(Machine::A, from_a);
        assert_eq!(b.owner(&id), Some(Machine::A));

        let quiet = |what: &str| {
            assert!(bus.ops().is_empty(), "{what}: {:?}", bus.ops());
        };
        bus.clear_ops();
        b.tick(false).await;
        quiet("the owner is reachable");
        assert!(!b.direct(&id));

        // The owner becomes unreachable: B reads the live state itself, nothing else.
        bus.osd_switch(h, monitor_ddc::Input::Hdmi);
        bus.clear_ops();
        b.set_peer_reachable(false);
        b.tick(false).await;
        let ops = bus.ops();
        assert!(
            ops.iter().all(|op| matches!(
                op,
                BusOp::Get {
                    code: monitor_ddc::VCP_LIVE | monitor_ddc::VCP_INPUT,
                    ..
                }
            )),
            "{ops:?}"
        );
        assert_eq!(b.input(&id), Some(MonitorInput::Hdmi));
        assert_eq!(b.owner(&id), Some(Machine::A), "the owner does not move");
        // And switches by itself, with the verdict; the confirmed input is shown at once.
        assert!(b.direct(&id));
        b.switch(&id, MonitorInput::DisplayPort).await.unwrap();
        assert_eq!(bus.screen_input(h), Some(monitor_ddc::Input::DisplayPort));
        assert_eq!(b.confirmed(&id, MonitorInput::DisplayPort), None);
        assert_eq!(b.input(&id), Some(MonitorInput::DisplayPort));

        // Reachable again: B stops, and the owner's next report replaces its reading.
        b.set_peer_reachable(true);
        bus.clear_ops();
        b.tick(false).await;
        quiet("reachable again");
        let e = b.switch(&id, MonitorInput::Hdmi).await.unwrap_err();
        assert!(e.contains(tandem_monitor::NOT_OWNER), "{e}");
        let from_a = a.tick(false).await.reports.unwrap();
        b.receive(Machine::A, from_a.clone());
        assert_eq!(b.views()[0].report.as_ref(), Some(&from_a[0]));

        // A is lost before it reads a menu switch: B takes the monitor in the next turn and
        // reads that switch, from A's last cursor.
        bus.osd_switch(h, monitor_ddc::Input::Hdmi);
        b.set_peer(None);
        assert_eq!(b.owner(&id), Some(Machine::B));
        let report = b.tick(false).await.reports.unwrap().remove(0);
        assert!(
            report
                .events
                .iter()
                .any(|e| e.kind == tandem_proto::MonitorEventKind::Input
                    && e.input == Some(MonitorInput::Hdmi)),
            "{:?}",
            report.events
        );
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
