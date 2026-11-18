//! Punta a punta de T-108: un cliente MCP (JSON-RPC por stdio) y la CLI contra dos
//! `tandemd` reales por loopback (`start_pair!`), con el doble de `Spawner` en la destino.

use serde_json::{Value, json};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use tandem_cli::api::{Api, Context};
use tandem_cli::{cli, mcp, render};
use tandem_core::{Machine, Step};
use tandem_link::identity::{Identity, PublicKey, TrustStore, TrustedPeer};
use tandem_proto::{ClientKind, Peer};
use tandem_tunnel::process::fake::FakeSpawner;
use tandemd::daemon::{self, Backends, Daemon};
use tandemd::ipc::{IpcClient, mem};
use tandemd::network::NetOptions;
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::paths::Paths;
use tandemd::test_ports::{PortTaken, port_taken};
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
    /// Un cliente (`tandem mcp`) conectado a la IPC de esta máquina.
    client: Option<IpcClient<DuplexStream>>,
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
    let (listener, _connector) = mem::pair();
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
        client: Some(client),
        stops,
        task,
        _dir: dir,
    })
}

/// Un cliente MCP de prueba: habla JSON-RPC por líneas con `mcp::serve`.
struct McpClient {
    to: tokio::io::DuplexStream,
    from: tokio::io::Lines<tokio::io::BufReader<tokio::io::DuplexStream>>,
    next: u64,
}

