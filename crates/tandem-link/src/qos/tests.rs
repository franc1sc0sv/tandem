use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::AsyncWrite;
use tokio::time::Instant;

use super::*;

/// Anota cuándo y cuánto se escribió.
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

/// La próxima pausa o reanudación, sin los cambios de tope.
async fn next_gate(events: &mut broadcast::Receiver<QosEvent>) -> QosEvent {
    loop {
        match events.recv().await.unwrap() {
            QosEvent::Cap { .. } => {}
            e => return e,
        }
    }
}

/// `lat` de latencia de la entrada, un evento por milisegundo, durante `dur`.
async fn input_for(qos: &Qos, lat: Duration, dur: Duration) {
    let end = Instant::now() + dur;
    while Instant::now() < end {
        qos.record_input_latency(lat);
        tokio::time::sleep(ms(1)).await;
    }
}

#[test]
fn la_entrada_va_primero_y_los_datos_al_final() {
    let order = [
        StreamClass::Input,
        StreamClass::Control,
        StreamClass::Clipboard,
        StreamClass::Data,
    ];
    assert!(order.windows(2).all(|w| w[0].priority() > w[1].priority()));
    // Un stream sin clase (quinn arranca en 0) es de control.
    assert_eq!(StreamClass::Control.priority(), 0);
    assert_eq!(StreamClass::of(Channel::Input), StreamClass::Input);
    assert_eq!(StreamClass::of(Channel::Clipboard), StreamClass::Clipboard);
}

#[tokio::test(start_paused = true)]
async fn el_tope_sigue_al_camino_y_a_las_preferencias() {
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
    // Por cable, sin tope; de vuelta al Wi-Fi, el de antes.
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
async fn los_datos_por_wifi_no_pasan_el_tope_y_por_cable_van_libres() {
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
        assert!(window <= 1_000_000, "{window} bytes en 1 s");
    }
    assert!(rec.total() >= 2_800_000, "solo {}", rec.total());

    // Por cable no hay tope: sale todo sin esperar.
    qos.set_link(Some(LinkKind::Usb4));
    let rec = Recorder::default();
    let mut data = qos.data_stream(rec.clone());
    let t1 = Instant::now();
    data.write_all(&vec![0u8; 50_000_000]).await.unwrap();
    assert_eq!(rec.total(), 50_000_000);
    assert_eq!(Instant::now(), t1);
}

#[tokio::test(start_paused = true)]
async fn la_entrada_atrasada_pausa_los_datos_y_sin_entrada_vuelven() {
    let qos = Qos::default();
    qos.set_link(Some(LinkKind::Wifi));
    let mut events = qos.events();
    let gate = qos.watch_gate();
    let t0 = Instant::now();
    // 2,1 s de entrada a 40 ms (el objetivo del Wi-Fi es 20 ms).
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

    // En pausa no sale nada; sin entrada, la pausa termina sola (ventana + histéresis).
    let rec = Recorder::default();
    let mut data = qos.data_stream(rec.clone());
    let stopped = Instant::now();
    data.write_all(b"hola").await.unwrap();
    let waited = Instant::now() - stopped;
    assert!((ms(1900)..ms(2300)).contains(&waited), "esperó {waited:?}");
    assert_eq!(rec.total(), 4);
    assert_eq!(qos.gate(), DataGate::Open);
    assert!(matches!(
        next_gate(&mut events).await,
        QosEvent::Resumed {
            link: Some(LinkKind::Wifi),
            ..
        }
    ));
}

