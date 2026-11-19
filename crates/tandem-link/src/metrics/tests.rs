use std::net::SocketAddr;

use proptest::prelude::*;
use tandem_proto::Link;

use super::*;

fn status(kind: LinkKind, rtt_ms: Option<u64>) -> LinkStatus {
    LinkStatus {
        kind,
        addr: SocketAddr::from(([127, 0, 0, 1], 1)),
        rtt: rtt_ms.map(Duration::from_millis),
    }
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn states(m: &NetMetrics) -> Vec<PathState> {
    LinkKind::ALL.into_iter().map(|k| m.state(k)).collect()
}

#[test]
fn the_states_follow_the_manager() {
    let mut m = NetMetrics::default();
    use PathState::*;
    assert_eq!(states(&m), [Unplugged, Unplugged, Unplugged]);
    // Ethernet configured but without a cable; USB-C and Wi-Fi answer.
    let links = [
        status(LinkKind::Usb4, Some(1)),
        status(LinkKind::Ethernet, None),
        status(LinkKind::Wifi, Some(4)),
    ];
    m.observe(&links, Some(LinkKind::Usb4));
    assert_eq!(states(&m), [Primary, Unplugged, Backup]);
    // USB-C is cut: down, not "no cable".
    let links = [
        status(LinkKind::Usb4, None),
        status(LinkKind::Ethernet, None),
        status(LinkKind::Wifi, Some(4)),
    ];
    m.observe(&links, Some(LinkKind::Wifi));
    assert_eq!(states(&m), [Down, Unplugged, Primary]);
    let s = m.status();
    assert_eq!(s.primary().map(|p| p.link), Some(Link::Wifi));
    assert_eq!(s.path(Link::Wifi).and_then(|p| p.rtt_us), Some(4000));
    assert_eq!(s.path(Link::Usb4).map(|p| p.input_target_us), Some(4000));
    assert_eq!(s.path(Link::Wifi).map(|p| p.input_target_us), Some(20_000));
}

#[test]
fn input_and_history_go_per_second() {
    let mut m = NetMetrics::default();
    m.observe(&[status(LinkKind::Wifi, Some(3))], Some(LinkKind::Wifi));
    m.set_input(LinkKind::Wifi, Some((ms(2), ms(9))));
    m.tick();
    m.observe(&[status(LinkKind::Wifi, None)], None);
    m.set_input(LinkKind::Wifi, None);
    m.tick();
    let s = m.status();
    let wifi = s.path(Link::Wifi).unwrap();
    assert_eq!((wifi.input_p50_us, wifi.input_p99_us), (None, None));
    assert_eq!(
        wifi.history,
        [
            LatencySample {
                rtt_us: Some(3000),
                input_p99_us: Some(9000)
            },
            LatencySample::default()
        ]
    );
    // Each path has its history, also the one that never answered.
    assert_eq!(s.path(Link::Ethernet).unwrap().history.len(), 2);
}

#[test]
fn changes_stay_from_newest_to_oldest() {
    let mut m = NetMetrics::default();
    m.switched(
        &LinkEvent {
            seq: 0,
            from: None,
            to: Some(LinkKind::Usb4),
            took: None,
        },
        10,
    );
    m.switched(
        &LinkEvent {
            seq: 1,
            from: Some(LinkKind::Usb4),
            to: Some(LinkKind::Wifi),
            took: Some(ms(600)),
        },
        20,
    );
    let s = m.status();
    assert_eq!(
        s.switches[0],
        PathSwitch {
            at_ms: 20,
            from: Some(Link::Usb4),
            to: Some(Link::Wifi),
            took_ms: Some(600)
        }
    );
    assert_eq!(s.switches[1].at_ms, 10);
}

#[test]
fn one_measurement_at_a_time_and_it_reaches_the_data_cap() {
    let metrics = Metrics::default();
    let qos = LinkQos::default();
    let first = metrics.begin_measure(LinkKind::Wifi).unwrap();
    assert_eq!(metrics.status().measuring, Some(Link::Wifi));
    assert!(metrics.begin_measure(LinkKind::Usb4).is_none());
    metrics.record_bandwidth(LinkKind::Wifi, 40_000_000, &qos);
    drop(first);
    assert_eq!(metrics.status().measuring, None);
    let bw = metrics
        .status()
        .path(Link::Wifi)
        .unwrap()
        .bandwidth
        .unwrap();
    assert_eq!(bw.bytes_per_sec, 40_000_000);
    assert!(bw.at_ms > 0);
    // 50 % of what was measured (T-101).
    assert_eq!(qos.get(LinkKind::Wifi).cap(), Some(20_000_000));
    assert!(metrics.begin_measure(LinkKind::Usb4).is_some());
}

proptest! {
    /// With any history: at most one primary (and it is the active one), 60 samples and 50
    /// changes.
    #[test]
    fn the_view_stays_bounded_and_coherent(
        steps in prop::collection::vec((any::<[bool; 3]>(), 0usize..4, any::<bool>()), 0..300)
    ) {
        let mut m = NetMetrics::default();
        let mut seq = 0;
        for (up, active, switch) in steps {
            let links: Vec<_> = LinkKind::ALL
                .into_iter()
                .zip(up)
                .map(|(k, up)| status(k, up.then_some(1)))
                .collect();
            let active = LinkKind::ALL.get(active).copied();
            if switch {
                m.switched(&LinkEvent { seq, from: None, to: active, took: None }, seq);
                seq += 1;
            }
            m.observe(&links, active);
            m.tick();
            let s = m.status();
            prop_assert!(s.switches.len() <= MAX_SWITCHES);
            prop_assert!(s.paths.iter().all(|p| p.history.len() <= HISTORY_SECS));
            let primaries: Vec<_> = s.paths.iter().filter(|p| p.state == PathState::Primary).collect();
            prop_assert!(primaries.len() <= 1);
            if let Some(p) = primaries.first() {
                prop_assert_eq!(Some(p.link), active.map(Into::into));
                prop_assert!(p.rtt_us.is_some());
            }
        }
    }
}
