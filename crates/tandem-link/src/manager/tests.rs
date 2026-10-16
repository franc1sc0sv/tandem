//! El gestor con un doble del transporte y el reloj de tokio pausado: los
//! tiempos medidos son exactos y no hay esperas reales.

use std::collections::HashMap;
use std::sync::Mutex;

use super::*;

/// Lo que hace un enlace simulado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wire {
    Up,
    /// Cable desconectado: nada responde ni falla, solo silencio.
    Cut,
    /// El par rechaza la conexión al instante.
    Refused,
}

#[derive(Debug, thiserror::Error)]
#[error("rechazado")]
struct Refused;

#[derive(Default)]
struct FakeNet {
    wires: Mutex<HashMap<SocketAddr, Wire>>,
    connects: Mutex<u32>,
}

impl FakeNet {
    fn set(&self, addr: SocketAddr, wire: Wire) {
        self.wires.lock().unwrap().insert(addr, wire);
    }

    fn get(&self, addr: SocketAddr) -> Wire {
        *self.wires.lock().unwrap().get(&addr).unwrap_or(&Wire::Cut)
    }
}

struct FakeProber(Arc<FakeNet>);

/// Handle = (dirección, número de conexión), para ver reconexiones.
type Handle = (SocketAddr, u32);

impl Prober for FakeProber {
    type Session = Handle;
    type Handle = Handle;
    type Error = Refused;

    async fn connect(&self, addr: SocketAddr) -> Result<Handle, Refused> {
        match self.0.get(addr) {
            Wire::Up => {
                let mut n = self.0.connects.lock().unwrap();
                *n += 1;
                Ok((addr, *n))
            }
            Wire::Refused => Err(Refused),
            Wire::Cut => std::future::pending().await,
        }
    }

    async fn ping(&self, s: &mut Handle) -> Result<(), Refused> {
        match self.0.get(s.0) {
            Wire::Up => {
                time::sleep(Duration::from_millis(2)).await;
                Ok(())
            }
            Wire::Refused => Err(Refused),
            Wire::Cut => std::future::pending().await,
        }
    }

    fn handle(s: &Handle) -> Handle {
        *s
    }
}

fn addr(kind: LinkKind) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 40000 + u16::from(kind.priority())))
}

fn cands(kinds: &[LinkKind]) -> Vec<LinkCandidate> {
    kinds
        .iter()
        .map(|&kind| LinkCandidate {
            kind,
            addr: addr(kind),
        })
        .collect()
}

struct Rig {
    net: Arc<FakeNet>,
    manager: LinkManager<Handle>,
    events: mpsc::UnboundedReceiver<LinkEvent>,
    seen: Vec<LinkEvent>,
}

impl Rig {
    fn new(kinds: &[LinkKind], up: &[LinkKind]) -> Self {
        let net = Arc::new(FakeNet::default());
        for k in up {
            net.set(addr(*k), Wire::Up);
        }
        let (manager, events) = LinkManager::spawn(
            &cands(kinds),
            FakeProber(net.clone()),
            HealthConfig::default(),
        )
        .unwrap();
        Self {
            net,
            manager,
            events,
            seen: Vec::new(),
        }
    }

    async fn next(&mut self) -> LinkEvent {
        let e = time::timeout(Duration::from_secs(30), self.events.recv())
            .await
            .expect("no llegó ningún evento")
            .expect("el gestor se detuvo");
        self.seen.push(e);
        e
    }

    async fn until_active(&mut self, kind: Option<LinkKind>) -> Duration {
        let start = Instant::now();
        while self.next().await.to != kind {}
        start.elapsed()
    }
}

/// Peor caso del diseño: falla justo después de un ping bueno, así que se nota
/// en el siguiente (1 s) más el tiempo máximo del Pong (0,25 s).
const MAX_SWITCH: Duration = Duration::from_millis(1500);

