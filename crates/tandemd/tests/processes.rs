//! Per-tunnel processes between two real `tandemd`s over loopback (T-104), with the
//! `Spawner` double on the destination: five processes at once with interleaved output, the kill of
//! one without touching the others and the list of running processes on both UIs.

mod common;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tandem_core::{Machine, Step};
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_proto::{
    ClientKind, ExecSpec, OutputCursor, Peer, ProcessId, ProcessState, Project, Reply,
    TunnelProcess, UiCommand,
};
use tandem_tunnel::process::fake::FakeSpawner;
use tandemd::daemon::{self, Backends, Daemon};
use tandemd::ipc::{IpcClient, mem};
use tandemd::network::NetOptions;
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::paths::Paths;
use tandemd::test_ports::{PortTaken, port_taken};
use tandemd::tunnels::CLIENTS_ONLY;
use tandemd::{Agent, AgentConfig, StoreError, autoconfig};
use tokio::io::DuplexStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const TIMEOUT: Duration = Duration::from_secs(30);

/// Says where it waited, to know which step hung (T-230).
#[track_caller]
fn within<F: Future>(f: F) -> impl Future<Output = F::Output> {
    let at = std::panic::Location::caller();
    async move {
        match tokio::time::timeout(TIMEOUT, f).await {
            Ok(v) => v,
            Err(_) => panic!("the test exceeded the maximum time waiting at {at}"),
        }
    }
}

/// The first is side A's (DECISIONS A1).
fn sides(x: Identity, y: Identity) -> (Identity, Identity) {
    if x.public_key().as_bytes() < y.public_key().as_bytes() {
        (x, y)
    } else {
        (y, x)
    }
}

struct Ok_;

impl LocalSteps for Ok_ {
    fn execute<'a>(&'a self, _step: &'a Step) -> StepFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

struct Running {
    agent: Arc<Agent>,
    /// This machine's UI (only watches).
    ui: IpcClient<DuplexStream>,
    /// A client (`tandem mcp`) connected to this machine's IPC.
    client: IpcClient<DuplexStream>,
    stops: Vec<oneshot::Sender<()>>,
    task: JoinHandle<Result<(), StoreError>>,
    _dir: tempfile::TempDir,
}

impl Running {
    async fn stop(mut self) {
        for stop in self.stops.drain(..) {
            let _ = stop.send(());
        }
        within(self.task).await.unwrap().unwrap();
    }
}

async fn start(
    machine: Machine,
    identity: &Identity,
    port: u16,
    peer: (PublicKey, u16),
) -> Result<Running, PortTaken> {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        config: dir.path().join("tandem.toml"),
        data_dir: dir.path().to_owned(),
    };
    identity.save(&paths.identity_file()).unwrap();
    let mut trust = TrustStore::open(paths.trust_file()).unwrap();
    trust
        .add_and_save(TrustedPeer {
            public_key: peer.0,
            name: machine.other().to_string(),
        })
        .unwrap();
    let text = autoconfig::generate(machine).replace(
        &format!("port = {}", tandemd::config::DEFAULT_PORT),
        &format!(
            "port = {port}\npeers = [{{ kind = \"ethernet\", addr = \"{}\" }}]",
            loopback(peer.1)
        ),
    );
    std::fs::write(&paths.config, &text).unwrap();
    let cfg = AgentConfig::load(&paths.config).unwrap();
    let started = daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: Arc::new(Ok_),
            input: None,
            env: None,
        },
        NetOptions {
            bind: Some(loopback(port)),
            mdns: false,
            // Without reservation: a taken port fails right away and others are chosen.
            fallback_ports: Vec::new(),
            health: common::health(),
            ..NetOptions::default()
        },
    );
    let Daemon {
        agent,
        network,
        input,
    } = port_taken(started).unwrap()?;
    let mut stops = Vec::new();
    let (listener, connector) = mem::pair();
    let (stop, stopped) = oneshot::channel::<()>();
    stops.push(stop);
    let task = tokio::spawn(
        Daemon {
            agent: agent.clone(),
            network,
            input,
        }
        .serve(listener, async {
            let _ = stopped.await;
        }),
    );
    let ui = IpcClient::handshake(connector.connect().unwrap())
        .await
        .unwrap();
    // Another IPC, this one for the client.
    let (listener, connector) = mem::pair();
    let (stop, stopped) = oneshot::channel::<()>();
    stops.push(stop);
    tokio::spawn(daemon::run(agent.clone(), listener, async {
        let _ = stopped.await;
    }));
    let client =
        IpcClient::handshake_as(connector.connect().unwrap(), Peer::Client(ClientKind::Mcp))
            .await
            .unwrap();
    Ok(Running {
        agent,
        ui,
        client,
        stops,
        task,
        _dir: dir,
    })
}

