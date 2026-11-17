//! Banco de latencia de la entrada (T-093, DESIGN §7.1): dos agentes por QUIC real en
//! loopback, captura e inyección simuladas, 1000 movimientos por segundo.
//!
//! La latencia de cada movimiento es desde que la captura simulada lo entrega hasta que
//! el inyector del otro lado pone el cursor en (o más allá de) su posición; los dos lados
//! están en el mismo proceso, así que se mide con un solo reloj. Además se comprueba que
//! la medición propia del motor (marcas del servidor + desfase de los latidos) llegue a
//! las dos UIs.
//!
//! - En CI solo falla con una regresión grande (p99 > 20 ms): los runners son
//!   compartidos y el objetivo real no se puede exigir ahí.
//! - El objetivo (p99 ≤ 1 ms) se comprueba en local, en release:
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

/// Percentil `q` (0–1) de una lista ordenada.
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

/// Cruza el cursor de A a B y manda `n` movimientos de 1 px a 1000 por segundo desde un
/// hilo propio (como el de una captura real). Devuelve la latencia de cada uno.
async fn bench(pair: &InputPair, n: usize) -> Run {
    let a = &pair.a;
    let b = &pair.b;
    a.until_input(LIMIT, |s| s.session && s.server == a.machine())
        .await
        .unwrap();
    // Si el cursor quedó en B de una corrida anterior, vuelve a A por el borde.
    a.fake
        .capture(InputEvent::MotionRelative { dx: -1e6, dy: 0.0 });
    // Las pantallas de B tienen que haber llegado a A para que el borde exista.
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
            "el cursor no cruzó a B.\n A: {:?}\n B: {:?}\n estado de A: {:?}\n pantallas de B en A: {:?}",
            a.input(),
            b.input(),
            a.agent.state(),
            a.agent.inbox().screens.borrow().clone(),
        )
    });
    assert_eq!(entered, 0.0, "entra por el borde izquierdo de B");
    // Dos latidos con eco: el motor ya estima el desfase.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let start = b.fake.timeline().len();
    let fake = a.fake.clone();
    let sent = std::thread::spawn(move || {
        let mut at = Vec::with_capacity(n);
        let t0 = Instant::now();
        for i in 0..n {
            // Al ritmo de un mouse de 1000 Hz, sin acumular atraso.
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
    .expect("no llegaron todos los movimientos");

    // El movimiento `i` (x = i + 1) llegó cuando el cursor de B alcanzó esa x.
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
        println!("lentos: {slow:?}");
    }
    latencies.sort_unstable();
    Run { sorted: latencies }
}

/// En CI: hasta tres tandas de 1000 movimientos; falla solo si en las tres el p99 pasa de
/// 20 ms. Una regresión del motor se ve en todas; una pausa del planificador de una
/// máquina cargada (un runner compartido, otros procesos) solo en alguna. La medición
/// del motor llega a la UI de las dos máquinas, con el enlace.
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
            "latencia captura → inyección: p50 {:?}, p99 {:?}, máx {:?} ({} movimientos)",
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
        "regresión grande: el mejor p99 de tres tandas es {best:?}"
    );

    // La medición propia del motor: la publica quien recibe (B) y quien manda (A).
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

/// En local y en release: el objetivo de DESIGN §7.1 para USB-C/Ethernet (p99 ≤ 1 ms) en
/// loopback, con 5000 movimientos.
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
        "latencia captura → inyección: p50 {:?}, p99 {:?}, máx {:?} ({} movimientos)",
        run.p50(),
        run.p99(),
        run.max(),
        run.sorted.len()
    );
    assert!(run.p99() <= Duration::from_millis(1), "p99 {:?}", run.p99());
    pair.stop().await;
}
