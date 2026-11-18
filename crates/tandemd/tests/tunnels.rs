//! Túneles entre dos agentes reales por QUIC en loopback (T-103): un cliente abre el túnel
//! por la IPC de A, la salida que produce B llega a A, el camino se corta a mitad del flujo,
//! la salida sigue por otro camino (otra conexión) completa y en orden, y cerrar desde A
//! suelta los recursos en B.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tandem_core::Machine;
use tandem_link::identity::{Identity, PublicKey};
use tandem_link::transport::{Connection, Endpoint, PeerTrust};
use tandem_proto::{
    AgentToUi, ClientKind, Peer, Project, Reply, Tunnel, TunnelId, TunnelState, UiCommand,
};
use tandem_tunnel::{Read, Resource};
use tandemd::ipc::{IpcClient, mem};
use tandemd::tunnels::{CLIENTS_ONLY, QuicDialer};
use tandemd::{Agent, AgentConfig, StateStore, daemon, peer};
use tokio::io::DuplexStream;
use tokio::sync::oneshot;

const TIMEOUT: Duration = Duration::from_secs(30);

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .expect("la prueba superó el tiempo máximo")
}

struct Keys(BTreeSet<PublicKey>);

impl PeerTrust for Keys {
    fn is_trusted(&self, key: &PublicKey) -> bool {
        self.0.contains(key)
    }
}

fn agent(dir: &std::path::Path, machine: Machine) -> Arc<Agent> {
    let cfg = AgentConfig::from_toml(&format!("machine = \"{machine}\"\n")).unwrap();
    let store = StateStore::new(dir.join(format!("{machine}.json")));
    Arc::new(Agent::new(cfg, store))
}

/// Una conexión más de A hacia B (un camino), atendida por B como las del agente real.
async fn path(a: &Endpoint, b: &Endpoint, pc: &Arc<Agent>) -> Connection {
    let (to_b, to_a) =
        within(async { tokio::join!(a.connect(b.local_addr().unwrap()), b.accept()) }).await;
    tokio::spawn(peer::accept_streams(to_a.unwrap().unwrap(), pc.clone()));
    to_b.unwrap()
}

/// Un cliente (o la UI) conectado a la IPC en memoria de `agent`.
async fn ipc(
    agent: &Arc<Agent>,
    peer: Peer,
    stops: &mut Vec<oneshot::Sender<()>>,
) -> IpcClient<DuplexStream> {
    let (listener, connector) = mem::pair();
    let (stop, stopped) = oneshot::channel::<()>();
    stops.push(stop);
    tokio::spawn(daemon::run(agent.clone(), listener, async {
        let _ = stopped.await;
    }));
    IpcClient::handshake_as(connector.connect().unwrap(), peer)
        .await
        .unwrap()
}

struct Flag(Arc<AtomicBool>);

