//! Procesos por túnel entre dos `tandemd` reales por loopback (T-104), con el doble de
//! `Spawner` en la destino: cinco procesos a la vez con la salida intercalada, el kill de
//! uno sin tocar a los demás y la lista de procesos corriendo en las dos UIs.

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

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .expect("la prueba superó el tiempo máximo")
}

/// La primera es la del lado A (DECISIONS A1).
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
    /// La UI de esta máquina (solo mira).
    ui: IpcClient<DuplexStream>,
    /// Un cliente (`tandem mcp`) conectado a la IPC de esta máquina.
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
            // Sin reserva: un puerto tomado falla enseguida y se eligen otros.
            fallback_ports: Vec::new(),
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
    // Otra IPC, esta para el cliente.
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

/// Los procesos corriendo que ve esa UI en sus túneles.
async fn running(ui: &mut IpcClient<DuplexStream>) -> Vec<TunnelProcess> {
    match request(ui, UiCommand::GetTunnels).await {
        Reply::Tunnels(groups) => groups
            .into_iter()
            .flat_map(|g| g.tunnels)
            .flat_map(|t| t.running)
            .collect(),
        other => panic!("se esperaban los túneles: {other:?}"),
    }
}

/// Toda la salida del proceso desde esa UI, hasta que termina.
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
                panic!("se esperaba la salida: {reply:?}");
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
async fn cinco_procesos_entre_dos_agentes_con_kill_de_uno() {
    let (a_id, b_id) = sides(Identity::from_seed([21; 32]), Identity::from_seed([22; 32]));
    let (mut a, mut b) = tandemd::start_pair!(
        |a_port, b_port| start(Machine::A, &a_id, a_port, (b_id.public_key(), b_port)),
        start(Machine::B, &b_id, b_port, (a_id.public_key(), a_port))
    );
    let fake = FakeSpawner::new();
    b.agent.tunnels().host.set_spawner(Arc::new(fake.clone()));

    // A abre el túnel en cuanto tiene enlace con B.
    let open = UiCommand::OpenTunnel {
        title: "cinco".into(),
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

    // Lanzar y matar es de los clientes; la UI solo mira (D9).
    let spec = |n: usize| ExecSpec {
        cmd: format!("trabajo-{n}"),
        args: vec!["--rápido".into()],
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
            panic!("se esperaba el proceso {n}");
        };
        assert_eq!(p.state, ProcessState::Running);
        ids.push(p.id);
    }
    let procs = fake.processes();
    assert_eq!(procs.len(), 5, "los cinco corren a la vez en B");
    assert_eq!(procs[3].launch.program, "trabajo-3");
    assert_eq!(procs[3].launch.cwd.as_deref(), Some("/src/tandem"));
    assert_eq!(procs[3].launch.env, [("N".to_owned(), "3".to_owned())]);

    // Las dos UIs ven los cinco corriendo.
    within(async {
        while running(&mut a.ui).await.len() < 5 || running(&mut b.ui).await.len() < 5 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    // Salida intercalada de los cinco.
    let line = |i: usize, round: usize| format!("p{i}.r{round}\n");
    for round in 0..3 {
        for (i, p) in procs.iter().enumerate() {
            p.stdout(line(i, round).as_bytes());
            p.stderr(format!("e{i}.{round}\n").as_bytes());
        }
        tokio::task::yield_now().await;
    }

    // Matar el segundo: solo cae él.
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
        assert_eq!(p.is_killed(), i == 1, "kill del proceso {i}");
    }
    within(async {
        while running(&mut a.ui).await.len() != 4 || running(&mut b.ui).await.len() != 4 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        !running(&mut a.ui).await.iter().any(|p| p.id == ids[1]),
        "el matado salió de la lista de la UI"
    );

    // Los demás siguen y terminan cada uno con su código.
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
        // Desde la UI de A (la que abrió) y, por la otra, desde la UI de B (la destino).
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
            assert_eq!(p.state, want, "estado del proceso {i}");
        }
    }
    within(async {
        while !running(&mut a.ui).await.is_empty() || !running(&mut b.ui).await.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    // Terminados: consultables, aunque ya no estén en la lista de la UI.
    let Reply::Processes(all) = request(&mut a.ui, UiCommand::GetProcesses { tunnel }).await else {
        panic!("se esperaba la lista");
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
