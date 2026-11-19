//! Input latency bench (T-093, DESIGN §7.1): two agents over real QUIC on
//! loopback, simulated capture and injection, 1000 movements per second.
//!
//! The latency of each movement is from when the simulated capture delivers it until
//! the injector on the other side puts the cursor at (or beyond) its position; both sides
//! are in the same process, so it is measured with a single clock. It is also checked that
//! the engine's own measurement (server marks + heartbeat offset) reaches
//! both UIs.
//!
//! - In CI it only fails on a large regression (p99 > 20 ms): runners are
//!   shared and the real target cannot be demanded there.
//! - The target (p99 ≤ 1 ms) is checked locally, in release:
//!   `cargo test -p tandem-sim --release --test latency -- --ignored --nocapture`.

use std::time::{Duration, Instant};

use tandem_input::InputEvent;
use tandem_input::geometry::{LayoutOptions, Rect, Screen};
use tandem_input::session::{FollowConfig, SessionConfig};
use tandem_proto::Link;
use tandem_sim::InputPair;
use tandemd::input::InputOptions;

const LIMIT: Duration = Duration::from_secs(30);

fn opts() -> InputOptions {
    InputOptions {
        session: SessionConfig {
            heartbeat_interval: Duration::from_millis(100),
            heartbeat_timeout: Duration::from_millis(3000),
            latency_every: Duration::from_millis(500),
            ..SessionConfig::default()
        },
        layout: LayoutOptions { corner_margin: 0.0 },
        follow: FollowConfig::default(),
        report_every: Duration::from_millis(200),
        screens_every: Duration::from_millis(200),
        permissions_every: Duration::from_millis(200),
        retry: Duration::from_millis(100),
        ..InputOptions::default()
    }
}

fn screen(id: &str, w: u32, h: u32) -> Screen {
    Screen::new(id.into(), Rect::new(0, 0, w, h), 1.0)
}

