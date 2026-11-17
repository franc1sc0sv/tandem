//! Métricas de red por camino (T-102, DESIGN §6): lo que ve la pantalla Red cuando se corta
//! un camino, y la prueba de ancho de banda sobre un Wi-Fi simulado con capacidad limitada.
//!
//! Con el reloj de tokio pausado: los pings, los tiempos de cambio y la capacidad del Wi-Fi
//! son exactos.

use std::time::Duration;

use tandem_core::Machine;
use tandem_link::manager::LinkKind;
use tandem_link::metrics::Metrics;
use tandem_link::metrics::probe::{receive_probe, send_probe};
use tandem_link::qos::LinkQos;
use tandem_proto::{Link, NetworkStatus, PathState, PathSwitch};
use tandem_sim::{Desk, Outcome, SimWifi};

/// La Mac con la PC prendida y los dos agentes viéndose por los tres caminos.
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
async fn cortar_un_camino_genera_el_evento_con_su_duración_y_cambia_los_estados() {
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
    // Desde la última respuesta del USB-C: como mucho un ping y su tiempo máximo.
    let took = took_ms.expect("un cambio por caída tiene duración");
    assert!((1..=1250).contains(&took), "{took} ms");
    // El historial ya muestra el USB-C sin respuesta.
    let usb = after.path(Link::Usb4).unwrap();
    assert_eq!(usb.rtt_us, None);
    assert_eq!(usb.history.last().and_then(|s| s.rtt_us), None);

    // La otra máquina ve lo mismo.
    let pc = d.agent(Machine::B).network_status();
    assert_eq!(pc.primary().map(|p| p.link), Some(Link::Ethernet), "{pc:?}");
    assert_eq!(
        pc.switches.first().map(|s| (s.from, s.to)),
        Some((Some(Link::Usb4), Some(Link::Ethernet)))
    );

    // Y si cae también el Ethernet, pasa al Wi-Fi.
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

/// Mide un Wi-Fi simulado de `capacity` bytes por segundo con la prueba de 3 s.
async fn measure(wifi: &SimWifi) -> u64 {
    let (mut w, mut r) = wifi.data_pipe();
    let answer = tokio::spawn(async move { receive_probe(&mut r, 0).await.unwrap() });
    send_probe(&mut w, Duration::from_secs(3)).await.unwrap();
    drop(w);
    answer.await.unwrap().bytes_per_sec().unwrap()
}

fn near(got: u64, want: u64) -> bool {
    got.abs_diff(want) * 100 <= want * 2
}

#[tokio::test(start_paused = true)]
async fn la_medición_da_lo_que_da_el_wifi_simulado_y_fija_el_tope() {
    const CAPACITY: u64 = 8_000_000;
    let wifi = SimWifi::new(CAPACITY, Duration::from_millis(2), 256 * 1024);
    let bps = measure(&wifi).await;
    assert!(near(bps, CAPACITY), "{bps} B/s");

    // Lo medido queda en las métricas y el tope de los datos es su mitad (T-101).
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

    // Si el Wi-Fi empeora, la medición siguiente lo dice.
    wifi.set_capacity(CAPACITY / 4);
    let worse = measure(&wifi).await;
    assert!(near(worse, CAPACITY / 4), "{worse} B/s");
}
