//! Data over Wi-Fi without delaying input (T-101, DECISIONS A2, DESIGN §6 and §7.1).
//!
//! Both ends in the same process, joined by a [`SimWifi`] of 8 MB/s with 2 ms of
//! propagation and a 256 KiB queue at the access point. One sends data flat out over a
//! [`DataStream`] and input at 1000 events/s; the other measures how long each event takes and
//! hands it back to the connection's [`Qos`] (what T-093's measurement will do). All with the
//! tokio clock paused: times are exact.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tandem_link::manager::LinkKind;
use tandem_link::qos::{Qos, QosEvent, input_p99_target};
use tandem_sim::SimWifi;
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;
use tokio::time::{self, Instant, MissedTickBehavior};

const CAPACITY: u64 = 8_000_000;
const BASE: Duration = Duration::from_millis(2);
const BUFFER: u64 = 256 * 1024;
const CHUNK: usize = 16 * 1024;
/// The simulated target: the Wi-Fi p99 of DESIGN §7.1.
const TARGET: Duration = Duration::from_millis(20);

fn secs(s: f64) -> Duration {
    Duration::from_secs_f64(s)
}

/// Latencies measured on the other side: when each event left and how long it took.
type Latencies = Arc<Mutex<Vec<(Instant, Duration)>>>;

/// 1000 input events per second until `end`; the other side measures each one on arrival and
/// passes it to `qos` if there is one.
fn input(wifi: &SimWifi, qos: Option<Qos>, end: Instant) -> (JoinHandle<()>, Latencies) {
    let lat: Latencies = Arc::default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(Instant, Instant)>();
    let wifi = wifi.clone();
    let send = tokio::spawn(async move {
        let mut tick = time::interval(Duration::from_millis(1));
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        while Instant::now() < end {
            tick.tick().await;
            // A movement event or a key: a few dozen bytes with QUIC.
            let _ = tx.send((Instant::now(), wifi.send_input(64)));
        }
    });
    let seen = lat.clone();
    let receive = tokio::spawn(async move {
        while let Some((sent, arrives)) = rx.recv().await {
            time::sleep_until(arrives).await;
            let took = Instant::now() - sent;
            if let Some(q) = &qos {
                q.record_input_latency(took);
            }
            seen.lock().unwrap().push((sent, took));
        }
    });
    let both = tokio::spawn(async move {
        let _ = send.await;
        let _ = receive.await;
    });
    (both, lat)
}

/// p99 of the events that left in [from, to).
fn p99(lat: &Latencies, from: Instant, to: Instant) -> Duration {
    let mut v: Vec<Duration> = lat
        .lock()
        .unwrap()
        .iter()
        .filter(|(t, _)| (from..to).contains(t))
        .map(|(_, l)| *l)
        .collect();
    assert!(v.len() > 100, "too few samples: {}", v.len());
    v.sort();
    v[(v.len() * 99).div_ceil(100) - 1]
}

/// Bytes of data that left in [from, to).
fn data_between(wifi: &SimWifi, from: Instant, to: Instant) -> usize {
    wifi.data_sent()
        .iter()
        .filter(|(t, _)| (from..to).contains(t))
        .map(|(_, n)| n)
        .sum()
}

#[tokio::test(start_paused = true)]
async fn without_a_cap_the_data_adds_latency_to_input() {
    // What the cap prevents: data at everything the Wi-Fi gives fills the access point's
    // queue and every event waits behind 256 KiB.
    let wifi = SimWifi::new(CAPACITY, BASE, BUFFER);
    let t0 = Instant::now();
    let end = t0 + Duration::from_secs(3);
    let mut raw = wifi.data_writer();
    let data = tokio::spawn(async move {
        let chunk = vec![0u8; CHUNK];
        while Instant::now() < end {
            raw.write_all(&chunk).await.unwrap();
        }
    });
    let (input, lat) = input(&wifi, None, end);
    input.await.unwrap();
    data.await.unwrap();
    let p = p99(&lat, t0 + secs(1.0), end);
    assert!(p > Duration::from_millis(30), "p99 without a cap: {p:?}");
}

/// What happened in a run with the `Qos` in the middle.
struct Run {
    t0: Instant,
    end: Instant,
    wifi: SimWifi,
    lat: Latencies,
    /// Pauses and resumes, in order (without the cap changes).
    gates: Vec<(Instant, QosEvent)>,
}

impl Run {
    fn at(&self, s: f64) -> Instant {
        self.t0 + secs(s)
    }

    /// Bytes per second of data in [from, to) (in seconds from the start).
    fn rate(&self, from: f64, to: f64) -> f64 {
        data_between(&self.wifi, self.at(from), self.at(to)) as f64 / (to - from)
    }

    fn p99(&self, from: f64, to: f64) -> Duration {
        p99(&self.lat, self.at(from), self.at(to))
    }
}

