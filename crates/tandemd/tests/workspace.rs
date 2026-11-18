//! Git workspaces between two real agents over QUIC on loopback (T-107): a client opens
//! a tunnel with A's repo, works on B (commits and changes) and brings it back as a
//! branch, all through the IPC; the repos are real git ones in temporary folders.

use std::collections::BTreeSet;
use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use std::path::Path;
use std::process::Command;
use tandem_core::Machine;
use tandem_link::identity::{Identity, PublicKey};
use tandem_link::transport::{Endpoint, PeerTrust};
use tandem_proto::{ClientKind, ExecSpec, Peer, Reply, UiCommand, WorkspaceSpec};
use tandem_tunnel::process::fake::FakeSpawner;
use tandemd::ipc::{IpcClient, mem};
use tandemd::tunnels::{CLIENTS_ONLY, QuicDialer};
use tandemd::{Agent, AgentConfig, StateStore, daemon, peer};
use tokio::io::DuplexStream;
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

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_WORK_TREE")
        .output()
        .expect("git on the PATH");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_repo_goes_to_the_other_machine_and_comes_back_as_a_branch_over_quic() {
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
    let fake = FakeSpawner::new();
    pc.tunnels().host.set_spawner(Arc::new(fake.clone()));

    let mut stops = Vec::new();
    let mut claude = ipc(&mac, Peer::Client(ClientKind::Mcp), &mut stops).await;
    let mut ui = ipc(&mac, Peer::Ui, &mut stops).await;

    // A's repo, with an uncommitted change and a new file.
    let repo = dir.path().join("project");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "yo@example.com"]);
    git(&repo, &["config", "user.name", "Yo"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "initial"]);
    fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
    fs::write(repo.join("new.txt"), "new\n").unwrap();
    let before = git(&repo, &["status", "--porcelain"]);

    let open = |workspace| UiCommand::OpenTunnel {
        title: "Fix".into(),
        project: None,
        target: Machine::B,
        session: "s".into(),
        workspace,
    };
    let ws = WorkspaceSpec {
        repo_path: repo.display().to_string(),
        keep: false,
        include: vec![],
    };
    // The UI only watches (D9).
    assert_eq!(
        within(ui.request(open(Some(ws.clone())))).await.unwrap(),
        Reply::Rejected(CLIENTS_ONLY.into())
    );
    // A repo that does not exist does not leave a tunnel open.
    let bad = WorkspaceSpec {
        repo_path: dir.path().join("missing").display().to_string(),
        keep: false,
        include: vec![],
    };
    let Reply::Rejected(why) = within(claude.request(open(Some(bad)))).await.unwrap() else {
        panic!("the rejection was expected");
    };
    assert!(why.contains("workspace"), "{why}");
    // The close reaches the source over the data channel, an instant later.
    within(async {
        while mac
            .tunnels()
            .origin
            .tunnels()
            .iter()
            .any(|t| t.state == tandem_proto::TunnelState::Open)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    let Reply::Tunnel { tunnel: t, .. } = within(claude.request(open(Some(ws)))).await.unwrap()
    else {
        panic!("the tunnel was expected");
    };
    let work = pc.workspaces().dir_of(t.id).expect("workspace en B");
    assert_eq!(
        git(&work, &["rev-parse", "HEAD"]),
        git(&repo, &["rev-parse", "HEAD"])
    );
    assert_eq!(
        fs::read_to_string(work.join("a.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "one\ntwo\n"
    );
    assert!(work.join("new.txt").is_file());

    // The tunnel's processes start in the workspace.
    let Reply::Process(_) = within(claude.request(UiCommand::Exec {
        tunnel: t.id,
        spec: ExecSpec {
            cmd: "cargo test".into(),
            ..ExecSpec::default()
        },
    }))
    .await
    .unwrap() else {
        panic!("the process was expected");
    };
    assert_eq!(
        fake.processes()[0].launch.cwd.as_deref(),
        Some(work.to_str().unwrap())
    );

    // Work on B and return as a branch.
    fs::write(work.join("b.txt"), "done on B\n").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "Fix on B"]);
    let back = UiCommand::BringBack {
        tunnel: t.id,
        commit: None,
        name: None,
    };
    assert_eq!(
        within(ui.request(back.clone())).await.unwrap(),
        Reply::Rejected(CLIENTS_ONLY.into())
    );
    let Reply::BroughtBack(r) = within(claude.request(back)).await.unwrap() else {
        panic!("the report was expected");
    };
    assert_eq!(r.branch.as_deref(), Some("tandem/fix"));
    assert_eq!(r.commits.len(), 1);
    assert_eq!(r.commits[0].summary, "Fix on B");
    assert_eq!(git(&repo, &["show", "tandem/fix:b.txt"]), "done on B");
    // The source: same branch and same state as before.
    assert_eq!(git(&repo, &["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(git(&repo, &["status", "--porcelain"]), before);

    // Closing the tunnel cleans B's workspace.
    assert_eq!(
        within(claude.request(UiCommand::CloseTunnel { id: t.id }))
            .await
            .unwrap(),
        Reply::Done
    );
    within(async {
        while work.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    for stop in stops {
        let _ = stop.send(());
    }
}
