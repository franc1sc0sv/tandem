//! Gestor de enlaces con QUIC real por loopback. Cada enlace es un relé UDP
//! en su propio puerto delante del mismo agente: "desconectar el cable" es que
//! el relé tire los paquetes en los dos sentidos, sin avisar a nadie.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tandem_core::Machine;
use tandem_link::identity::{Identity, PublicKey};
use tandem_link::manager::{
    HealthConfig, LinkCandidate, LinkEvent, LinkKind, LinkManager, PlaneError, QuicProber,
    answer_pings,
};
use tandem_link::transport::{Connection, Endpoint, PeerTrust};
use tandem_proto::{Hello, Peer};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

const LOOPBACK: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
const TIMEOUT: Duration = Duration::from_secs(10);
/// Criterio de la tarea.
const MAX_SWITCH: Duration = Duration::from_millis(1500);

struct Only(PublicKey);

impl PeerTrust for Only {
    fn is_trusted(&self, key: &PublicKey) -> bool {
        *key == self.0
    }
}

/// Relé UDP de un solo cliente: lo que llega a `addr` va al agente desde un
/// socket propio, y las respuestas vuelven al último cliente visto.
struct Relay {
    addr: SocketAddr,
    plugged: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl Relay {
    async fn new(target: SocketAddr) -> Self {
        let front = Arc::new(UdpSocket::bind(LOOPBACK).await.unwrap());
        let back = Arc::new(UdpSocket::bind(LOOPBACK).await.unwrap());
        back.connect(target).await.unwrap();
        let addr = front.local_addr().unwrap();
        let plugged = Arc::new(AtomicBool::new(true));
        let flag = plugged.clone();
        let task = tokio::spawn(async move {
            let mut client: Option<SocketAddr> = None;
            let mut up = vec![0u8; 65536];
            let mut down = vec![0u8; 65536];
            loop {
                tokio::select! {
                    r = front.recv_from(&mut up) => {
                        let Ok((n, from)) = r else { continue };
                        if flag.load(Ordering::SeqCst) {
                            client = Some(from);
                            let _ = back.send(&up[..n]).await;
                        }
                    }
                    r = back.recv(&mut down) => {
                        let Ok(n) = r else { continue };
                        if let (true, Some(c)) = (flag.load(Ordering::SeqCst), client) {
                            let _ = front.send_to(&down[..n], c).await;
                        }
                    }
                }
            }
        });
        Self {
            addr,
            plugged,
            task,
        }
    }

    fn plug(&self, on: bool) {
        self.plugged.store(on, Ordering::SeqCst);
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Desk {
    _pc: Arc<Endpoint>,
    _mac: Arc<Endpoint>,
    relays: Vec<(LinkKind, Relay)>,
    manager: LinkManager<Connection>,
    events: mpsc::UnboundedReceiver<LinkEvent>,
    seen: Vec<LinkEvent>,
}

impl Desk {
    /// La PC contesta pings; la Mac corre el gestor con un enlace por relé.
    async fn new(kinds: &[LinkKind]) -> Self {
        let mac_id = Identity::from_seed([1; 32]);
        let pc_id = Identity::from_seed([2; 32]);
        let pc = Arc::new(
            Endpoint::bind(LOOPBACK, &pc_id, Arc::new(Only(mac_id.public_key()))).unwrap(),
        );
        let mac = Arc::new(
            Endpoint::bind(LOOPBACK, &mac_id, Arc::new(Only(pc_id.public_key()))).unwrap(),
        );

        let server = pc.clone();
        tokio::spawn(async move {
            while let Some(conn) = server.accept().await {
                if let Ok(conn) = conn {
                    tokio::spawn(async move {
                        let hello = Hello::new(Peer::Agent(Machine::Pc));
                        let _ = answer_pings(conn, &hello).await;
                    });
                }
            }
        });

        let target = pc.local_addr().unwrap();
        let mut relays = Vec::new();
        for &kind in kinds {
            relays.push((kind, Relay::new(target).await));
        }
        let candidates: Vec<_> = relays
            .iter()
            .map(|(kind, r)| LinkCandidate {
                kind: *kind,
                addr: r.addr,
            })
            .collect();
        let prober = QuicProber::new(mac.clone(), Hello::new(Peer::Agent(Machine::Mac)));
        let (manager, events) =
            LinkManager::spawn(&candidates, prober, HealthConfig::default()).unwrap();
        Self {
            _pc: pc,
            _mac: mac,
            relays,
            manager,
            events,
            seen: Vec::new(),
        }
    }

    fn relay(&self, kind: LinkKind) -> &Relay {
        &self.relays.iter().find(|(k, _)| *k == kind).unwrap().1
    }

    async fn until_active(&mut self, kind: Option<LinkKind>) -> Duration {
        let start = Instant::now();
        loop {
            let e = tokio::time::timeout(TIMEOUT, self.events.recv())
                .await
                .expect("no llegó el cambio de enlace")
                .expect("el gestor se detuvo");
            self.seen.push(e);
            if e.to == kind {
                return start.elapsed();
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn caída_y_vuelta_del_enlace_preferido() {
    use LinkKind::*;
    let mut desk = Desk::new(&[Usb4, Ethernet, Wifi]).await;
    desk.until_active(Some(Usb4)).await;
    let usb4 = desk.manager.active().unwrap();
    assert_eq!(usb4.addr, desk.relay(Usb4).addr);
    // La conexión entregada sirve para tráfico real.
    assert!(usb4.handle.max_datagram_size().is_some());

    desk.relay(Usb4).plug(false);
    let took = desk.until_active(Some(Ethernet)).await;
    println!("USB4 → Ethernet en {took:?}");
    assert!(took < MAX_SWITCH, "conmutó en {took:?}");
    assert!(desk.manager.data_plane_available());

    desk.relay(Usb4).plug(true);
    let took = desk.until_active(Some(Usb4)).await;
    println!("Ethernet → USB4 en {took:?}");
    assert_eq!(desk.manager.active().unwrap().kind, Usb4);

    // Ethernet nunca cayó: los eventos fueron exactamente estos, en orden.
    let tail: Vec<_> = desk.seen[desk.seen.len() - 2..]
        .iter()
        .map(|e| (e.from, e.to))
        .collect();
    assert_eq!(
        tail,
        [(Some(Usb4), Some(Ethernet)), (Some(Ethernet), Some(Usb4))]
    );
    for (i, e) in desk.seen.iter().enumerate() {
        assert_eq!(e.seq, i as u64);
        assert_eq!(e.from, i.checked_sub(1).and_then(|p| desk.seen[p].to));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn con_solo_wifi_no_hay_plano_de_datos() {
    use LinkKind::*;
    let mut desk = Desk::new(&[Ethernet, Wifi]).await;
    desk.relay(Ethernet).plug(false);
    desk.until_active(Some(Wifi)).await;
    // Ethernet nunca respondió, así que Wi-Fi sigue activo.
    assert_eq!(desk.manager.active().unwrap().kind, Wifi);

    assert!(!desk.manager.data_plane_available());
    assert_eq!(
        desk.manager.data_plane().err(),
        Some(PlaneError::RequiresCable { active: Wifi })
    );
    assert_eq!(desk.manager.control_plane().unwrap().kind, Wifi);

    desk.relay(Ethernet).plug(true);
    desk.until_active(Some(Ethernet)).await;
    assert_eq!(desk.manager.data_plane().unwrap().kind, Ethernet);
}