/// Data flat out and input at 1000 events/s for `total` seconds; the Wi-Fi's capacity
/// changes according to `changes` (second, bytes per second).
async fn run(total: f64, changes: &[(f64, u64)]) -> Run {
    let wifi = SimWifi::new(CAPACITY, BASE, BUFFER);
    let qos = Qos::default();
    qos.set_link(Some(LinkKind::Wifi));
    assert_eq!(input_p99_target(Some(LinkKind::Wifi)), TARGET);
    // The last measurement of the Wi-Fi: the default cap is half.
    qos.set_measured(CAPACITY);
    assert_eq!(qos.cap(), Some(CAPACITY / 2));

    let events: Arc<Mutex<Vec<(Instant, QosEvent)>>> = Arc::default();
    let mut rx = qos.events();
    let seen = events.clone();
    let watcher = tokio::spawn(async move {
        while let Ok(e) = rx.recv().await {
            if !matches!(e, QosEvent::Cap { .. }) {
                seen.lock().unwrap().push((Instant::now(), e));
            }
        }
    });

    let t0 = Instant::now();
    let end = t0 + secs(total);
    let mut out = qos.data_stream(wifi.data_writer());
    let data = tokio::spawn(async move {
        let chunk = vec![0u8; CHUNK];
        while Instant::now() < end {
            out.write_all(&chunk).await.unwrap();
        }
    });
    let (input, lat) = input(&wifi, Some(qos.clone()), end);
    for &(at, capacity) in changes {
        time::sleep_until(t0 + secs(at)).await;
        wifi.set_capacity(capacity);
    }
    input.await.unwrap();
    data.await.unwrap();
    drop(qos);
    watcher.abort();

    // No second exceeds the cap.
    let log = wifi.data_sent();
    for (i, (start, _)) in log.iter().enumerate() {
        let window: usize = log[i..]
            .iter()
            .take_while(|(t, _)| *t <= *start + Duration::from_secs(1))
            .map(|(_, n)| n)
            .sum();
        assert!(window as u64 <= CAPACITY / 2, "{window} bytes in 1 s");
    }
    let gates = events.lock().unwrap().clone();
    for (t, e) in &gates {
        println!("{:?}: {e:?}", *t - t0);
    }
    Run {
        t0,
        end,
        wifi,
        lat,
        gates,
    }
}

#[tokio::test(start_paused = true)]
async fn data_flat_out_over_wifi_with_input_at_1000_events_per_second() {
    // At 5 s the Wi-Fi drops to 300 kB/s (strong interference): not even lowering the cap
    // saves the input in time. At 8 s it comes back.
    let r = run(30.0, &[(5.0, 300_000), (8.0, CAPACITY)]).await;

    // With the Wi-Fi fine, input within the target and data at the cap (4 MB/s).
    let steady = r.p99(1.0, 5.0);
    assert!(steady <= TARGET, "p99 with data flat out: {steady:?}");
    assert!(r.rate(1.0, 5.0) >= 3_800_000.0, "{}", r.rate(1.0, 5.0));

    // The drop: input exceeds the target for 2 s and the data pauses.
    let (paused_at, paused) = r.gates.first().expect("no pause");
    match paused {
        QosEvent::Paused { p99, target, link } => {
            assert_eq!(*target, TARGET);
            assert!(*p99 > TARGET);
            assert_eq!(*link, Some(LinkKind::Wifi));
        }
        other => panic!("the pause was expected: {other:?}"),
    }
    let after_drop = *paused_at - r.at(5.0);
    assert!(
        (secs(2.0)..secs(2.5)).contains(&after_drop),
        "paused {after_drop:?} after the drop"
    );
    let (resumed_at, resumed) = r.gates.get(1).expect("no resume");
    assert!(matches!(resumed, QosEvent::Resumed { .. }), "{resumed:?}");
    // While paused nothing leaves except what was already being written.
    let during = data_between(&r.wifi, *paused_at, *resumed_at);
    assert!(during <= 2 * CHUNK, "{during} bytes left while paused");
    // A single pause: with the Wi-Fi back there are no more.
    assert_eq!(r.gates.len(), 2, "{:?}", r.gates);

    // The throughput returns to the cap only because the Wi-Fi came back: not before 8 s.
    assert!(r.rate(5.5, 8.0) < 1_000_000.0, "{}", r.rate(5.5, 8.0));
    assert!(r.rate(26.0, 30.0) >= 3_800_000.0, "{}", r.rate(26.0, 30.0));
    println!(
        "from 5.5 to 8 s: {} B/s; from 26 to 30 s: {} B/s",
        r.rate(5.5, 8.0),
        r.rate(26.0, 30.0)
    );
    let back = p99(&r.lat, *resumed_at + secs(1.0), r.end);
    assert!(back <= TARGET, "p99 after resuming: {back:?}");
}

#[tokio::test(start_paused = true)]
async fn with_a_wifi_that_does_not_recover_input_stays_within_the_target() {
    // At 5 s the Wi-Fi drops to 2 MB/s and stays that way: the 4 MB/s cap no longer suffices.
    let r = run(30.0, &[(5.0, 2_000_000)]).await;
    // Data drops to what the Wi-Fi gives without delaying input.
    let p = r.p99(15.0, 30.0);
    println!(
        "p99 from 15 to 30 s: {p:?}; data: {} B/s",
        r.rate(15.0, 30.0)
    );
    assert!(p <= TARGET, "p99 with the bad Wi-Fi: {p:?}");
    // Without pauses and resumes in a cycle.
    let late: Vec<_> = r.gates.iter().filter(|(t, _)| *t >= r.at(12.0)).collect();
    assert!(late.is_empty(), "{late:?}");
    // And the data keeps leaving, at a reasonable fraction of what remains.
    let rate = r.rate(15.0, 30.0);
    assert!(rate >= 800_000.0, "only {rate} B/s");
}