/// Percentile `q` (0–1) of a sorted list.
fn pct(sorted: &[Duration], q: f64) -> Duration {
    let rank = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

struct Run {
    sorted: Vec<Duration>,
}

impl Run {
    fn p50(&self) -> Duration {
        pct(&self.sorted, 0.50)
    }
    fn p99(&self) -> Duration {
        pct(&self.sorted, 0.99)
    }
    fn max(&self) -> Duration {
        pct(&self.sorted, 1.0)
    }
}

/// Crosses the cursor from A to B and sends `n` 1 px movements at 1000 per second from a
/// thread of its own (like a real capture's). Returns the latency of each one.
async fn bench(pair: &InputPair, n: usize) -> Run {
    if let Ok(filter) = std::env::var("TANDEM_TEST_LOG") {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .try_init();
    }
    let a = &pair.a;
    let b = &pair.b;
    a.until_input(LIMIT, |s| s.session && s.server == a.machine())
        .await
        .unwrap();
    // If the cursor was left on B from a previous run, return to A through the edge.
    a.fake
        .capture(InputEvent::MotionRelative { dx: -1e6, dy: 0.0 });
    // B's screens must have reached A for the edge to exist.
    let entered = tokio::time::timeout(LIMIT, async {
        loop {
            let before = b.fake.timeline().len();
            a.fake.capture(InputEvent::MotionAbsolute {
                x: 1919.0,
                y: 500.0,
            });
            a.fake
                .capture(InputEvent::MotionRelative { dx: 5.0, dy: 0.0 });
            tokio::time::sleep(Duration::from_millis(50)).await;
            let entered = b.fake.timeline()[before..]
                .iter()
                .find_map(|(_, e)| match e {
                    InputEvent::MotionAbsolute { x, .. } => Some(*x),
                    _ => None,
                });
            if let Some(x) = entered {
                return x;
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "the cursor did not cross to B.\n A: {:?}\n B: {:?}\n A's state: {:?}\n B's screens on A: {:?}\n A's arrangement: {:?}\n A's capture: {} {:?}\n injected on B: {}",
            a.input(),
            b.input(),
            a.agent.state(),
            a.agent.inbox().screens.borrow().clone(),
            a.agent.desk().arrangement,
            a.fake.capturing(),
            a.fake.capture_calls().borrow().clone(),
            b.fake.timeline().len(),
        )
    });
    assert_eq!(entered, 0.0, "enters through B's left edge");
    // Two heartbeats with echo: the engine already estimates the offset.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let start = b.fake.timeline().len();
    let fake = a.fake.clone();
    let sent = std::thread::spawn(move || {
        let mut at = Vec::with_capacity(n);
        let t0 = Instant::now();
        for i in 0..n {
            // At the pace of a 1000 Hz mouse, without accumulating lag.
            let due = t0 + Duration::from_millis(i as u64);
            while Instant::now() < due {
                std::thread::sleep(Duration::from_micros(100));
            }
            at.push(Instant::now());
            fake.capture(InputEvent::MotionRelative { dx: 1.0, dy: 0.0 });
        }
        at
    })
    .join()
    .unwrap();

    let target = n as f64;
    tokio::time::timeout(LIMIT, async {
        loop {
            let done = b.fake.timeline()[start..]
                .iter()
                .any(|(_, e)| matches!(e, InputEvent::MotionAbsolute { x, .. } if *x >= target));
            if done {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("not all the movements arrived");

    // Movement `i` (x = i + 1) arrived when B's cursor reached that x.
    let timeline = b.fake.timeline();
    let positions: Vec<(Instant, f64)> = timeline[start..]
        .iter()
        .filter_map(|(t, e)| match e {
            InputEvent::MotionAbsolute { x, .. } => Some((*t, *x)),
            _ => None,
        })
        .collect();
    let mut latencies = Vec::with_capacity(n);
    let mut j = 0;
    for (i, sent_at) in sent.iter().enumerate() {
        let x = (i + 1) as f64;
        while positions[j].1 < x {
            j += 1;
        }
        latencies.push(positions[j].0.saturating_duration_since(*sent_at));
    }
    if std::env::var_os("TANDEM_BENCH_DUMP").is_some() {
        let slow: Vec<(usize, u128)> = latencies
            .iter()
            .enumerate()
            .filter(|(_, d)| **d > Duration::from_millis(3))
            .map(|(i, d)| (i, d.as_micros()))
            .collect();
        println!("slow: {slow:?}");
    }
    latencies.sort_unstable();
    Run { sorted: latencies }
}

/// In CI: up to three batches of 1000 movements; it fails only if in all three p99 exceeds
/// 20 ms. An engine regression shows in all of them; a scheduler pause on a
/// loaded machine (a shared runner, other processes) only in some. The engine's
/// measurement reaches both machines' UI, with the link.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn input_latency_bench_in_ci() {
    let pair = InputPair::start(
        vec![screen("a-1", 1920, 1080)],
        vec![screen("b-1", 3440, 1440)],
        opts(),
    )
    .await
    .unwrap();
    let mut best = Duration::MAX;
    for _ in 0..3 {
        let run = bench(&pair, 1000).await;
        println!(
            "capture → injection latency: p50 {:?}, p99 {:?}, max {:?} ({} movements)",
            run.p50(),
            run.p99(),
            run.max(),
            run.sorted.len()
        );
        best = best.min(run.p99());
        if best <= Duration::from_millis(20) {
            break;
        }
    }
    assert!(
        best <= Duration::from_millis(20),
        "large regression: the best p99 of three batches is {best:?}"
    );

    // The engine's own measurement: published by the receiver (B) and the sender (A).
    let measured = pair
        .b
        .until_input(LIMIT, |s| s.latency.is_some_and(|l| l.count > 0))
        .await
        .unwrap()
        .latency
        .unwrap();
    assert_eq!(measured.link, Some(Link::Usb4));
    assert!(measured.p50_us <= measured.p99_us && measured.p99_us <= measured.max_us);
    let reported = pair
        .a
        .until_input(LIMIT, |s| s.latency.is_some())
        .await
        .unwrap()
        .latency
        .unwrap();
    assert_eq!(reported.link, Some(Link::Usb4));
    pair.stop().await;
}

/// Locally and in release: DESIGN §7.1's target for USB-C/Ethernet (p99 ≤ 1 ms) on
/// loopback, with 5000 movements.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench"]
async fn input_latency_bench_target() {
    let pair = InputPair::start(
        vec![screen("a-1", 1920, 1080)],
        vec![screen("b-1", 7680, 1440)],
        opts(),
    )
    .await
    .unwrap();
    let run = bench(&pair, 5000).await;
    println!(
        "capture → injection latency: p50 {:?}, p99 {:?}, max {:?} ({} movements)",
        run.p50(),
        run.p99(),
        run.max(),
        run.sorted.len()
    );
    assert!(run.p99() <= Duration::from_millis(1), "p99 {:?}", run.p99());
    pair.stop().await;
}

/// T-128: with the live cursor requested on both machines (the worst case: both sample
/// and send the samples), input latency does not get worse. In CI, the same large
/// regression as without a subscription; the fine comparison is the one below.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn input_latency_with_the_live_cursor_watched_in_ci() {
    let pair = InputPair::start(
        vec![screen("a-1", 1920, 1080)],
        vec![screen("b-1", 3440, 1440)],
        opts(),
    )
    .await
    .unwrap();
    let _watching = (pair.a.agent.watch_cursor(), pair.b.agent.watch_cursor());
    let mut best = Duration::MAX;
    for _ in 0..3 {
        let run = bench(&pair, 1000).await;
        println!(
            "latency with the live cursor: p50 {:?}, p99 {:?}, max {:?}",
            run.p50(),
            run.p99(),
            run.max()
        );
        best = best.min(run.p99());
        if best <= Duration::from_millis(20) {
            break;
        }
    }
    assert!(
        best <= Duration::from_millis(20),
        "large regression with the live cursor: the best p99 of three batches is {best:?}"
    );
    // It was really being published: the bench moved the cursor.
    assert!(pair.a.agent.cursor_published() + pair.b.agent.cursor_published() > 0);
    pair.stop().await;
}

/// Locally and in release: p99 with the live cursor requested does not exceed that of without
/// it plus 0.3 ms (measurement noise; on a machine with another load the noise is larger than that: run it at rest). Each side is the best of three batches of 5000
/// movements, alternated: a scheduler pause on the machine ruins a single
/// batch (it shows even without the live cursor), not all three.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench"]
async fn input_latency_does_not_get_worse_with_the_live_cursor_watched() {
    let pair = InputPair::start(
        vec![screen("a-1", 1920, 1080)],
        vec![screen("b-1", 7680, 1440)],
        opts(),
    )
    .await
    .unwrap();
    let (mut without, mut with) = (Duration::MAX, Duration::MAX);
    for _ in 0..3 {
        let run = bench(&pair, 5000).await;
        without = without.min(run.p99());
        let watching = (pair.a.agent.watch_cursor(), pair.b.agent.watch_cursor());
        let run = bench(&pair, 5000).await;
        with = with.min(run.p99());
        drop(watching);
    }
    println!("best p99 without the live cursor {without:?}; with it {with:?}");
    assert!(
        with <= without + Duration::from_micros(300),
        "the live cursor worsens latency: {with:?} against {without:?}"
    );
    pair.stop().await;
}
