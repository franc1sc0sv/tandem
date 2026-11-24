//! Per-path network metrics (T-102, DESIGN §6): what the Network screen shows when a
//! path is cut, and the bandwidth test over a simulated Wi-Fi with limited capacity.
//!
//! With the tokio clock paused: pings, switch times and the Wi-Fi capacity
//! are exact.

use std::time::Duration;

use tandem_core::Machine;
use tandem_link::manager::LinkKind;
use tandem_link::metrics::Metrics;
use tandem_link::metrics::probe::{receive_probe, send_probe};
use tandem_link::qos::{LinkQos, Qos, input_p99_target};
use tandem_proto::{Link, NetworkStatus, PathState, PathSwitch};
use tandem_sim::{Desk, Outcome, SimWifi};

/// The Mac with the PC on and both agents seeing each other over the three paths.
async fn both_on() -> Desk {
    let d = Desk::new().await.unwrap();
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    d.pc_agrees().await.unwrap();
    d
}

fn states(s: &NetworkStatus) -> Vec<(Link, PathState)> {
    s.paths.iter().map(|p| (p.link, p.state)).collect()
}

#[tokio::test(start_paused = true)]
async fn cutting_a_path_produces_the_event_with_its_duration_and_changes_the_states() {
    let d = both_on().await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    let mac = d.agent(Machine::A);
    let before = mac.network_status();
    assert_eq!(
        states(&before),
        [
            (Link::Usb4, PathState::Primary),
            (Link::Ethernet, PathState::Backup),
            (Link::Wifi, PathState::Backup),
        ]
    );
    let usb = before.path(Link::Usb4).unwrap();
    assert!(usb.rtt_us.is_some());
    assert!(usb.history.len() >= 5, "{:?}", usb.history);
    let seen = before.switches.len();

    d.net.cut(LinkKind::Usb4);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let after = mac.network_status();
    assert_eq!(
        states(&after),
        [
            (Link::Usb4, PathState::Down),
            (Link::Ethernet, PathState::Primary),
            (Link::Wifi, PathState::Backup),
        ]
    );
    assert_eq!(after.switches.len(), seen + 1, "{:?}", after.switches);
    let PathSwitch {
        from, to, took_ms, ..
    } = after.switches[0];
    assert_eq!((from, to), (Some(Link::Usb4), Some(Link::Ethernet)));
    // From the last USB-C answer: at most one ping and its maximum time.
    let took = took_ms.expect("a switch caused by a drop has a duration");
    assert!((1..=1250).contains(&took), "{took} ms");
    // The history already shows the USB-C without a response.
    let usb = after.path(Link::Usb4).unwrap();
    assert_eq!(usb.rtt_us, None);
    assert_eq!(usb.history.last().and_then(|s| s.rtt_us), None);

    // The other machine sees the same.
    let pc = d.agent(Machine::B).network_status();
    assert_eq!(pc.primary().map(|p| p.link), Some(Link::Ethernet), "{pc:?}");
    assert_eq!(
        pc.switches.first().map(|s| (s.from, s.to)),
        Some((Some(Link::Usb4), Some(Link::Ethernet)))
    );

    // And if Ethernet also drops, it goes to Wi-Fi.
    d.net.cut(LinkKind::Ethernet);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let wifi = mac.network_status();
    assert_eq!(wifi.primary().map(|p| p.link), Some(Link::Wifi));
    assert_eq!(
        wifi.switches.first().map(|s| (s.from, s.to)),
        Some((Some(Link::Ethernet), Some(Link::Wifi)))
    );
    assert!(wifi.switches[0].took_ms.is_some());
}

/// Measures a simulated Wi-Fi of `capacity` bytes per second with the 3 s test.
async fn measure(wifi: &SimWifi) -> u64 {
    let (mut w, mut r) = wifi.data_pipe();
    let answer = tokio::spawn(async move { receive_probe(&mut r, 0).await.unwrap() });
    let end = send_probe(&mut w, Duration::from_secs(3), &Qos::default())
        .await
        .unwrap();
    assert!(!end.paused);
    drop(w);
    answer.await.unwrap().bytes_per_sec().unwrap()
}

fn near(got: u64, want: u64) -> bool {
    got.abs_diff(want) * 100 <= want * 2
}