impl McpClient {
    fn start(client: IpcClient<DuplexStream>, cwd: &std::path::Path) -> Self {
        let (to, server_in) = tokio::io::duplex(1 << 20);
        let (server_out, from) = tokio::io::duplex(1 << 20);
        let ctx = Context {
            cwd: Some(cwd.to_owned()),
            session: "claude-e2e".into(),
        };
        tokio::spawn(async move {
            let mut api = Api::new(client, ctx);
            let _ = mcp::serve(&mut api, tokio::io::BufReader::new(server_in), server_out).await;
        });
        McpClient {
            to,
            from: tokio::io::BufReader::new(from).lines(),
            next: 0,
        }
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params});
        self.to
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .unwrap();
        let line = within(self.from.next_line())
            .await
            .unwrap()
            .expect("respuesta");
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], self.next);
        v
    }

    /// Llama a una herramienta; el resultado (JSON de `content`), o el texto del error.
    async fn tool(&mut self, name: &str, args: Value) -> Result<Value, String> {
        Ok(self.call(name, args).await?.0)
    }

    /// Como `tool`, y además la respuesta cruda de `tools/call` (para ver su forma).
    async fn call(&mut self, name: &str, args: Value) -> Result<(Value, Value), String> {
        let v = self
            .rpc("tools/call", json!({"name": name, "arguments": args}))
            .await;
        let r = v["result"].clone();
        let text = r["content"][0]["text"].as_str().unwrap();
        if r["isError"] == true {
            return Err(text.to_owned());
        }
        Ok((serde_json::from_str(text).unwrap(), r))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn un_cliente_mcp_abre_un_tunel_ejecuta_lee_copia_y_cierra() {
    let (a_id, b_id) = sides(Identity::from_seed([31; 32]), Identity::from_seed([32; 32]));
    let (mut a, b) = tandemd::start_pair!(
        |a_port, b_port| start(Machine::A, &a_id, a_port, (b_id.public_key(), b_port)),
        start(Machine::B, &b_id, b_port, (a_id.public_key(), a_port))
    );
    let fake = FakeSpawner::new();
    b.agent.tunnels().host.set_spawner(Arc::new(fake.clone()));
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("mi-proyecto");
    std::fs::create_dir(&project).unwrap();
    let mut mcp = McpClient::start(a.client.take().unwrap(), &project);

    // Handshake y lista de herramientas.
    let init = mcp
        .rpc(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}}),
        )
        .await;
    assert_eq!(init["result"]["serverInfo"]["name"], "tandem");
    let list = mcp.rpc("tools/list", json!({})).await;
    let names: Vec<_> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    for want in [
        "machines",
        "resources",
        "tunnel_open",
        "exec",
        "output",
        "kill",
        "bring_back",
    ] {
        assert!(names.iter().any(|n| n == want), "falta {want}");
    }

    // Máquinas y recursos de la otra (cuando ya hay enlace).
    let machines = mcp.tool("machines", json!({})).await.unwrap();
    assert_eq!(machines["this"], "a");
    within(async {
        while mcp.tool("resources", json!({})).await.is_err() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;

    // Abrir el túnel en cuanto hay enlace; sin título sale el nombre del proyecto.
    let tunnel = within(async {
        loop {
            match mcp.tool("tunnel_open", json!({})).await {
                Ok(t) => return t,
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    })
    .await;
    assert_eq!(tunnel["title"], "mi-proyecto");
    assert_eq!(tunnel["target"], "b");
    assert_eq!(tunnel["opened_by"]["session"], "claude-e2e");
    let id = tunnel["id"].as_str().unwrap().to_owned();

    // Ejecutar y leer la salida en vivo.
    let p = mcp
        .tool(
            "exec",
            json!({"tunnel": id, "cmd": "cargo", "args": ["test"], "env": ["A=1"]}),
        )
        .await
        .unwrap();
    let pid = p["id"].as_u64().unwrap();
    let proc = fake.processes().remove(0);
    assert_eq!(proc.launch.program, "cargo");
    proc.stdout(b"running 3 tests\n");
    proc.stderr(b"aviso\n");
    let out = mcp
        .tool(
            "output",
            json!({"tunnel": id, "process": pid, "wait_ms": 10000}),
        )
        .await
        .unwrap();
    assert_eq!(out["stdout"], "running 3 tests\n");
    assert_eq!(out["stderr"], "aviso\n");
    assert_eq!(out["done"], false);
    proc.stdout(b"ok\n");
    proc.exit(0);
    let from = &out["next"];
    let out = mcp
        .tool(
            "output",
            json!({"tunnel": id, "process": pid, "wait_ms": 10000,
                   "from_stdout": from["stdout"], "from_stderr": from["stderr"]}),
        )
        .await
        .unwrap();
    assert_eq!(out["stdout"], "ok\n");
    within(async {
        loop {
            let o = mcp
                .tool(
                    "output",
                    json!({"tunnel": id, "process": pid, "from_stdout": 99}),
                )
                .await
                .unwrap();
            if o["done"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;

    // Un segundo proceso, y matarlo.
    let p2 = mcp
        .tool(
            "exec",
            json!({"tunnel": id, "cmd": "sleep", "args": ["100"]}),
        )
        .await
        .unwrap();
    mcp.tool("kill", json!({"tunnel": id, "process": p2["id"]}))
        .await
        .unwrap();
    assert!(fake.processes()[1].is_killed());

    // Archivos: copiar a la otra máquina y volver.
    let src = dir.path().join("hola.txt");
    std::fs::write(&src, b"hola tandem").unwrap();
    let dst = dir.path().join("copiado.txt");
    let r = mcp
        .tool(
            "files_put",
            json!({"tunnel": id, "local": src.display().to_string(), "remote": dst.display().to_string()}),
        )
        .await
        .unwrap();
    assert_eq!(r["files"], 1);
    assert_eq!(std::fs::read(&dst).unwrap(), b"hola tandem");
    let back = dir.path().join("de-vuelta.txt");
    mcp.tool(
        "files_get",
        json!({"tunnel": id, "remote": dst.display().to_string(), "local": back.display().to_string()}),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(&back).unwrap(), b"hola tandem");

    // Puerto: se abre y se cierra.
    let free = tandemd::test_ports::free_port().expect("puerto libre");
    let fwd = mcp
        .tool(
            "port_forward",
            json!({"tunnel": id, "local_port": free, "remote_port": 9}),
        )
        .await
        .unwrap();
    assert_eq!(fwd["tunnel"], id.as_str());
    mcp.tool("port_stop", json!({"id": fwd["id"]}))
        .await
        .unwrap();

    // Errores: lo ve el modelo como resultado, no como fallo del protocolo.
    let err = mcp
        .tool("kill", json!({"tunnel": "zz", "process": 1}))
        .await;
    assert!(err.unwrap_err().contains("id de túnel"));
    let bad = mcp
        .rpc("tools/call", json!({"name": "nada", "arguments": {}}))
        .await;
    assert_eq!(bad["error"]["code"], -32602);

    // La CLI usa la misma API: `tandem --json tunnels` y la salida legible.
    let parsed = cli::parse(["--json", "tunnels"].map(String::from)).unwrap();
    assert!(parsed.json);
    let listed = mcp.tool("tunnels", json!({})).await.unwrap();
    let group = &listed["projects"][0];
    assert_eq!(group["project"]["name"], "mi-proyecto");
    assert_eq!(group["tunnels"][0]["tunnel"]["id"], id.as_str());
    assert!(
        render::render("tunnels", &listed)
            .out
            .contains("mi-proyecto")
    );

    // Cerrar: ya no aparece.
    mcp.tool("tunnel_close", json!({"tunnel": id}))
        .await
        .unwrap();
    let listed = mcp.tool("tunnels", json!({})).await.unwrap();
    assert!(listed["projects"].as_array().unwrap().is_empty());
    assert!(proc.is_killed() || proc.has_exited());

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn una_salida_grande_se_lee_por_paginas_sin_perder_ni_repetir_bytes() {
    let (a_id, b_id) = sides(Identity::from_seed([33; 32]), Identity::from_seed([34; 32]));
    let (mut a, b) = tandemd::start_pair!(
        |a_port, b_port| start(Machine::A, &a_id, a_port, (b_id.public_key(), b_port)),
        start(Machine::B, &b_id, b_port, (a_id.public_key(), a_port))
    );
    let fake = FakeSpawner::new();
    b.agent.tunnels().host.set_spawner(Arc::new(fake.clone()));
    let dir = tempfile::tempdir().unwrap();
    let mut mcp = McpClient::start(a.client.take().unwrap(), dir.path());
    let tunnel = within(async {
        loop {
            match mcp.tool("tunnel_open", json!({"title": "grande"})).await {
                Ok(t) => return t,
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    })
    .await;
    let id = tunnel["id"].as_str().unwrap().to_owned();
    let p = mcp
        .tool(
            "exec",
            json!({"tunnel": id, "cmd": "cargo", "args": ["test"]}),
        )
        .await
        .unwrap();
    let pid = p["id"].as_u64().unwrap();
    let proc = fake.processes().remove(0);

    // 200 KiB de stdout con caracteres de 1 a 4 bytes (los cortes caen en medio de alguno) y
    // 100 KiB de stderr, más de lo que cabe en una página.
    let unit = "línea €uro 😀 ok\n";
    let out_text = unit.repeat(200 * 1024 / unit.len());
    let err_text = "x\u{e9}".repeat(50 * 1024 / 3);
    proc.stdout(out_text.as_bytes());
    proc.stderr(err_text.as_bytes());
    proc.exit(0);

    let (mut got_out, mut got_err) = (String::new(), String::new());
    let (mut from_o, mut from_e) = (json!(0), json!(0));
    let mut pages = 0;
    within(async {
        loop {
            let (o, raw) = mcp
                .call(
                    "output",
                    json!({"tunnel": id, "process": pid, "wait_ms": 10000,
                           "from_stdout": from_o, "from_stderr": from_e}),
                )
                .await
                .unwrap();
            pages += 1;
            let (so, se) = (o["stdout"].as_str().unwrap(), o["stderr"].as_str().unwrap());
            assert!(so.len() + se.len() <= 24 * 1024, "página sobre el tope");
            // Sin partir caracteres.
            assert!(!so.contains('\u{FFFD}') && !se.contains('\u{FFFD}'));
            // El cursor avanza por los bytes entregados.
            assert_eq!(
                o["next"]["stdout"],
                o["from"]["stdout"].as_u64().unwrap() + so.len() as u64
            );
            assert_eq!(
                o["next"]["stderr"],
                o["from"]["stderr"].as_u64().unwrap() + se.len() as u64
            );
            // Sin duplicar: la respuesta grande va solo en `content`, compacta.
            assert!(raw.get("structuredContent").is_none());
            assert!(!raw["content"][0]["text"].as_str().unwrap().contains("\n  "));
            got_out.push_str(so);
            got_err.push_str(se);
            from_o = o["next"]["stdout"].clone();
            from_e = o["next"]["stderr"].clone();
            if o["more"] == false {
                assert_eq!(o["done"], true);
                break;
            }
            assert_eq!(o["done"], false);
        }
    })
    .await;
    assert!(pages >= 10, "se esperaban varias páginas: {pages}");
    assert!(got_out == out_text, "stdout perdió o repitió bytes");
    assert!(got_err == err_text, "stderr perdió o repitió bytes");

    // `max_bytes` baja el tope y el tope duro lo limita.
    let (o, _) = mcp
        .call(
            "output",
            json!({"tunnel": id, "process": pid, "max_bytes": 100}),
        )
        .await
        .unwrap();
    assert!(o["stdout"].as_str().unwrap().len() + o["stderr"].as_str().unwrap().len() <= 100);
    assert_eq!(o["more"], true);
    let (o, _) = mcp
        .call(
            "output",
            json!({"tunnel": id, "process": pid, "max_bytes": 10_000_000}),
        )
        .await
        .unwrap();
    assert!(o["stdout"].as_str().unwrap().len() + o["stderr"].as_str().unwrap().len() <= 64 * 1024);

    // Una respuesta pequeña sí lleva `structuredContent` y el mismo JSON compacto.
    let (small, raw) = mcp.call("machines", json!({})).await.unwrap();
    assert_eq!(raw["structuredContent"], small);
    assert_eq!(raw["content"][0]["text"], small.to_string());

    a.stop().await;
    b.stop().await;
}

/// Corre git de verdad en `dir`.
fn git(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
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
}

/// T-130: la otra máquina nunca recibe credenciales. Workspace, archivos y entorno de
/// `exec`, con dos agentes reales y avisos en cada herramienta.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn la_otra_maquina_nunca_recibe_credenciales() {
    let (a_id, b_id) = sides(Identity::from_seed([35; 32]), Identity::from_seed([36; 32]));
    let (mut a, b) = tandemd::start_pair!(
        |a_port, b_port| start(Machine::A, &a_id, a_port, (b_id.public_key(), b_port)),
        start(Machine::B, &b_id, b_port, (a_id.public_key(), a_port))
    );
    let fake = FakeSpawner::new();
    b.agent.tunnels().host.set_spawner(Arc::new(fake.clone()));
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proyecto");
    std::fs::create_dir(&project).unwrap();
    git(&project, &["init", "-q", "-b", "main"]);
    git(&project, &["config", "user.email", "yo@example.com"]);
    git(&project, &["config", "user.name", "Yo"]);
    git(&project, &["config", "commit.gpgsign", "false"]);
    std::fs::write(project.join("lib.rs"), "fn a() {}\n").unwrap();
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-q", "-m", "inicial"]);
    // Nada de esto está en .gitignore.
    std::fs::write(project.join(".env"), "DB_PASSWORD=hunter2\n").unwrap();
    std::fs::create_dir(project.join(".ssh")).unwrap();
    std::fs::write(project.join(".ssh/id_ed25519"), "PRIVATE\n").unwrap();
    std::fs::write(project.join(".npmrc"), "//r/:_authToken=npm_x\n").unwrap();
    std::fs::write(project.join("nuevo.rs"), "fn n() {}\n").unwrap();
    let mut mcp = McpClient::start(a.client.take().unwrap(), &project);

    let tunnel = within(async {
        loop {
            match mcp.tool("tunnel_open", json!({"workspace": true})).await {
                Ok(t) => return t,
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    })
    .await;
    let id = tunnel["id"].as_str().unwrap().to_owned();
    let skipped: Vec<&str> = tunnel["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for s in [".env", ".ssh/id_ed25519", ".npmrc"] {
        assert!(skipped.contains(&s), "el aviso no dice {s}: {skipped:?}");
    }
    assert!(
        tunnel["warning"]
            .as_str()
            .unwrap()
            .contains("include_secrets")
    );
    let tid = tandem_proto::TunnelId(u64::from_str_radix(&id, 16).unwrap());
    let work = b.agent.workspaces().dir_of(tid).expect("workspace");
    assert!(work.join("nuevo.rs").is_file());
    for s in [".env", ".ssh", ".npmrc"] {
        assert!(!work.join(s).exists(), "llegó {s}");
    }

    // Un exec con GH_TOKEN se rechaza sin permiso explícito; SSH_AUTH_SOCK nunca.
    let e = mcp
        .tool(
            "exec",
            json!({"tunnel": id, "cmd": "git", "env": ["GH_TOKEN=ghp_x"]}),
        )
        .await
        .unwrap_err();
    assert!(
        e.contains("GH_TOKEN") && e.contains("allow_secret_env"),
        "{e}"
    );
    assert!(fake.processes().is_empty());
    let e = mcp
        .tool(
            "exec",
            json!({"tunnel": id, "cmd": "git", "env": ["SSH_AUTH_SOCK=/tmp/agent"],
                   "allow_secret_env": true}),
        )
        .await
        .unwrap_err();
    assert!(e.contains("SSH_AUTH_SOCK"), "{e}");
    assert!(fake.processes().is_empty());
    mcp.tool(
        "exec",
        json!({"tunnel": id, "cmd": "git", "env": ["GH_TOKEN=ghp_x"], "allow_secret_env": true}),
    )
    .await
    .unwrap();
    assert_eq!(fake.processes().len(), 1);

    // Archivos: la carpeta llega sin lo que parece credencial y el aviso lo dice.
    let src = dir.path().join("para-copiar");
    std::fs::create_dir_all(src.join("conf")).unwrap();
    std::fs::write(src.join("ok.txt"), "hola").unwrap();
    std::fs::write(src.join("conf/.env.local"), "X=1").unwrap();
    std::fs::write(src.join("llave.pem"), "pem").unwrap();
    let dst = dir.path().join("copiado");
    let r = mcp
        .tool(
            "files_put",
            json!({"tunnel": id, "local": src.display().to_string(),
                   "remote": dst.display().to_string()}),
        )
        .await
        .unwrap();
    assert_eq!(r["files"], 1);
    assert!(dst.join("ok.txt").is_file());
    assert!(!dst.join("llave.pem").exists());
    assert!(!dst.join("conf/.env.local").exists());
    assert_eq!(r["skipped"].as_array().unwrap().len(), 2, "{r}");
    assert!(r["warning"].as_str().unwrap().contains("llave.pem"));
    // Nombrándolo, sí viaja.
    let dst2 = dir.path().join("copiado2");
    let r = mcp
        .tool(
            "files_put",
            json!({"tunnel": id, "local": src.display().to_string(),
                   "remote": dst2.display().to_string(), "include_secrets": ["llave.pem"]}),
        )
        .await
        .unwrap();
    assert!(dst2.join("llave.pem").is_file());
    assert!(!dst2.join("conf/.env.local").exists());
    assert_eq!(r["skipped"].as_array().unwrap().len(), 1);

    // La CLI también avisa, por stderr.
    let rendered = render::render("files_put", &r);
    assert!(rendered.err.starts_with("[aviso]"), "{rendered:?}");

    a.stop().await;
    b.stop().await;
}