impl Resource for Flag {
    fn close(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn received(a: &Agent, id: TunnelId) -> u64 {
    a.tunnels()
        .origin
        .flows(id)
        .first()
        .map_or(0, |f| f.received)
}

async fn until(mut done: impl FnMut() -> bool) {
    within(async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
}

/// El siguiente aviso de túneles que le llega a la UI.
async fn next_tunnels(ui: &mut IpcClient<DuplexStream>) -> Vec<Tunnel> {
    within(async {
        loop {
            if let Some(AgentToUi::Tunnels(groups)) = ui.next_event().await.unwrap() {
                return groups
                    .into_iter()
                    .flat_map(|g| g.tunnels)
                    .map(|v| v.tunnel)
                    .collect();
            }
        }
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn la_salida_sobrevive_al_corte_del_camino_y_cerrar_suelta_los_recursos() {
    let dir = tempfile::tempdir().unwrap();
    let mac = agent(dir.path(), Machine::A);
    let pc = agent(dir.path(), Machine::B);
    let (mac_id, pc_id) = (Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
    let trust = |id: &Identity| Arc::new(Keys(BTreeSet::from([id.public_key()])));
    let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    let mac_ep = Endpoint::bind(loopback, &mac_id, trust(&pc_id)).unwrap();
    let pc_ep = Endpoint::bind(loopback, &pc_id, trust(&mac_id)).unwrap();

    // Dos caminos: el primero se corta a mitad de la salida.
    let first = path(&mac_ep, &pc_ep, &pc).await;
    let second = path(&mac_ep, &pc_ep, &pc).await;
    let origin = &mac.tunnels().origin;
    origin.set_link(Some(Arc::new(QuicDialer::new(first.clone(), Machine::A))));

    let mut stops = Vec::new();
    let mut claude = ipc(&mac, Peer::Client(ClientKind::Mcp), &mut stops).await;
    let mut ui = ipc(&mac, Peer::Ui, &mut stops).await;

    // La UI solo mira.
    let open = UiCommand::OpenTunnel {
        title: String::new(),
        project: Some(Project {
            name: "tandem".into(),
            root: "/src/tandem".into(),
        }),
        target: Machine::B,
        session: "claude-1".into(),
        workspace: None,
    };
    assert_eq!(
        within(ui.request(open.clone())).await.unwrap(),
        Reply::Rejected(CLIENTS_ONLY.into())
    );

    let Reply::Tunnel { tunnel: t, .. } = within(claude.request(open)).await.unwrap() else {
        panic!("se esperaba el túnel abierto");
    };
    assert_eq!(t.title, "tandem", "sin título, el del proyecto");
    assert_eq!((t.origin, t.target), (Machine::A, Machine::B));
    assert_eq!(next_tunnels(&mut ui).await, [(*t).clone()]);

    // B produce la salida; A la recibe.
    let ht = pc.tunnels().host.get(t.id).expect("B hospeda el túnel");
    let closed = Arc::new(AtomicBool::new(false));
    ht.add_resource(Box::new(Flag(closed.clone()))).unwrap();
    let w = ht.add_flow("cargo test · stdout").unwrap();
    let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    let half = data.len() / 2;
    w.write(&data[..half]);
    until(|| received(&mac, t.id) > 0).await;

    // Se corta el camino a mitad del flujo, B sigue escribiendo y el enlace pasa al otro.
    first.close("cable desenchufado");
    w.write(&data[half..]);
    w.finish();
    origin.set_link(Some(Arc::new(QuicDialer::new(second.clone(), Machine::A))));
    until(|| {
        origin
            .flows(t.id)
            .first()
            .is_some_and(|f| f.ended == Some(data.len() as u64))
    })
    .await;
    let status = &origin.flows(t.id)[0];
    assert_eq!(status.received, data.len() as u64);
    assert_eq!(status.lost, 0);
    assert_eq!(status.info.label, "cargo test · stdout");
    assert_eq!(
        origin.read(t.id, w.id(), 0, usize::MAX),
        Some(Read::Data {
            offset: 0,
            bytes: data
        })
    );

    // La UI tampoco puede cerrarlo (D9): el túnel y su recurso siguen abiertos.
    assert_eq!(
        within(ui.request(UiCommand::CloseTunnel { id: t.id }))
            .await
            .unwrap(),
        Reply::Rejected(CLIENTS_ONLY.into())
    );
    assert!(!closed.load(Ordering::SeqCst));
    assert!(ht.is_open());

    // Cerrar desde el origen suelta los recursos en la destino.
    assert_eq!(
        within(claude.request(UiCommand::CloseTunnel { id: t.id }))
            .await
            .unwrap(),
        Reply::Done
    );
    assert!(
        closed.load(Ordering::SeqCst),
        "el recurso de B sigue abierto"
    );
    until(|| origin.tunnels()[0].state == TunnelState::Closed).await;
    assert_eq!(
        within(ui.request(UiCommand::GetTunnels)).await.unwrap(),
        Reply::Tunnels(vec![])
    );
    assert!(pc.tunnels().view().is_empty());
}