#[tokio::test(start_paused = true)]
async fn caída_del_preferido_conmuta_en_menos_de_1_5_s_y_vuelve() {
    use LinkKind::*;
    let mut rig = Rig::new(&ALL_KINDS, &ALL_KINDS);
    rig.until_active(Some(Usb4)).await;
    let first_usb4 = rig.manager.active().unwrap().handle;

    rig.net.set(addr(Usb4), Wire::Cut);
    let took = rig.until_active(Some(Ethernet)).await;
    assert!(took < MAX_SWITCH, "conmutó en {took:?}");
    assert!(rig.manager.data_plane_available());
    assert_eq!(rig.manager.active().unwrap().kind, Ethernet);

    rig.net.set(addr(Usb4), Wire::Up);
    let took = rig.until_active(Some(Usb4)).await;
    assert!(took <= Duration::from_secs(2), "volvió en {took:?}");
    // Es una conexión nueva, no la que se cortó.
    assert_ne!(rig.manager.active().unwrap().handle, first_usb4);
}

const ALL_KINDS: [LinkKind; 3] = LinkKind::ALL;

#[tokio::test(start_paused = true)]
async fn la_conmutación_respeta_el_presupuesto_en_cualquier_fase_del_ping() {
    // Corta el cable en distintos momentos entre dos pings.
    for offset_ms in [0u64, 1, 150, 500, 900, 999, 1000, 1001, 1299] {
        let mut rig = Rig::new(&ALL_KINDS, &ALL_KINDS);
        rig.until_active(Some(LinkKind::Usb4)).await;
        time::sleep(Duration::from_millis(offset_ms)).await;
        rig.net.set(addr(LinkKind::Usb4), Wire::Cut);
        let took = rig.until_active(Some(LinkKind::Ethernet)).await;
        assert!(took < MAX_SWITCH, "corte a +{offset_ms} ms: {took:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn un_rechazo_inmediato_cuenta_como_caída() {
    let mut rig = Rig::new(&ALL_KINDS, &ALL_KINDS);
    rig.until_active(Some(LinkKind::Usb4)).await;
    rig.net.set(addr(LinkKind::Usb4), Wire::Refused);
    let took = rig.until_active(Some(LinkKind::Ethernet)).await;
    assert!(took <= Duration::from_secs(1), "{took:?}");
}

#[tokio::test(start_paused = true)]
async fn con_solo_wifi_el_plano_de_datos_se_rechaza() {
    let mut rig = Rig::new(&ALL_KINDS, &[LinkKind::Wifi]);
    rig.until_active(Some(LinkKind::Wifi)).await;

    assert!(!rig.manager.data_plane_available());
    assert_eq!(
        rig.manager.data_plane(),
        Err(PlaneError::RequiresCable {
            active: LinkKind::Wifi
        })
    );
    let control = rig.manager.control_plane().unwrap();
    assert_eq!(control.kind, LinkKind::Wifi);
    assert_eq!(control.addr, addr(LinkKind::Wifi));
    assert!(!rig.seen.last().unwrap().data_plane);

    // Al conectar un cable, el plano de datos aparece solo.
    rig.net.set(addr(LinkKind::Ethernet), Wire::Up);
    let e = rig.next().await;
    assert_eq!(e.to, Some(LinkKind::Ethernet));
    assert!(e.data_plane);
    assert_eq!(rig.manager.data_plane().unwrap().kind, LinkKind::Ethernet);
}

#[tokio::test(start_paused = true)]
async fn sin_enlaces_no_hay_ningún_plano() {
    let rig = Rig::new(&ALL_KINDS, &[]);
    time::sleep(Duration::from_secs(5)).await;
    assert_eq!(rig.manager.active(), None);
    assert_eq!(rig.manager.data_plane(), Err(PlaneError::NoLink));
    assert_eq!(rig.manager.control_plane(), Err(PlaneError::NoLink));
    assert!(rig.manager.links().iter().all(|l| l.rtt.is_none()));
}

#[tokio::test(start_paused = true)]
async fn los_eventos_salen_en_orden_y_encadenados() {
    use LinkKind::*;
    let mut rig = Rig::new(&ALL_KINDS, &[Wifi]);
    rig.until_active(Some(Wifi)).await;
    rig.net.set(addr(Ethernet), Wire::Up);
    rig.until_active(Some(Ethernet)).await;
    rig.net.set(addr(Usb4), Wire::Up);
    rig.until_active(Some(Usb4)).await;
    rig.net.set(addr(Usb4), Wire::Cut);
    rig.net.set(addr(Ethernet), Wire::Cut);
    rig.until_active(Some(Wifi)).await;
    rig.net.set(addr(Wifi), Wire::Cut);
    rig.until_active(None).await;
    rig.net.set(addr(Usb4), Wire::Up);
    rig.until_active(Some(Usb4)).await;

    for (i, e) in rig.seen.iter().enumerate() {
        assert_eq!(e.seq, i as u64);
        let prev = i.checked_sub(1).and_then(|p| rig.seen[p].to);
        assert_eq!(e.from, prev, "evento {i}: {e:?}");
    }
    let tos: Vec<_> = rig.seen.iter().map(|e| e.to).collect();
    assert_eq!(tos.first(), Some(&Some(Wifi)));
    assert_eq!(tos.last(), Some(&Some(Usb4)));
    assert!(tos.contains(&None));
}

#[tokio::test(start_paused = true)]
async fn la_foto_muestra_la_salud_de_cada_enlace() {
    let mut rig = Rig::new(&[LinkKind::Wifi, LinkKind::Ethernet], &[LinkKind::Ethernet]);
    rig.until_active(Some(LinkKind::Ethernet)).await;
    let snap = rig.manager.snapshot();
    assert!(snap.data_plane_available());
    assert_eq!(
        snap.links.iter().map(|l| l.kind).collect::<Vec<_>>(),
        [LinkKind::Ethernet, LinkKind::Wifi]
    );
    assert_eq!(snap.links[0].rtt, Some(Duration::from_millis(2)));
    assert_eq!(snap.links[1].rtt, None);

    let mut watch = rig.manager.watch();
    rig.net.set(addr(LinkKind::Ethernet), Wire::Cut);
    let snap = time::timeout(
        Duration::from_secs(5),
        watch.wait_for(|s| s.active.is_none()),
    )
    .await
    .unwrap()
    .unwrap()
    .clone();
    assert!(!snap.data_plane_available());
}

#[tokio::test(start_paused = true)]
async fn soltar_el_gestor_detiene_los_pings() {
    let mut rig = Rig::new(&ALL_KINDS, &ALL_KINDS);
    rig.until_active(Some(LinkKind::Usb4)).await;
    let Rig {
        net,
        manager,
        mut events,
        ..
    } = rig;
    drop(manager);
    // La cola de eventos se cierra y ya no se abren conexiones.
    assert!(
        time::timeout(Duration::from_secs(5), async {
            while events.recv().await.is_some() {}
        })
        .await
        .is_ok()
    );
    let before = *net.connects.lock().unwrap();
    net.set(addr(LinkKind::Usb4), Wire::Cut);
    time::sleep(Duration::from_secs(5)).await;
    net.set(addr(LinkKind::Usb4), Wire::Up);
    time::sleep(Duration::from_secs(5)).await;
    assert_eq!(*net.connects.lock().unwrap(), before);
}

#[tokio::test]
async fn config_inválida_se_rechaza() {
    let net = Arc::new(FakeNet::default());
    let bad = HealthConfig {
        ping_interval: Duration::ZERO,
        ..HealthConfig::default()
    };
    assert_eq!(
        LinkManager::spawn(&cands(&ALL_KINDS), FakeProber(net.clone()), bad).err(),
        Some(ManagerError::ZeroDuration)
    );
    assert_eq!(
        LinkManager::spawn(&[], FakeProber(net.clone()), HealthConfig::default()).err(),
        Some(ManagerError::NoCandidates)
    );
    let dup = cands(&[LinkKind::Usb4, LinkKind::Usb4]);
    assert_eq!(
        LinkManager::spawn(&dup, FakeProber(net), HealthConfig::default()).err(),
        Some(ManagerError::DuplicateLink(LinkKind::Usb4))
    );
}

#[test]
fn el_presupuesto_por_omisión_cumple_el_diseño() {
    let c = HealthConfig::default();
    assert_eq!(c.ping_interval, Duration::from_secs(1));
    assert!(c.ping_interval + c.ping_timeout < MAX_SWITCH);
}
