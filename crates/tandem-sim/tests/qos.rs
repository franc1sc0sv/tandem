//! Datos por Wi-Fi sin atrasar la entrada (T-101, DECISIONS A2, DESIGN §6 y §7.1).
//!
//! Las dos puntas en el mismo proceso, unidas por un [`SimWifi`] de 8 MB/s con 2 ms de
//! propagación y una cola de 256 KiB en el punto de acceso. Una manda datos a tope por un
//! [`DataStream`] y entrada a 1000 eventos/s; la otra mide cuánto tarda cada evento y se
//! lo devuelve al [`Qos`] de la conexión (lo que hará la medición de T-093). Todo con el
//! reloj de tokio pausado: los tiempos son exactos.

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
/// El objetivo simulado: el p99 del Wi-Fi de DESIGN §7.1.
const TARGET: Duration = Duration::from_millis(20);

fn secs(s: f64) -> Duration {
    Duration::from_secs_f64(s)
}

/// Latencias medidas en el otro lado: cuándo salió cada evento y cuánto tardó.
type Latencies = Arc<Mutex<Vec<(Instant, Duration)>>>;

/// 1000 eventos de entrada por segundo hasta `end`; el otro lado mide cada uno al llegar y
/// se lo pasa a `qos` si hay.
fn input(wifi: &SimWifi, qos: Option<Qos>, end: Instant) -> (JoinHandle<()>, Latencies) {
    let lat: Latencies = Arc::default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(Instant, Instant)>();
    let wifi = wifi.clone();
    let send = tokio::spawn(async move {
        let mut tick = time::interval(Duration::from_millis(1));
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        while Instant::now() < end {
            tick.tick().await;
            // Un evento de movimiento o una tecla: unas decenas de bytes con QUIC.
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

/// p99 de los eventos que salieron en [from, to).
fn p99(lat: &Latencies, from: Instant, to: Instant) -> Duration {
    let mut v: Vec<Duration> = lat
        .lock()
        .unwrap()
        .iter()
        .filter(|(t, _)| (from..to).contains(t))
        .map(|(_, l)| *l)
        .collect();
    assert!(v.len() > 100, "pocas muestras: {}", v.len());
    v.sort();
    v[(v.len() * 99).div_ceil(100) - 1]
}

/// Bytes de datos que salieron en [from, to).
fn data_between(wifi: &SimWifi, from: Instant, to: Instant) -> usize {
    wifi.data_sent()
        .iter()
        .filter(|(t, _)| (from..to).contains(t))
        .map(|(_, n)| n)
        .sum()
}

#[tokio::test(start_paused = true)]
async fn sin_tope_los_datos_le_meten_latencia_a_la_entrada() {
    // Lo que el tope evita: datos a todo lo que da el Wi-Fi llenan la cola del punto de
    // acceso y cada evento espera detrás de 256 KiB.
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
    assert!(p > Duration::from_millis(30), "p99 sin tope: {p:?}");
}

/// Lo que pasó en una corrida con el `Qos` en medio.
struct Run {
    t0: Instant,
    end: Instant,
    wifi: SimWifi,
    lat: Latencies,
    /// Pausas y reanudaciones, en orden (sin los cambios de tope).
    gates: Vec<(Instant, QosEvent)>,
}

impl Run {
    fn at(&self, s: f64) -> Instant {
        self.t0 + secs(s)
    }

    /// Bytes por segundo de datos en [from, to) (en segundos desde el principio).
    fn rate(&self, from: f64, to: f64) -> f64 {
        data_between(&self.wifi, self.at(from), self.at(to)) as f64 / (to - from)
    }

    fn p99(&self, from: f64, to: f64) -> Duration {
        p99(&self.lat, self.at(from), self.at(to))
    }
}

/// Datos a tope y entrada a 1000 eventos/s durante `total` segundos; la capacidad del
/// Wi-Fi cambia según `changes` (segundo, bytes por segundo).
async fn run(total: f64, changes: &[(f64, u64)]) -> Run {
    let wifi = SimWifi::new(CAPACITY, BASE, BUFFER);
    let qos = Qos::default();
    qos.set_link(Some(LinkKind::Wifi));
    assert_eq!(input_p99_target(Some(LinkKind::Wifi)), TARGET);
    // Lo último medido del Wi-Fi: el tope por omisión es la mitad.
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

    // Ningún segundo pasa el tope.
    let log = wifi.data_sent();
    for (i, (start, _)) in log.iter().enumerate() {
        let window: usize = log[i..]
            .iter()
            .take_while(|(t, _)| *t <= *start + Duration::from_secs(1))
            .map(|(_, n)| n)
            .sum();
        assert!(window as u64 <= CAPACITY / 2, "{window} bytes en 1 s");
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
async fn datos_a_tope_por_wifi_con_entrada_a_1000_eventos_por_segundo() {
    // A los 5 s el Wi-Fi cae a 300 kB/s (interferencia fuerte): ni bajando el tope se
    // salva la entrada a tiempo. A los 8 s vuelve.
    let r = run(30.0, &[(5.0, 300_000), (8.0, CAPACITY)]).await;

    // Con el Wi-Fi bien, la entrada dentro del objetivo y los datos al tope (4 MB/s).
    let steady = r.p99(1.0, 5.0);
    assert!(steady <= TARGET, "p99 con datos a tope: {steady:?}");
    assert!(r.rate(1.0, 5.0) >= 3_800_000.0, "{}", r.rate(1.0, 5.0));

    // La caída: la entrada pasa el objetivo durante 2 s y los datos se pausan.
    let (paused_at, paused) = r.gates.first().expect("sin pausa");
    match paused {
        QosEvent::Paused { p99, target, link } => {
            assert_eq!(*target, TARGET);
            assert!(*p99 > TARGET);
            assert_eq!(*link, Some(LinkKind::Wifi));
        }
        other => panic!("se esperaba la pausa: {other:?}"),
    }
    let after_drop = *paused_at - r.at(5.0);
    assert!(
        (secs(2.0)..secs(2.5)).contains(&after_drop),
        "pausó {after_drop:?} después de la caída"
    );
    let (resumed_at, resumed) = r.gates.get(1).expect("sin reanudar");
    assert!(matches!(resumed, QosEvent::Resumed { .. }), "{resumed:?}");
    // En pausa no sale nada más que lo que ya estaba escribiéndose.
    let during = data_between(&r.wifi, *paused_at, *resumed_at);
    assert!(during <= 2 * CHUNK, "salieron {during} bytes en pausa");
    // Una sola pausa: con el Wi-Fi de vuelta no hay más.
    assert_eq!(r.gates.len(), 2, "{:?}", r.gates);

    // El caudal vuelve al tope solo porque volvió el Wi-Fi: antes de los 8 s no.
    assert!(r.rate(5.5, 8.0) < 1_000_000.0, "{}", r.rate(5.5, 8.0));
    assert!(r.rate(26.0, 30.0) >= 3_800_000.0, "{}", r.rate(26.0, 30.0));
    println!(
        "de 5,5 a 8 s: {} B/s; de 26 a 30 s: {} B/s",
        r.rate(5.5, 8.0),
        r.rate(26.0, 30.0)
    );
    let back = p99(&r.lat, *resumed_at + secs(1.0), r.end);
    assert!(back <= TARGET, "p99 después de reanudar: {back:?}");
}

#[tokio::test(start_paused = true)]
async fn con_un_wifi_que_no_vuelve_la_entrada_queda_dentro_del_objetivo() {
    // A los 5 s el Wi-Fi cae a 2 MB/s y se queda así: el tope de 4 MB/s ya no alcanza.
    let r = run(30.0, &[(5.0, 2_000_000)]).await;
    // Los datos bajan a lo que da el Wi-Fi sin atrasar la entrada.
    let p = r.p99(15.0, 30.0);
    println!("p99 de 15 a 30 s: {p:?}; datos: {} B/s", r.rate(15.0, 30.0));
    assert!(p <= TARGET, "p99 con el Wi-Fi malo: {p:?}");
    // Sin pausas y reanudaciones en ciclo.
    let late: Vec<_> = r.gates.iter().filter(|(t, _)| *t >= r.at(12.0)).collect();
    assert!(late.is_empty(), "{late:?}");
    // Y los datos siguen saliendo, a una parte razonable de lo que queda.
    let rate = r.rate(15.0, 30.0);
    assert!(rate >= 800_000.0, "solo {rate} B/s");
}
