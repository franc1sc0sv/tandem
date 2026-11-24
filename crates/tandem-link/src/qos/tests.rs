use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::AsyncWrite;
use tokio::time::Instant;

use super::*;

/// Records when and how much was written.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<(Instant, usize)>>>);

impl Recorder {
    fn writes(&self) -> Vec<(Instant, usize)> {
        self.0.lock().unwrap().clone()
    }

    fn total(&self) -> usize {
        self.writes().iter().map(|(_, n)| n).sum()
    }
}

impl AsyncWrite for Recorder {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.0.lock().unwrap().push((Instant::now(), buf.len()));
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// The next pause or resume, without the cap changes.
async fn next_gate(events: &mut broadcast::Receiver<QosEvent>) -> QosEvent {
    loop {
        match events.recv().await.unwrap() {
            QosEvent::Cap { .. } => {}
            e => return e,
        }
    }
}

/// `lat` of input latency, one event per millisecond, for `dur`.
async fn input_for(qos: &Qos, lat: Duration, dur: Duration) {
    let end = Instant::now() + dur;
    while Instant::now() < end {
        qos.record_input_latency(lat);
        tokio::time::sleep(ms(1)).await;
    }
}

#[test]
fn input_goes_first_and_data_last() {
    let order = [
        StreamClass::Input,
        StreamClass::Control,
        StreamClass::Clipboard,
        StreamClass::Data,
    ];
    assert!(order.windows(2).all(|w| w[0].priority() > w[1].priority()));
    // A stream without a class (quinn starts at 0) is a control one.
    assert_eq!(StreamClass::Control.priority(), 0);
    assert_eq!(StreamClass::of(Channel::Input), StreamClass::Input);
    assert_eq!(StreamClass::of(Channel::Clipboard), StreamClass::Clipboard);
}

#[tokio::test(start_paused = true)]
async fn the_cap_follows_the_path_and_the_preferences() {
    let qos = Qos::default();
    let mut events = qos.events();
    assert_eq!(qos.cap(), None);

    qos.set_link(Some(LinkKind::Wifi));
    assert_eq!(qos.cap(), Some(UNMEASURED_WIFI / 2));
    qos.set_measured(8_000_000);
    assert_eq!(qos.cap(), Some(4_000_000));
    qos.set_prefs(DataPrefs {
        wifi_cap_percent: 25,
        wifi_cap_mbps: None,
    });
    assert_eq!(qos.cap(), Some(2_000_000));
    // Over cable, no cap; back on Wi-Fi, the previous one.
    qos.set_link(Some(LinkKind::Ethernet));
    assert_eq!(qos.cap(), None);
    qos.set_link(Some(LinkKind::Wifi));
    assert_eq!(qos.cap(), Some(2_000_000));

    let wifi = Some(LinkKind::Wifi);
    let caps: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
    assert_eq!(
        caps,
        vec![
            QosEvent::Cap {
                link: wifi,
                cap: Some(10_000_000)
            },
            QosEvent::Cap {
                link: wifi,
                cap: Some(4_000_000)
            },
            QosEvent::Cap {
                link: wifi,
                cap: Some(2_000_000)
            },
            QosEvent::Cap {
                link: Some(LinkKind::Ethernet),
                cap: None
            },
            QosEvent::Cap {
                link: wifi,
                cap: Some(2_000_000)
            },
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn data_over_wifi_does_not_pass_the_cap_and_over_cable_goes_free() {
    let qos = Qos::default();
    qos.set_link(Some(LinkKind::Wifi));
    qos.set_measured(2_000_000);
    let rec = Recorder::default();
    let mut data = qos.data_stream(rec.clone());
    let t0 = Instant::now();
    let chunk = vec![0u8; 16 * 1024];
    while Instant::now() < t0 + Duration::from_secs(3) {
        data.write_all(&chunk).await.unwrap();
    }
    let writes = rec.writes();
    for (i, (start, _)) in writes.iter().enumerate() {
        let window: usize = writes[i..]
            .iter()
            .take_while(|(t, _)| *t <= *start + Duration::from_secs(1))
            .map(|(_, n)| n)
            .sum();
        assert!(window <= 1_000_000, "{window} bytes in 1 s");
    }
    assert!(rec.total() >= 2_800_000, "only {}", rec.total());

    // Over cable there is no cap: everything goes out without waiting.
    qos.set_link(Some(LinkKind::Usb4));
    let rec = Recorder::default();
    let mut data = qos.data_stream(rec.clone());
    let t1 = Instant::now();
    data.write_all(&vec![0u8; 50_000_000]).await.unwrap();
    assert_eq!(rec.total(), 50_000_000);
    assert_eq!(Instant::now(), t1);
}

#[tokio::test(start_paused = true)]
async fn late_input_pauses_the_data_and_without_input_it_resumes() {
    let qos = Qos::default();
    qos.set_link(Some(LinkKind::Wifi));
    let mut events = qos.events();
    let gate = qos.watch_gate();
    let t0 = Instant::now();
    // 2.1 s of input at 40 ms (the Wi-Fi target is 20 ms).
    while Instant::now() < t0 + ms(2100) {
        qos.record_input_latency(ms(40));
        tokio::time::sleep(ms(1)).await;
    }
    assert_eq!(
        qos.gate(),
        DataGate::Paused {
            p99: ms(40),
            target: ms(20)
        }
    );
    assert!(gate.has_changed().unwrap());
    let paused = next_gate(&mut events).await;
    assert!(matches!(paused, QosEvent::Paused { p99, .. } if p99 == ms(40)));

    // While paused nothing goes out; without input, the pause ends on its own (window + hysteresis).
    let rec = Recorder::default();
    let mut data = qos.data_stream(rec.clone());
    let stopped = Instant::now();
    data.write_all(b"hello").await.unwrap();
    let waited = Instant::now() - stopped;
    assert!((ms(1900)..ms(2300)).contains(&waited), "waited {waited:?}");
    assert_eq!(rec.total(), 5);
    assert_eq!(qos.gate(), DataGate::Open);
    assert!(matches!(
        next_gate(&mut events).await,
        QosEvent::Resumed {
            link: Some(LinkKind::Wifi),
            ..
        }
    ));
}

/// A terminal's output is not held by the input pause (a pause once lasted 90 s on the PC and
/// froze the terminal view), but it still respects the cap.
#[tokio::test(start_paused = true)]
async fn live_data_ignores_the_pause_but_not_the_cap() {
    let qos = Qos::default();
    qos.set_link(Some(LinkKind::Wifi));
    qos.set_measured(2_000_000);
    let t0 = Instant::now();
    while Instant::now() < t0 + ms(2100) {
        qos.record_input_latency(ms(40));
        tokio::time::sleep(ms(1)).await;
    }
    assert!(!qos.gate().is_open());
    let rec = Recorder::default();
    let mut live = qos.live_stream(rec.clone());
    let t1 = Instant::now();
    live.write_all(b"PS C:\\> ").await.unwrap();
    assert_eq!(Instant::now(), t1, "it went out during the pause");
    assert_eq!(rec.total(), 8);
    // The cap (1 MB/s here) still holds it.
    while Instant::now() < t1 + Duration::from_secs(2) {
        live.write_all(&[0u8; 16 * 1024]).await.unwrap();
    }
    assert!(rec.total() <= 2_200_000, "{} bytes in 2 s", rec.total());
}

#[tokio::test(start_paused = true)]
async fn changing_path_lifts_the_pause() {
    let qos = Qos::default();
    qos.set_link(Some(LinkKind::Wifi));
    let t0 = Instant::now();
    while Instant::now() < t0 + ms(2100) {
        qos.record_input_latency(ms(100));
        tokio::time::sleep(ms(1)).await;
    }
    assert!(!qos.gate().is_open());
    qos.set_link(Some(LinkKind::Ethernet));
    assert!(qos.gate().is_open());
}

fn wifi_at(measured: u64) -> Qos {
    let qos = Qos::default();
    qos.set_link(Some(LinkKind::Wifi));
    qos.set_measured(measured);
    qos
}

#[tokio::test(start_paused = true)]
async fn input_near_the_target_lowers_the_cap_a_bit() {
    let qos = wifi_at(8_000_000);
    assert_eq!(qos.cap(), Some(4_000_000));
    // Fine: nothing changes.
    input_for(&qos, ms(3), ms(3000)).await;
    assert_eq!(qos.cap(), Some(4_000_000));
    // Near the target (16 of 20 ms): a bit less, only once in the window.
    input_for(&qos, ms(16), ms(900)).await;
    assert_eq!(qos.cap(), Some(2_800_000));
}

#[tokio::test(start_paused = true)]
async fn past_the_target_the_cap_drops_to_half_once_per_window() {
    let qos = wifi_at(8_000_000);
    input_for(&qos, ms(25), ms(900)).await;
    assert_eq!(qos.cap(), Some(2_000_000));
    assert!(qos.gate().is_open());
}

#[tokio::test(start_paused = true)]
async fn with_margin_the_cap_returns_little_by_little_and_in_the_middle_it_stays() {
    let qos = wifi_at(8_000_000);
    input_for(&qos, ms(25), ms(900)).await;
    // In the middle band (between 10 and 15 ms) it neither rises nor falls, with the window already clean.
    input_for(&qos, ms(12), ms(2100)).await;
    let held = qos.cap().unwrap();
    assert!(held < 4_000_000);
    input_for(&qos, ms(12), ms(3000)).await;
    assert_eq!(qos.cap(), Some(held));
    // With margin it rises, little by little and without exceeding the configured one.
    input_for(&qos, ms(3), ms(2100)).await;
    let up = qos.cap().unwrap();
    assert!(up > held && up < held * 2, "{held} → {up}");
    input_for(&qos, ms(3), ms(30_000)).await;
    assert_eq!(qos.cap(), Some(4_000_000));
    assert_eq!(qos.configured_cap(), Some(4_000_000));
}

#[tokio::test(start_paused = true)]
async fn a_pause_leaves_the_cap_at_half_on_returning() {
    let qos = wifi_at(8_000_000);
    input_for(&qos, ms(200), ms(2100)).await;
    assert!(!qos.gate().is_open());
    // To half once per window while input stayed over (4 → 2 → 1 MB/s) and
    // once more because of the pause.
    assert_eq!(qos.cap(), Some(500_000));
    // A new measurement uses the whole cap again.
    qos.set_measured(6_000_000);
    assert_eq!(qos.cap(), Some(3_000_000));
}

#[tokio::test(start_paused = true)]
async fn one_qos_per_shared_path_with_the_desks_data() {
    let set = LinkQos::default();
    let wifi = set.get(LinkKind::Wifi);
    assert!(wifi.same(&set.get(LinkKind::Wifi)));
    assert!(!wifi.same(&set.get(LinkKind::Ethernet)));
    assert_eq!(wifi.link(), Some(LinkKind::Wifi));
    assert_eq!(wifi.cap(), Some(UNMEASURED_WIFI / 2));

    let prefs = DataPrefs {
        wifi_cap_percent: 20,
        wifi_cap_mbps: None,
    };
    set.set_prefs(prefs);
    assert_eq!(set.prefs(), prefs);
    assert_eq!(wifi.cap(), Some(UNMEASURED_WIFI / 5));
    // One created later also takes them.
    let usb4 = set.get(LinkKind::Usb4);
    assert_eq!(usb4.cap(), None);
    usb4.set_link(Some(LinkKind::Wifi));
    assert_eq!(usb4.cap(), Some(UNMEASURED_WIFI / 5));
}

#[tokio::test(start_paused = true)]
async fn the_label_recovers_when_the_other_announces_a_cable_again() {
    let qos = Qos::default();
    qos.set_link(Some(LinkKind::Ethernet));
    qos.set_peer_link(Some(LinkKind::Wifi));
    assert_eq!(qos.link(), Some(LinkKind::Wifi));
    assert_eq!(qos.own_link(), Some(LinkKind::Ethernet));
    qos.set_peer_link(Some(LinkKind::Ethernet));
    assert_eq!(qos.link(), Some(LinkKind::Ethernet));
    assert_eq!(qos.cap(), None);
    qos.set_peer_link(Some(LinkKind::Wifi));
    qos.set_peer_link(None);
    assert_eq!(qos.link(), Some(LinkKind::Ethernet));
}

#[tokio::test(start_paused = true)]
async fn the_others_class_is_not_lost_if_the_own_is_unknown() {
    let qos = Qos::default();
    qos.set_peer_link(Some(LinkKind::Wifi));
    assert_eq!(qos.link(), Some(LinkKind::Wifi));
    qos.set_link(Some(LinkKind::Usb4));
    assert_eq!(qos.link(), Some(LinkKind::Wifi));
}

#[tokio::test(start_paused = true)]
async fn closing_an_old_connection_does_not_erase_the_new_ones_class() {
    let qos = Qos::default();
    qos.set_link(Some(LinkKind::Ethernet));
    qos.set_peer_link_from(1, LinkKind::Wifi);
    // The new connection announces its own and then the old one closes.
    qos.set_peer_link_from(2, LinkKind::Wifi);
    qos.clear_peer_link_of(1);
    assert_eq!(qos.link(), Some(LinkKind::Wifi));
    // When the one that announced it closes, it is erased.
    qos.clear_peer_link_of(2);
    assert_eq!(qos.link(), Some(LinkKind::Ethernet));
}
