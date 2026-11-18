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
use tandem_link::qos::{LinkQos, Qos, input_p99_target};
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

/// Revisión de T-102: la prueba de ancho de banda va sin tope pero respeta la pausa de
/// T-101. Si la entrada pasa su objetivo durante la prueba, se corta antes de los 3 s.
#[tokio::test(start_paused = true)]
async fn la_medición_se_corta_si_la_entrada_pasa_su_objetivo() {
    let wifi = SimWifi::new(8_000_000, Duration::from_millis(2), 256 * 1024);
    let qos = Qos::default();
    qos.set_link(Some(LinkKind::Wifi));
    let late = input_p99_target(Some(LinkKind::Wifi)) * 2;
    // Entrada a 1000 eventos/s, toda sobre el objetivo.
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
    // La pausa salta a los 2 s sobre el objetivo (T-101), antes del final de la prueba.
    assert!(took < Duration::from_secs(3), "{took:?}");
    assert!(took >= Duration::from_secs(2), "{took:?}");
    // El otro lado recibe el fin igual.
    answer.await.unwrap();
}

/// T-094: un cable USB4 que se enchufa a mitad de la sesión se detecta por su interfaz, se
/// vuelve el camino principal en las dos máquinas, y al desconectarlo la entrada pasa al
/// camino siguiente en menos de 1 s, sin esperar a la revisión de salud.
#[tokio::test(start_paused = true)]
async fn el_cable_usb4_que_aparece_se_vuelve_el_principal_y_su_baja_pasa_al_siguiente() {
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

    // Aparece la interfaz USB4 en las dos máquinas.
    d.plug_cable(LinkKind::Usb4);
    tokio::time::sleep(Duration::from_secs(3)).await;
    for a in [mac, pc] {
        let s = a.network_status();
        assert_eq!(s.primary().map(|p| p.link), Some(Link::Usb4), "{s:?}");
    }

    // Se desconecta: en menos de 1 s la entrada ya va por el Ethernet.
    d.unplug_cable(LinkKind::Usb4);
    tokio::time::sleep(Duration::from_millis(900)).await;
    for a in [mac, pc] {
        let s = a.network_status();
        assert_eq!(s.primary().map(|p| p.link), Some(Link::Ethernet), "{s:?}");
        let last = s.switches.first().expect("quedó el cambio en el historial");
        assert_eq!(
            (last.from, last.to),
            (Some(Link::Usb4), Some(Link::Ethernet))
        );
    }
}