#[tokio::test(start_paused = true)]
async fn cambiar_de_camino_levanta_la_pausa() {
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
async fn la_entrada_cerca_del_objetivo_baja_un_poco_el_tope() {
    let qos = wifi_at(8_000_000);
    assert_eq!(qos.cap(), Some(4_000_000));
    // Bien: nada cambia.
    input_for(&qos, ms(3), ms(3000)).await;
    assert_eq!(qos.cap(), Some(4_000_000));
    // Cerca del objetivo (16 de 20 ms): un poco menos, una sola vez en la ventana.
    input_for(&qos, ms(16), ms(900)).await;
    assert_eq!(qos.cap(), Some(2_800_000));
}

#[tokio::test(start_paused = true)]
async fn pasado_el_objetivo_el_tope_baja_a_la_mitad_una_vez_por_ventana() {
    let qos = wifi_at(8_000_000);
    input_for(&qos, ms(25), ms(900)).await;
    assert_eq!(qos.cap(), Some(2_000_000));
    assert!(qos.gate().is_open());
}

#[tokio::test(start_paused = true)]
async fn con_margen_el_tope_vuelve_de_a_poco_y_en_el_medio_se_queda() {
    let qos = wifi_at(8_000_000);
    input_for(&qos, ms(25), ms(900)).await;
    // En la franja del medio (entre 10 y 15 ms) no sube ni baja, ya con la ventana limpia.
    input_for(&qos, ms(12), ms(2100)).await;
    let held = qos.cap().unwrap();
    assert!(held < 4_000_000);
    input_for(&qos, ms(12), ms(3000)).await;
    assert_eq!(qos.cap(), Some(held));
    // Con margen sube, de a poco y sin pasar el configurado.
    input_for(&qos, ms(3), ms(2100)).await;
    let up = qos.cap().unwrap();
    assert!(up > held && up < held * 2, "{held} → {up}");
    input_for(&qos, ms(3), ms(30_000)).await;
    assert_eq!(qos.cap(), Some(4_000_000));
    assert_eq!(qos.configured_cap(), Some(4_000_000));
}

#[tokio::test(start_paused = true)]
async fn una_pausa_deja_el_tope_a_la_mitad_al_volver() {
    let qos = wifi_at(8_000_000);
    input_for(&qos, ms(200), ms(2100)).await;
    assert!(!qos.gate().is_open());
    // A la mitad una vez por ventana mientras la entrada siguió pasada (4 → 2 → 1 MB/s) y
    // otra más por la pausa.
    assert_eq!(qos.cap(), Some(500_000));
    // Una medición nueva vuelve a usar el tope entero.
    qos.set_measured(6_000_000);
    assert_eq!(qos.cap(), Some(3_000_000));
}

#[tokio::test(start_paused = true)]
async fn un_qos_por_camino_compartido_y_con_el_data_del_puesto() {
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
    // Uno que se crea después también las toma.
    let usb4 = set.get(LinkKind::Usb4);
    assert_eq!(usb4.cap(), None);
    usb4.set_link(Some(LinkKind::Wifi));
    assert_eq!(usb4.cap(), Some(UNMEASURED_WIFI / 5));
}

#[tokio::test(start_paused = true)]
async fn la_etiqueta_se_recupera_cuando_el_otro_vuelve_a_anunciar_cable() {
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
async fn la_clase_del_otro_no_se_pierde_si_la_propia_no_se_conoce() {
    let qos = Qos::default();
    qos.set_peer_link(Some(LinkKind::Wifi));
    assert_eq!(qos.link(), Some(LinkKind::Wifi));
    qos.set_link(Some(LinkKind::Usb4));
    assert_eq!(qos.link(), Some(LinkKind::Wifi));
}

#[tokio::test(start_paused = true)]
async fn cerrarse_una_conexion_vieja_no_borra_la_clase_de_la_nueva() {
    let qos = Qos::default();
    qos.set_link(Some(LinkKind::Ethernet));
    qos.set_peer_link_from(1, LinkKind::Wifi);
    // La nueva conexión anuncia la suya y después se cierra la vieja.
    qos.set_peer_link_from(2, LinkKind::Wifi);
    qos.clear_peer_link_of(1);
    assert_eq!(qos.link(), Some(LinkKind::Wifi));
    // Al cerrarse la que la anunció, sí se borra.
    qos.clear_peer_link_of(2);
    assert_eq!(qos.link(), Some(LinkKind::Ethernet));
}
