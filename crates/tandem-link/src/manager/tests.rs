//! The manager with a transport double and the tokio clock paused: the
//! measured times are exact and there are no real waits.

use std::collections::HashMap;
use std::sync::Mutex;

use super::*;

/// What a simulated link does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wire {
    Up,
    /// Cable disconnected: nothing answers or fails, just silence.
    Cut,
    /// The peer rejects the connection instantly.
    Refused,
}

#[derive(Debug, thiserror::Error)]
#[error("rejected")]
struct Refused;

#[derive(Default)]
struct FakeNet {
    wires: Mutex<HashMap<SocketAddr, Wire>>,
    connects: Mutex<u32>,
}

impl FakeNet {
    fn set(&self, addr: SocketAddr, wire: Wire) {
        self.wires.lock().unwrap().insert(addr, wire);
    }

    fn get(&self, addr: SocketAddr) -> Wire {
        *self.wires.lock().unwrap().get(&addr).unwrap_or(&Wire::Cut)
    }
}

struct FakeProber(Arc<FakeNet>);

/// Handle = (address, connection number), to see reconnections.
type Handle = (SocketAddr, u32);

impl Prober for FakeProber {
    type Session = Handle;
    type Handle = Handle;
    type Error = Refused;

    async fn connect(&self, addr: SocketAddr) -> Result<Handle, Refused> {
        match self.0.get(addr) {
            Wire::Up => {
                let mut n = self.0.connects.lock().unwrap();
                *n += 1;
                Ok((addr, *n))
            }
            Wire::Refused => Err(Refused),
            Wire::Cut => std::future::pending().await,
        }
    }

    async fn ping(&self, s: &mut Handle) -> Result<(), Refused> {
        match self.0.get(s.0) {
            Wire::Up => {
                time::sleep(Duration::from_millis(2)).await;
                Ok(())
            }
            Wire::Refused => Err(Refused),
            Wire::Cut => std::future::pending().await,
        }
    }

    fn handle(s: &Handle) -> Handle {
        *s
    }
}

fn addr(kind: LinkKind) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 40000 + u16::from(kind.priority())))
}

fn cands(kinds: &[LinkKind]) -> Vec<LinkCandidate> {
    kinds
        .iter()
        .map(|&kind| LinkCandidate {
            kind,
            addr: addr(kind),
        })
        .collect()
}

struct Rig {
    net: Arc<FakeNet>,
    manager: LinkManager<Handle>,
    events: mpsc::UnboundedReceiver<LinkEvent>,
    seen: Vec<LinkEvent>,
}

impl Rig {
    fn new(kinds: &[LinkKind], up: &[LinkKind]) -> Self {
        let net = Arc::new(FakeNet::default());
        for k in up {
            net.set(addr(*k), Wire::Up);
        }
        let (manager, events) = LinkManager::spawn(
            &cands(kinds),
            FakeProber(net.clone()),
            HealthConfig::default(),
        )
        .unwrap();
        Self {
            net,
            manager,
            events,
            seen: Vec::new(),
        }
    }

    async fn next(&mut self) -> LinkEvent {
        let e = time::timeout(Duration::from_secs(30), self.events.recv())
            .await
            .expect("no event arrived")
            .expect("the manager stopped");
        self.seen.push(e);
        e
    }

    async fn until_active(&mut self, kind: Option<LinkKind>) -> Duration {
        let start = Instant::now();
        while self.next().await.to != kind {}
        start.elapsed()
    }
}

/// Worst case of the design: it fails right after a good ping, so it is noticed
/// on the next one (1 s) plus the Pong's maximum time (0.25 s).
const MAX_SWITCH: Duration = Duration::from_millis(1500);

#[tokio::test(start_paused = true)]
async fn preferred_dropping_switches_in_under_1_5_s_and_comes_back() {
    use LinkKind::*;
    let mut rig = Rig::new(&ALL_KINDS, &ALL_KINDS);
    rig.until_active(Some(Usb4)).await;
    let first_usb4 = rig.manager.active().unwrap().handle;

    rig.net.set(addr(Usb4), Wire::Cut);
    let took = rig.until_active(Some(Ethernet)).await;
    assert!(took < MAX_SWITCH, "switched in {took:?}");
    assert_eq!(rig.manager.active().unwrap().kind, Ethernet);

    rig.net.set(addr(Usb4), Wire::Up);
    let took = rig.until_active(Some(Usb4)).await;
    // Until the next ping (1 s) and then 3 s of good pings (hysteresis).
    let settle = HealthConfig::default().settle;
    assert!(
        took >= settle && took <= settle + Duration::from_millis(1100),
        "came back in {took:?}"
    );
    // It is a new connection, not the one that was cut.
    assert_ne!(rig.manager.active().unwrap().handle, first_usb4);
}

