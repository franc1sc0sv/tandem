//! Workspaces de git entre dos agentes reales por QUIC en loopback (T-107): un cliente abre
//! un túnel con el repo de A, trabaja en B (commits y cambios) y lo trae de vuelta como
//! rama, todo por la IPC; los repos son de git de verdad en carpetas temporales.

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
        .expect("git en el PATH");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn el_repo_va_a_la_otra_maquina_y_vuelve_como_rama_por_quic() {
    let dir = tempfile::tempdir().unwrap();
    round_trip(dir.path(), dir.path()).await;
}

/// T-197: with a relative `--data-dir`, git runs in another cwd and could not find the bundles.
/// The temporary directory lives under the test's cwd so it can be named relatively.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_works_with_a_relative_data_directory() {
    let dir = tempfile::tempdir_in(".").unwrap();
    let name = dir.path().file_name().unwrap();
    let relative = Path::new(".").join(name).join("data");
    assert!(relative.is_relative());
    fs::create_dir_all(&relative).unwrap();
    round_trip(&dir.path().canonicalize().unwrap(), &relative).await;
}

async fn round_trip(root: &Path, data: &Path) {
    let mac = agent(data, Machine::A);
    let pc = agent(data, Machine::B);
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

    // El repo de A, con un cambio sin commit y un archivo nuevo.
    let repo = root.join("proyecto");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "yo@example.com"]);
    git(&repo, &["config", "user.name", "Yo"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    fs::write(repo.join("a.txt"), "uno\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "inicial"]);
    fs::write(repo.join("a.txt"), "uno\ndos\n").unwrap();
    fs::write(repo.join("nuevo.txt"), "nuevo\n").unwrap();
    let before = git(&repo, &["status", "--porcelain"]);

    let open = |workspace| UiCommand::OpenTunnel {
        title: "Arreglo".into(),
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
    // La UI solo mira (D9).
    assert_eq!(
        within(ui.request(open(Some(ws.clone())))).await.unwrap(),
        Reply::Rejected(CLIENTS_ONLY.into())
    );
    // Un repo que no existe no deja un túnel abierto.
    let bad = WorkspaceSpec {
        repo_path: root.join("no-existe").display().to_string(),
        keep: false,
        include: vec![],
    };
    let Reply::Rejected(why) = within(claude.request(open(Some(bad)))).await.unwrap() else {
        panic!("se esperaba el rechazo");
    };
    assert!(why.contains("workspace"), "{why}");
    // El cierre llega al origen por el canal de datos, un instante después.
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
        panic!("se esperaba el túnel");
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
        "uno\ndos\n"
    );
    assert!(work.join("nuevo.txt").is_file());

    // Los procesos del túnel arrancan en el workspace.
    let Reply::Process(_) = within(claude.request(UiCommand::Exec {
        tunnel: t.id,
        spec: ExecSpec {
            cmd: "cargo test".into(),
            ..ExecSpec::default()
        },
    }))
    .await
    .unwrap() else {
        panic!("se esperaba el proceso");
    };
    assert_eq!(
        fake.processes()[0].launch.cwd.as_deref(),
        Some(work.to_str().unwrap())
    );

    // Trabajo en B y vuelta como rama.
    fs::write(work.join("b.txt"), "hecho en B\n").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "Arregla en B"]);
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
        panic!("se esperaba el informe");
    };
    assert_eq!(r.branch.as_deref(), Some("tandem/arreglo"));
    assert_eq!(r.commits.len(), 1);
    assert_eq!(r.commits[0].summary, "Arregla en B");
    assert_eq!(git(&repo, &["show", "tandem/arreglo:b.txt"]), "hecho en B");
    // El origen: misma rama y mismo estado de antes.
    assert_eq!(git(&repo, &["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(git(&repo, &["status", "--porcelain"]), before);

    // Cerrar el túnel limpia el workspace de B.
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