#[tokio::test(start_paused = true)]
async fn the_measurement_gives_what_the_simulated_wifi_gives_and_sets_the_cap() {
    const CAPACITY: u64 = 8_000_000;
    let wifi = SimWifi::new(CAPACITY, Duration::from_millis(2), 256 * 1024);
    let bps = measure(&wifi).await;
    assert!(near(bps, CAPACITY), "{bps} B/s");

    // What was measured stays in the metrics and the data cap is half of it (T-101).
    let metrics = Metrics::default();
    let qos = LinkQos::default();
    metrics.record_bandwidth(LinkKind::Wifi, bps, &qos);
    assert_eq!(
        metrics
            .status()
            .path(Link::Wifi)
            .and_then(|p| p.bandwidth)
            .map(|b| b.bytes_per_sec),
        Some(bps)
    );
    assert_eq!(qos.get(LinkKind::Wifi).cap(), Some(bps / 2));

    // If the Wi-Fi gets worse, the next measurement says so.
    wifi.set_capacity(CAPACITY / 4);
    let worse = measure(&wifi).await;
    assert!(near(worse, CAPACITY / 4), "{worse} B/s");
}

/// Review of T-102: the bandwidth test goes without a cap but respects the pause of
/// T-101. If input exceeds its target during the test, it is cut before the 3 s.
#[tokio::test(start_paused = true)]
async fn the_measurement_is_cut_off_if_input_exceeds_its_target() {
    let wifi = SimWifi::new(8_000_000, Duration::from_millis(2), 256 * 1024);
    let qos = Qos::default();
    qos.set_link(Some(LinkKind::Wifi));
    let late = input_p99_target(Some(LinkKind::Wifi)) * 2;
    // Input at 1000 events/s, all over the target.
    let feed = {
        let qos = qos.clone();
        tokio::spawn(async move {
            loop {
                qos.record_input_latency(late);
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
    };
    let (mut w, mut r) = wifi.data_pipe();
    let answer = tokio::spawn(async move { receive_probe(&mut r, 0).await.unwrap() });
    let t0 = tokio::time::Instant::now();
    let end = send_probe(&mut w, Duration::from_secs(3), &qos)
        .await
        .unwrap();
    let took = t0.elapsed();
    feed.abort();
    drop(w);
    assert!(end.paused, "{end:?}");
    // The pause kicks in at 2 s over the target (T-101), before the end of the test.
    assert!(took < Duration::from_secs(3), "{took:?}");
    assert!(took >= Duration::from_secs(2), "{took:?}");
    // The other side receives the end anyway.
    answer.await.unwrap();
}

/// T-094: a USB4 cable plugged in mid-session is detected by its interface, becomes
/// the main path on both machines, and on unplugging it input moves to the
/// next path in under 1 s, without waiting for the health check.
#[tokio::test(start_paused = true)]
async fn a_usb4_cable_that_appears_becomes_the_main_one_and_its_drop_moves_to_the_next() {
    let d = Desk::builder()
        .links(&[LinkKind::Ethernet, LinkKind::Wifi])
        .build()
        .await
        .unwrap();
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    d.pc_agrees().await.unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    let (mac, pc) = (d.agent(Machine::A), d.agent(Machine::B));
    for a in [mac, pc] {
        let s = a.network_status();
        assert_eq!(s.primary().map(|p| p.link), Some(Link::Ethernet));
        assert_eq!(
            s.path(Link::Usb4).map(|p| p.state),
            Some(PathState::Unplugged)
        );
    }

    // The USB4 interface appears on both machines.
    d.plug_cable(LinkKind::Usb4);
    tokio::time::sleep(Duration::from_secs(3)).await;
    for a in [mac, pc] {
        let s = a.network_status();
        assert_eq!(s.primary().map(|p| p.link), Some(Link::Usb4), "{s:?}");
    }

    // It is unplugged: in under 1 s input already goes over Ethernet.
    d.unplug_cable(LinkKind::Usb4);
    tokio::time::sleep(Duration::from_millis(900)).await;
    for a in [mac, pc] {
        let s = a.network_status();
        assert_eq!(s.primary().map(|p| p.link), Some(Link::Ethernet), "{s:?}");
        let last = s
            .switches
            .first()
            .expect("the switch stayed in the history");
        assert_eq!(
            (last.from, last.to),
            (Some(Link::Usb4), Some(Link::Ethernet))
        );
    }
}