const ALL_KINDS: [LinkKind; 3] = LinkKind::ALL;

/// Cuts and restores USB4 every 350 ms (a 700 ms cycle) for `cycles` cycles.
async fn flap(net: &FakeNet, cycles: u32) {
    for _ in 0..cycles {
        net.set(addr(LinkKind::Usb4), Wire::Cut);
        time::sleep(Duration::from_millis(350)).await;
        net.set(addr(LinkKind::Usb4), Wire::Up);
        time::sleep(Duration::from_millis(350)).await;
    }
}

/// Events that already arrived, without waiting for more.
fn drain(rig: &mut Rig) -> Vec<LinkEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rig.events.try_recv() {
        rig.seen.push(e);
        out.push(e);
    }
    out
}

#[tokio::test(start_paused = true)]
async fn a_flickering_usb4_does_not_move_the_link_and_returns_after_settling() {
    use LinkKind::*;
    // At different phases relative to the ping.
    for offset_ms in [0u64, 100, 250, 400, 600, 999] {
        let mut rig = Rig::new(&ALL_KINDS, &ALL_KINDS);
        rig.until_active(Some(Usb4)).await;
        time::sleep(Duration::from_millis(offset_ms)).await;

        // The first ping that falls in a cut moves to Ethernet; after that, 30 s of
        // flickering without any change.
        let net = rig.net.clone();
        let flapping = tokio::spawn(async move { flap(&net, 43).await });
        rig.until_active(Some(Ethernet)).await;
        flapping.await.unwrap();
        let during = drain(&mut rig);
        assert!(during.is_empty(), "offset {offset_ms} ms: {during:?}");
        assert_eq!(rig.manager.active().unwrap().kind, Ethernet);

        // Still: it returns to USB4 at most after the next ping and 3 s of good
        // pings. It may be sooner: a cut between two pings is not seen, so the
        // streak may have started during the last cycles.
        let took = rig.until_active(Some(Usb4)).await;
        let settle = HealthConfig::default().settle;
        assert!(
            took <= settle + Duration::from_millis(1100),
            "offset {offset_ms} ms: came back in {took:?}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn with_a_transaction_in_progress_it_only_switches_if_the_active_one_drops() {
    use LinkKind::*;
    let mut rig = Rig::new(&ALL_KINDS, &ALL_KINDS);
    rig.until_active(Some(Usb4)).await;
    rig.net.set(addr(Usb4), Wire::Cut);
    rig.until_active(Some(Ethernet)).await;

    let hold = rig.manager.hold();
    let second = rig.manager.hold();
    rig.net.set(addr(Usb4), Wire::Up);
    time::sleep(Duration::from_secs(10)).await;
    assert!(drain(&mut rig).is_empty());
    assert_eq!(rig.manager.active().unwrap().kind, Ethernet);

    // It does not go up until the last hold is released; then, right away.
    drop(second);
    time::sleep(Duration::from_millis(1)).await;
    assert!(drain(&mut rig).is_empty());
    let start = Instant::now();
    drop(hold);
    rig.until_active(Some(Usb4)).await;
    assert!(start.elapsed() < Duration::from_millis(10));

    // With a hold, if the active one drops it switches anyway.
    let _hold = rig.manager.hold();
    rig.net.set(addr(Usb4), Wire::Cut);
    let took = rig.until_active(Some(Ethernet)).await;
    assert!(took < MAX_SWITCH, "{took:?}");
}

#[tokio::test(start_paused = true)]
async fn the_switch_respects_the_budget_at_any_phase_of_the_ping() {
    // Cuts the cable at different moments between two pings.
    for offset_ms in [0u64, 1, 150, 500, 900, 999, 1000, 1001, 1299] {
        let mut rig = Rig::new(&ALL_KINDS, &ALL_KINDS);
        rig.until_active(Some(LinkKind::Usb4)).await;
        time::sleep(Duration::from_millis(offset_ms)).await;
        rig.net.set(addr(LinkKind::Usb4), Wire::Cut);
        let took = rig.until_active(Some(LinkKind::Ethernet)).await;
        assert!(took < MAX_SWITCH, "cut at +{offset_ms} ms: {took:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn an_immediate_rejection_counts_as_a_drop() {
    let mut rig = Rig::new(&ALL_KINDS, &ALL_KINDS);
    rig.until_active(Some(LinkKind::Usb4)).await;
    rig.net.set(addr(LinkKind::Usb4), Wire::Refused);
    let took = rig.until_active(Some(LinkKind::Ethernet)).await;
    assert!(took <= Duration::from_secs(1), "{took:?}");
}

#[tokio::test(start_paused = true)]
async fn with_only_wifi_everything_goes_over_wifi() {
    let mut rig = Rig::new(&ALL_KINDS, &[LinkKind::Wifi]);
    rig.until_active(Some(LinkKind::Wifi)).await;
    // Without the rule "Wi-Fi carries no data" (DECISIONS A2): the active one is for everything.
    let active = rig.manager.active().unwrap();
    assert_eq!(active.kind, LinkKind::Wifi);
    assert_eq!(active.addr, addr(LinkKind::Wifi));

    // When a cable connects, everything moves to it.
    rig.net.set(addr(LinkKind::Ethernet), Wire::Up);
    let e = rig.next().await;
    assert_eq!(e.to, Some(LinkKind::Ethernet));
    assert_eq!(rig.manager.active().unwrap().kind, LinkKind::Ethernet);
}

#[tokio::test(start_paused = true)]
async fn without_links_there_is_no_active_one() {
    let rig = Rig::new(&ALL_KINDS, &[]);
    time::sleep(Duration::from_secs(5)).await;
    assert_eq!(rig.manager.active(), None);
    assert!(rig.manager.links().iter().all(|l| l.rtt.is_none()));
}

#[tokio::test(start_paused = true)]
async fn events_come_out_in_order_and_chained() {
    use LinkKind::*;
    let mut rig = Rig::new(&ALL_KINDS, &[Wifi]);
    rig.until_active(Some(Wifi)).await;
    rig.net.set(addr(Ethernet), Wire::Up);
    rig.until_active(Some(Ethernet)).await;
    rig.net.set(addr(Usb4), Wire::Up);
    rig.until_active(Some(Usb4)).await;
    rig.net.set(addr(Usb4), Wire::Cut);
    rig.net.set(addr(Ethernet), Wire::Cut);
    rig.until_active(Some(Wifi)).await;
    rig.net.set(addr(Wifi), Wire::Cut);
    rig.until_active(None).await;
    rig.net.set(addr(Usb4), Wire::Up);
    rig.until_active(Some(Usb4)).await;

    for (i, e) in rig.seen.iter().enumerate() {
        assert_eq!(e.seq, i as u64);
        let prev = i.checked_sub(1).and_then(|p| rig.seen[p].to);
        assert_eq!(e.from, prev, "event {i}: {e:?}");
    }
    let tos: Vec<_> = rig.seen.iter().map(|e| e.to).collect();
    assert_eq!(tos.first(), Some(&Some(Wifi)));
    assert_eq!(tos.last(), Some(&Some(Usb4)));
    assert!(tos.contains(&None));
}

#[tokio::test(start_paused = true)]
async fn the_snapshot_shows_each_links_health() {
    let mut rig = Rig::new(&[LinkKind::Wifi, LinkKind::Ethernet], &[LinkKind::Ethernet]);
    rig.until_active(Some(LinkKind::Ethernet)).await;
    let snap = rig.manager.snapshot();
    assert!(snap.active.is_some());
    assert_eq!(
        snap.links.iter().map(|l| l.kind).collect::<Vec<_>>(),
        [LinkKind::Ethernet, LinkKind::Wifi]
    );
    assert_eq!(snap.links[0].rtt, Some(Duration::from_millis(2)));
    assert_eq!(snap.links[1].rtt, None);

    let mut watch = rig.manager.watch();
    rig.net.set(addr(LinkKind::Ethernet), Wire::Cut);
    let snap = time::timeout(
        Duration::from_secs(5),
        watch.wait_for(|s| s.active.is_none()),
    )
    .await
    .unwrap()
    .unwrap()
    .clone();
    assert!(snap.active.is_none());
}

#[tokio::test(start_paused = true)]
async fn dropping_the_manager_stops_the_pings() {
    let mut rig = Rig::new(&ALL_KINDS, &ALL_KINDS);
    rig.until_active(Some(LinkKind::Usb4)).await;
    let Rig {
        net,
        manager,
        mut events,
        ..
    } = rig;
    drop(manager);
    // The event queue closes and no more connections are opened.
    assert!(
        time::timeout(Duration::from_secs(5), async {
            while events.recv().await.is_some() {}
        })
        .await
        .is_ok()
    );
    let before = *net.connects.lock().unwrap();
    net.set(addr(LinkKind::Usb4), Wire::Cut);
    time::sleep(Duration::from_secs(5)).await;
    net.set(addr(LinkKind::Usb4), Wire::Up);
    time::sleep(Duration::from_secs(5)).await;
    assert_eq!(*net.connects.lock().unwrap(), before);
}

#[tokio::test]
async fn an_invalid_config_is_rejected() {
    let net = Arc::new(FakeNet::default());
    let bad = HealthConfig {
        ping_interval: Duration::ZERO,
        ..HealthConfig::default()
    };
    assert_eq!(
        LinkManager::spawn(&cands(&ALL_KINDS), FakeProber(net.clone()), bad).err(),
        Some(ManagerError::ZeroDuration)
    );
    assert_eq!(
        LinkManager::spawn(&[], FakeProber(net.clone()), HealthConfig::default()).err(),
        Some(ManagerError::NoCandidates)
    );
    let dup = cands(&[LinkKind::Usb4, LinkKind::Usb4]);
    assert_eq!(
        LinkManager::spawn(&dup, FakeProber(net), HealthConfig::default()).err(),
        Some(ManagerError::DuplicateLink(LinkKind::Usb4))
    );
}

#[test]
fn the_default_budget_meets_the_design() {
    let c = HealthConfig::default();
    assert_eq!(c.ping_interval, Duration::from_secs(1));
    assert!(c.ping_interval + c.ping_timeout < MAX_SWITCH);
}

#[tokio::test(start_paused = true)]
async fn a_link_that_appears_is_pinged_and_becomes_the_active_one() {
    let mut rig = Rig::new(&[LinkKind::Wifi], &[LinkKind::Wifi]);
    rig.until_active(Some(LinkKind::Wifi)).await;

    rig.net.set(addr(LinkKind::Usb4), Wire::Up);
    let usb4 = cands(&[LinkKind::Usb4])[0];
    rig.manager.control().add(usb4).unwrap();
    let took = rig.until_active(Some(LinkKind::Usb4)).await;
    assert!(took <= Duration::from_secs(2), "{took:?}");
    let kinds: Vec<_> = rig.manager.links().iter().map(|l| l.kind).collect();
    assert_eq!(kinds, [LinkKind::Usb4, LinkKind::Wifi]);
}

#[tokio::test(start_paused = true)]
async fn removing_the_active_one_moves_to_the_next_at_once_even_with_a_hold() {
    let mut rig = Rig::new(
        &[LinkKind::Usb4, LinkKind::Wifi],
        &[LinkKind::Usb4, LinkKind::Wifi],
    );
    rig.until_active(Some(LinkKind::Usb4)).await;
    time::sleep(Duration::from_secs(2)).await;
    let _hold = rig.manager.hold();

    rig.manager.control().remove(LinkKind::Usb4).unwrap();
    let took = rig.until_active(Some(LinkKind::Wifi)).await;
    assert!(took < Duration::from_millis(100), "{took:?}");
    assert_eq!(rig.manager.links().len(), 1);
    assert_eq!(rig.manager.active().map(|a| a.kind), Some(LinkKind::Wifi));
}

#[tokio::test(start_paused = true)]
async fn the_link_that_returns_after_being_removed_waits_for_the_hysteresis() {
    let mut rig = Rig::new(
        &[LinkKind::Usb4, LinkKind::Wifi],
        &[LinkKind::Usb4, LinkKind::Wifi],
    );
    rig.until_active(Some(LinkKind::Usb4)).await;
    time::sleep(Duration::from_secs(2)).await;
    let control = rig.manager.control();
    control.remove(LinkKind::Usb4).unwrap();
    rig.until_active(Some(LinkKind::Wifi)).await;

    // The cable comes back: until it has gone `settle` without cuts, traffic stays on Wi-Fi.
    control.add(cands(&[LinkKind::Usb4])[0]).unwrap();
    time::sleep(Duration::from_secs(2)).await;
    assert_eq!(rig.manager.active().map(|a| a.kind), Some(LinkKind::Wifi));
    let took = rig.until_active(Some(LinkKind::Usb4)).await;
    assert!(took <= Duration::from_secs(3), "{took:?}");
}

#[tokio::test(start_paused = true)]
async fn removing_a_link_that_is_not_there_does_nothing() {
    let mut rig = Rig::new(&[LinkKind::Wifi], &[LinkKind::Wifi]);
    rig.until_active(Some(LinkKind::Wifi)).await;
    rig.manager.control().remove(LinkKind::Ethernet).unwrap();
    time::sleep(Duration::from_secs(3)).await;
    assert_eq!(rig.manager.active().map(|a| a.kind), Some(LinkKind::Wifi));
    assert_eq!(rig.seen.len(), 1);
}
