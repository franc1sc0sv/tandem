//! Files and ports between two real agents over QUIC on loopback (T-106): a client
//! copies a folder to B and brings it back over A's IPC, with progress, and forwards a
//! TCP port in both directions.

use std::collections::BTreeSet;
use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tandem_core::Machine;
use tandem_link::identity::{Identity, PublicKey};
use tandem_link::transport::{Endpoint, PeerTrust};
use tandem_proto::{ClientKind, Direction, Peer, Reply, UiCommand};
use tandemd::ipc::{IpcClient, mem};
use tandemd::tunnels::{CLIENTS_ONLY, QuicDialer};
use tandemd::{Agent, AgentConfig, StateStore, daemon, peer};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

const TIMEOUT: Duration = Duration::from_secs(60);

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

async fn echo_server() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok(n) = s.read(&mut buf).await {
                    if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

async fn round_trip(port: u16, payload: Vec<u8>) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (mut rd, mut wr) = s.split();
    let send = async {
        wr.write_all(&payload).await.unwrap();
        wr.shutdown().await.unwrap();
    };
    let recv = async {
        let mut got = Vec::new();
        rd.read_to_end(&mut got).await.unwrap();
        got
    };
    let (_, got) = tokio::join!(send, recv);
    assert_eq!(got, payload);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn copies_folders_and_forwards_ports_over_quic() {
    let dir = tempfile::tempdir().unwrap();
    let mac = agent(dir.path(), Machine::A);
    let pc = agent(dir.path(), Machine::B);
    let (mac_id, pc_id) = (Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
    let trust = |id: &Identity| Arc::new(Keys(BTreeSet::from([id.public_key()])));
    let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    let mac_ep = Endpoint::bind(loopback, &mac_id, trust(&pc_id)).unwrap();
    let pc_ep = Endpoint::bind(loopback, &pc_id, trust(&mac_id)).unwrap();
    let (to_b, to_a) =
        within(async { tokio::join!(mac_ep.connect(pc_ep.local_addr().unwrap()), pc_ep.accept()) })
            .await;
    tokio::spawn(peer::accept_streams(to_a.unwrap().unwrap(), pc.clone()));
    mac.tunnels()
        .origin
        .set_link(Some(Arc::new(QuicDialer::new(to_b.unwrap(), Machine::A))));

    let mut stops = Vec::new();
    let mut claude = ipc(&mac, Peer::Client(ClientKind::Mcp), &mut stops).await;
    let mut ui = ipc(&mac, Peer::Ui, &mut stops).await;
    let Reply::Tunnel { tunnel: t, .. } = within(claude.request(UiCommand::OpenTunnel {
        title: "files".into(),
        project: None,
        target: Machine::B,
        session: "s".into(),
        workspace: None,
    }))
    .await
    .unwrap() else {
        panic!("the tunnel was expected");
    };

    // A folder with a file of several MiB (several chunks), an empty one and names with
    // spaces and accents.
    let src = dir.path().join("project");
    fs::create_dir_all(src.join("sub folder")).unwrap();
    let big: Vec<u8> = (0..2_500_000u32).map(|i| (i % 253) as u8).collect();
    fs::write(src.join("big.bin"), &big).unwrap();
    fs::write(src.join("sub folder/smörgås.txt"), "hello").unwrap();
    fs::write(src.join("empty"), "").unwrap();
    let dst = dir.path().join("on-the-pc");
    let back = dir.path().join("back");
    let mut progress = mac.files().subscribe_progress();

    let put = UiCommand::FilesPut {
        tunnel: t.id,
        local: src.display().to_string(),
        remote: dst.display().to_string(),
        include: vec![],
    };
    // The UI only watches.
    assert_eq!(
        within(ui.request(put.clone())).await.unwrap(),
        Reply::Rejected(CLIENTS_ONLY.into())
    );
    let Reply::Files(r) = within(claude.request(put)).await.unwrap() else {
        panic!("the report was expected");
    };
    assert_eq!((r.files, r.bytes, r.sent), (3, 2_500_004, 2_500_004));
    assert_eq!(fs::read(dst.join("big.bin")).unwrap(), big);
    assert_eq!(
        fs::read(dst.join("sub folder/smörgås.txt")).unwrap(),
        b"hello"
    );
    assert!(dst.join("empty").is_file());
    let last = within(async {
        let mut last = None;
        while let Ok(p) = progress.try_recv() {
            last = Some(p);
        }
        last
    })
    .await
    .expect("there was progress");
    assert!(last.upload && last.finished && last.done == last.total);

    let Reply::Files(r) = within(claude.request(UiCommand::FilesGet {
        tunnel: t.id,
        remote: dst.display().to_string(),
        local: back.display().to_string(),
    }))
    .await
    .unwrap() else {
        panic!("the report was expected");
    };
    assert_eq!((r.files, r.bytes), (3, 2_500_004));
    assert_eq!(fs::read(back.join("big.bin")).unwrap(), big);

    // Ports in both directions against an echo of A.
    let echo = echo_server().await;
    for (direction, local_port, remote_port) in [
        (Direction::LocalToRemote, 0, echo),
        (Direction::RemoteToLocal, echo, 0),
    ] {
        let Reply::Forward(f) = within(claude.request(UiCommand::PortForward {
            tunnel: t.id,
            local_port,
            remote_host: "127.0.0.1".into(),
            remote_port,
            direction,
        }))
        .await
        .unwrap() else {
            panic!("the port forward was expected");
        };
        let port = if direction == Direction::LocalToRemote {
            f.local_port
        } else {
            f.remote_port
        };
        let all = (0..10).map(|i| {
            tokio::spawn(round_trip(
                port,
                (0..30_000).map(|n| (n + i) as u8).collect(),
            ))
        });
        within(async {
            for h in all {
                h.await.unwrap();
            }
        })
        .await;
        assert_eq!(
            within(claude.request(UiCommand::StopForward { id: f.id }))
                .await
                .unwrap(),
            Reply::Done
        );
    }
}
