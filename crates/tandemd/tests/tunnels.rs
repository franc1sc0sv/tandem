//! Tunnels between two real agents over QUIC on loopback (T-103): a client opens the tunnel
//! through A's IPC, the output B produces reaches A, the path is cut midway through the stream,
//! the output continues over another path (another connection) complete and in order, and closing from A
//! releases the resources on B.

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
        .expect("the test exceeded the maximum time")
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

/// One more connection from A to B (a path), served by B like the real agent's.
async fn path(a: &Endpoint, b: &Endpoint, pc: &Arc<Agent>) -> Connection {
    let (to_b, to_a) =
        within(async { tokio::join!(a.connect(b.local_addr().unwrap()), b.accept()) }).await;
    tokio::spawn(peer::accept_streams(to_a.unwrap().unwrap(), pc.clone()));
    to_b.unwrap()
}

/// A client (or the UI) connected to `agent`'s in-memory IPC.
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

/// The next tunnels notice that reaches the UI.
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
async fn the_output_survives_the_path_cut_and_closing_releases_the_resources() {
    let dir = tempfile::tempdir().unwrap();
    let mac = agent(dir.path(), Machine::A);
    let pc = agent(dir.path(), Machine::B);
    let (mac_id, pc_id) = (Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
    let trust = |id: &Identity| Arc::new(Keys(BTreeSet::from([id.public_key()])));
    let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    let mac_ep = Endpoint::bind(loopback, &mac_id, trust(&pc_id)).unwrap();
    let pc_ep = Endpoint::bind(loopback, &pc_id, trust(&mac_id)).unwrap();

    // Two paths: the first is cut midway through the output.
    let first = path(&mac_ep, &pc_ep, &pc).await;
    let second = path(&mac_ep, &pc_ep, &pc).await;
    let origin = &mac.tunnels().origin;
    origin.set_link(Some(Arc::new(QuicDialer::new(first.clone(), Machine::A))));

    let mut stops = Vec::new();
    let mut claude = ipc(&mac, Peer::Client(ClientKind::Mcp), &mut stops).await;
    let mut ui = ipc(&mac, Peer::Ui, &mut stops).await;

    // The UI only watches.
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
        panic!("the open tunnel was expected");
    };
    assert_eq!(t.title, "tandem", "without a title, the project's");
    assert_eq!((t.origin, t.target), (Machine::A, Machine::B));
    assert_eq!(next_tunnels(&mut ui).await, [(*t).clone()]);

    // B produces the output; A receives it.
    let ht = pc.tunnels().host.get(t.id).expect("B hosts the tunnel");
    let closed = Arc::new(AtomicBool::new(false));
    ht.add_resource(Box::new(Flag(closed.clone()))).unwrap();
    let w = ht.add_flow("cargo test · stdout").unwrap();
    let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    let half = data.len() / 2;
    w.write(&data[..half]);
    until(|| received(&mac, t.id) > 0).await;

    // The path is cut midway through the stream, B keeps writing and the link moves to the other.
    first.close("cable unplugged");
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

    // The UI cannot close it either (D9): the tunnel and its resource stay open.
    assert_eq!(
        within(ui.request(UiCommand::CloseTunnel { id: t.id }))
            .await
            .unwrap(),
        Reply::Rejected(CLIENTS_ONLY.into())
    );
    assert!(!closed.load(Ordering::SeqCst));
    assert!(ht.is_open());

    // Closing from the source releases the resources on the destination.
    assert_eq!(
        within(claude.request(UiCommand::CloseTunnel { id: t.id }))
            .await
            .unwrap(),
        Reply::Done
    );
    assert!(
        closed.load(Ordering::SeqCst),
        "B's resource is still open"
    );
    until(|| origin.tunnels()[0].state == TunnelState::Closed).await;
    assert_eq!(
        within(ui.request(UiCommand::GetTunnels)).await.unwrap(),
        Reply::Tunnels(vec![])
    );
    assert!(pc.tunnels().view().is_empty());
}