async fn request(c: &mut IpcClient<DuplexStream>, cmd: UiCommand) -> Reply {
    within(c.request(cmd)).await.unwrap()
}

/// The running processes that UI sees in its tunnels.
async fn running(ui: &mut IpcClient<DuplexStream>) -> Vec<TunnelProcess> {
    match request(ui, UiCommand::GetTunnels).await {
        Reply::Tunnels(groups) => groups
            .into_iter()
            .flat_map(|g| g.tunnels)
            .flat_map(|t| t.running)
            .collect(),
        other => panic!("the tunnels were expected: {other:?}"),
    }
}

/// All of the process's output from that UI, until it finishes.
async fn read_all(
    ui: &mut IpcClient<DuplexStream>,
    tunnel: tandem_proto::TunnelId,
    process: ProcessId,
) -> (String, String, TunnelProcess) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut cursor = OutputCursor::default();
    within(async {
        loop {
            let reply = ui
                .request(UiCommand::Output {
                    tunnel,
                    process,
                    from: cursor,
                    max: 16,
                })
                .await
                .unwrap();
            let Reply::Output(o) = reply else {
                panic!("the output was expected: {reply:?}");
            };
            assert_eq!(o.from, cursor);
            out.extend_from_slice(&o.stdout);
            err.extend_from_slice(&o.stderr);
            cursor = o.next;
            if o.done {
                return (
                    String::from_utf8(out).unwrap(),
                    String::from_utf8(err).unwrap(),
                    o.process,
                );
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn five_processes_between_two_agents_with_the_kill_of_one() {
    let (a_id, b_id) = sides(Identity::from_seed([21; 32]), Identity::from_seed([22; 32]));
    let (mut a, mut b) = tandemd::start_pair!(
        |a_port, b_port| start(Machine::A, &a_id, a_port, (b_id.public_key(), b_port)),
        start(Machine::B, &b_id, b_port, (a_id.public_key(), a_port))
    );
    let fake = FakeSpawner::new();
    b.agent.tunnels().host.set_spawner(Arc::new(fake.clone()));

    // A opens the tunnel as soon as it has a link to B.
    let open = UiCommand::OpenTunnel {
        title: "five".into(),
        project: Some(Project {
            name: "tandem".into(),
            root: "/src/tandem".into(),
        }),
        target: Machine::B,
        session: "claude-1".into(),
        workspace: None,
    };
    let tunnel = within(async {
        loop {
            match a.client.request(open.clone()).await.unwrap() {
                Reply::Tunnel { tunnel: t, .. } => return t.id,
                _ => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    })
    .await;

    // Launching and killing belong to clients; the UI only watches (D9).
    let spec = |n: usize| ExecSpec {
        cmd: format!("work-{n}"),
        args: vec!["--fast".into()],
        shell: None,
        cwd: Some("/src/tandem".into()),
        env: vec![("N".into(), n.to_string())],
        allow_secret_env: false,
    };
    assert_eq!(
        request(
            &mut a.ui,
            UiCommand::Exec {
                tunnel,
                spec: spec(0)
            }
        )
        .await,
        Reply::Rejected(CLIENTS_ONLY.into())
    );
    assert_eq!(
        request(
            &mut a.ui,
            UiCommand::Kill {
                tunnel,
                process: ProcessId(1)
            }
        )
        .await,
        Reply::Rejected(CLIENTS_ONLY.into())
    );
    assert!(fake.processes().is_empty());

    let mut ids = Vec::new();
    for n in 0..5 {
        let Reply::Process(p) = request(
            &mut a.client,
            UiCommand::Exec {
                tunnel,
                spec: spec(n),
            },
        )
        .await
        else {
            panic!("process {n} was expected");
        };
        assert_eq!(p.state, ProcessState::Running);
        ids.push(p.id);
    }
    let procs = fake.processes();
    assert_eq!(procs.len(), 5, "all five run at once on B");
    assert_eq!(procs[3].launch.program, "work-3");
    assert_eq!(procs[3].launch.cwd.as_deref(), Some("/src/tandem"));
    assert_eq!(procs[3].launch.env, [("N".to_owned(), "3".to_owned())]);

    // Both UIs see the five running.
    within(async {
        while running(&mut a.ui).await.len() < 5 || running(&mut b.ui).await.len() < 5 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    // Interleaved output of the five.
    let line = |i: usize, round: usize| format!("p{i}.r{round}\n");
    for round in 0..3 {
        for (i, p) in procs.iter().enumerate() {
            p.stdout(line(i, round).as_bytes());
            p.stderr(format!("e{i}.{round}\n").as_bytes());
        }
        tokio::task::yield_now().await;
    }

    // Kill the second: only it falls.
    assert_eq!(
        request(
            &mut a.client,
            UiCommand::Kill {
                tunnel,
                process: ids[1]
            }
        )
        .await,
        Reply::Done
    );
    for (i, p) in procs.iter().enumerate() {
        assert_eq!(p.is_killed(), i == 1, "kill of process {i}");
    }
    within(async {
        while running(&mut a.ui).await.len() != 4 || running(&mut b.ui).await.len() != 4 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        !running(&mut a.ui).await.iter().any(|p| p.id == ids[1]),
        "the killed one left the UI's list"
    );

    // The others continue and each ends with its code.
    for round in 3..5 {
        for (i, p) in procs.iter().enumerate() {
            if i != 1 {
                p.stdout(line(i, round).as_bytes());
            }
        }
    }
    for (i, p) in procs.iter().enumerate() {
        if i != 1 {
            p.exit(i32::try_from(i).unwrap());
        }
    }

    for (i, id) in ids.iter().enumerate() {
        // From A's UI (the one that opened) and, through the other, from B's UI (the destination).
        for ui in [&mut a.ui, &mut b.ui] {
            let (out, err, p) = read_all(ui, tunnel, *id).await;
            let rounds = if i == 1 { 3 } else { 5 };
            assert_eq!(out, (0..rounds).map(|r| line(i, r)).collect::<String>());
            assert_eq!(
                err,
                (0..3).map(|r| format!("e{i}.{r}\n")).collect::<String>()
            );
            let want = if i == 1 {
                ProcessState::Killed
            } else {
                ProcessState::Exited {
                    code: Some(i32::try_from(i).unwrap()),
                }
            };
            assert_eq!(p.state, want, "state of process {i}");
        }
    }
    within(async {
        while !running(&mut a.ui).await.is_empty() || !running(&mut b.ui).await.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    // Finished: queryable, even if no longer in the UI's list.
    let Reply::Processes(all) = request(&mut a.ui, UiCommand::GetProcesses { tunnel }).await else {
        panic!("the list was expected");
    };
    assert_eq!(all.len(), 5);
    assert!(all.iter().all(|p| !p.is_running()));
    assert!(matches!(
        request(
            &mut a.ui,
            UiCommand::GetProcesses {
                tunnel: tandem_proto::TunnelId(1)
            }
        )
        .await,
        Reply::Rejected(_)
    ));

    a.stop().await;
    b.stop().await;
}
