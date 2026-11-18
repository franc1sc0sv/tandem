//! Comprobaciones contra un agente de prueba que ya corre, por su IPC. Cada una devuelve un
//! [`CheckResult`]; ninguna deja un túnel abierto ni un reenvío vivo al terminar.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tandem_core::Machine;
use tandem_proto::{
    AgentToUi, Direction, ExecSpec, FilesReport, OutputCursor, Peer, PeerStatus, ProcessState,
    Reply, Tunnel, TunnelId, UiCommand, WorkspaceSpec,
};

use crate::conn::{self, Conn};
use crate::latency::{self, Judgement};
use crate::report::CheckResult;

const IPC_WAIT: Duration = Duration::from_secs(30);
const STEP: Duration = Duration::from_secs(20);

/// Lo que cada comprobación necesita saber de su máquina y de la otra.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub data_dir: PathBuf,
    /// `pc` o `mac`.
    pub label: String,
    /// Ruta de `tandem-hw` en la otra máquina (los túneles lo usan como programa de
    /// prueba: sale igual en Windows y en macOS).
    pub peer_exe: String,
    /// Carpeta de trabajo de la otra máquina (temporal, ya creada) y su separador.
    pub remote_dir: String,
    pub remote_sep: char,
}

impl Ctx {
    fn remote(&self, name: &str) -> String {
        format!("{}{}{name}", self.remote_dir, self.remote_sep)
    }

    fn result(&self, check: &str, covers: &[&str]) -> CheckResult {
        CheckResult::new(check, &self.label, covers)
    }
}

/// Todas las comprobaciones que corre `run all` en una máquina, en orden.
pub const TUNNEL_CHECKS: &[&str] = &[
    "tunnel-process",
    "tunnel-files",
    "tunnel-port",
    "tunnel-workspace",
];

async fn req(c: &mut Conn, cmd: UiCommand) -> anyhow::Result<Reply> {
    match tokio::time::timeout(STEP, c.request(cmd)).await {
        Ok(Ok(Reply::Rejected(why))) => anyhow::bail!("el agente rechazó el pedido: {why}"),
        Ok(Ok(r)) => Ok(r),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => anyhow::bail!("el agente no contestó en {STEP:?}"),
    }
}

fn unexpected(r: Reply) -> anyhow::Error {
    anyhow::anyhow!("respuesta inesperada: {r:?}")
}

fn timed(mut r: CheckResult, started: Instant) -> CheckResult {
    r.millis = started.elapsed().as_millis() as u64;
    r
}

fn done(r: CheckResult, started: Instant, out: anyhow::Result<String>) -> CheckResult {
    timed(
        match out {
            Ok(d) => r.pass(d),
            Err(e) => r.fail(format!("{e:#}")),
        },
        started,
    )
}

// --- Enlace -----------------------------------------------------------------------------

/// H1: el agente de prueba ve a la otra máquina emparejada, con un enlace.
pub async fn pair(ctx: &Ctx, wait: Duration) -> CheckResult {
    let started = Instant::now();
    let r = ctx.result("pair", &["H1"]);
    let out = async {
        let mut ui = conn::connect(&ctx.data_dir, Peer::Ui, IPC_WAIT).await?;
        loop {
            match req(&mut ui, UiCommand::GetPeer).await? {
                Reply::Peer(PeerStatus::Paired {
                    link: Some(link), ..
                }) => return Ok(format!("emparejada por {link:?}")),
                Reply::Peer(_) => {}
                other => return Err(unexpected(other)),
            }
            if started.elapsed() >= wait {
                anyhow::bail!("no vio a la otra máquina en {wait:?}");
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    .await;
    done(r, started, out)
}

/// T-093 (control): latencia del ping por el enlace. No hay tope en la tarea; falla solo si
/// algún ping se pierde o el p99 pasa de 200 ms (un enlace roto, no uno lento).
pub async fn ping(ctx: &Ctx, count: usize) -> CheckResult {
    let started = Instant::now();
    let r = ctx.result("ping", &["T-093"]);
    let out = async {
        let mut ui = conn::connect(&ctx.data_dir, Peer::Ui, IPC_WAIT).await?;
        let mut millis = Vec::new();
        let mut link = None;
        for _ in 0..count {
            match req(&mut ui, UiCommand::PingPeer).await? {
                Reply::Pong { link: l, millis: m } => {
                    link = Some(l);
                    millis.push(m);
                }
                other => return Err(unexpected(other)),
            }
        }
        let (p50, p99) =
            latency::percentiles(&mut millis).ok_or_else(|| anyhow::anyhow!("sin pings"))?;
        let said = format!("{link:?}: p50 {p50} ms, p99 {p99} ms en {count} pings");
        anyhow::ensure!(p99 <= 200, "p99 demasiado alto: {said}");
        Ok(said)
    }
    .await;
    done(r, started, out)
}

// --- Entrada sintética ---------------------------------------------------------------------

/// Mueve el cursor del sistema con la entrada de verdad, en pasos chicos. Solo existe en
/// Windows y macOS; en otra plataforma la comprobación se omite.
fn spawn_injector(
    dx: f64,
    dy: f64,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<Result<(), String>> {
    std::thread::spawn(move || {
        let mut sink = Injector::new()?;
        while !stop.load(Ordering::Relaxed) {
            sink.motion(dx, dy)?;
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    })
}

#[cfg(windows)]
struct Injector(
    tandem_input::windows::emulate::Emulator<tandem_input::windows::emulate::SendInputSink>,
);

#[cfg(windows)]
impl Injector {
    fn new() -> Result<Self, String> {
        use tandem_input::windows::emulate::{Emulator, SendInputSink};
        let sink = SendInputSink::new().map_err(|e| e.to_string())?;
        Emulator::new(sink).map(Injector).map_err(|e| e.to_string())
    }

    fn motion(&mut self, dx: f64, dy: f64) -> Result<(), String> {
        self.0
            .emulate(&tandem_input::InputEvent::MotionRelative { dx, dy })
            .map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "macos")]
struct Injector(tandem_input::macos::emulate::MacEmulator<tandem_input::macos::emulate::CgSink>);

#[cfg(target_os = "macos")]
impl Injector {
    fn new() -> Result<Self, String> {
        use tandem_input::macos::emulate::{CgSink, MacEmulator};
        let sink = CgSink::new().map_err(|e| e.to_string())?;
        MacEmulator::new(sink)
            .map(Injector)
            .map_err(|e| e.to_string())
    }

    fn motion(&mut self, dx: f64, dy: f64) -> Result<(), String> {
        self.0
            .emit(&tandem_input::InputEvent::MotionRelative { dx, dy })
            .map_err(|e| e.to_string())
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
struct Injector;

#[cfg(not(any(windows, target_os = "macos")))]
impl Injector {
    fn new() -> Result<Self, String> {
        Err("sin inyección de entrada en este sistema".into())
    }

    fn motion(&mut self, _: f64, _: f64) -> Result<(), String> {
        Ok(())
    }
}

/// Los cuatro bordes y el vector con que se empuja contra cada uno.
const PUSHES: [(&str, f64, f64); 4] = [
    ("derecho", 6.0, 0.0),
    ("izquierdo", -6.0, 0.0),
    ("inferior", 0.0, 6.0),
    ("superior", 0.0, -6.0),
];

/// H4: esta máquina tiene el teclado y el mouse; se inyecta movimiento sintético contra
/// cada borde hasta que el agente avisa `Crossed` hacia la otra. No mueve el mouse físico.
/// Sigue inyectando `keep_injecting` después de cruzar para que la otra máquina mida su
/// latencia (corre `latency-watch` allá al mismo tiempo).
pub async fn input_cross(ctx: &Ctx, keep_injecting: Duration) -> CheckResult {
    let started = Instant::now();
    let r = ctx.result("input-cross", &["H4", "T-093"]);
    let out = async {
        let mut ui = conn::connect(&ctx.data_dir, Peer::Ui, IPC_WAIT).await?;
        let me = ui.agent();
        let status = match req(&mut ui, UiCommand::GetInput).await? {
            Reply::Input(s) => s,
            other => return Err(unexpected(other)),
        };
        if let Some(problem) = &status.problem {
            anyhow::bail!("el motor de entrada no funciona aquí: {problem}");
        }
        if !status.session {
            anyhow::bail!("no hay sesión de entrada con la otra máquina");
        }
        if status.server != me {
            req(&mut ui, UiCommand::SetKeyboard(me)).await?;
        }
        // El aviso de cruce llega por otra conexión, que se cierra al terminar.
        let mut watch = conn::connect(&ctx.data_dir, Peer::Ui, IPC_WAIT).await?;
        req(&mut watch, UiCommand::WatchCursor { on: true }).await?;
        for (edge, dx, dy) in PUSHES {
            let stop = Arc::new(AtomicBool::new(false));
            let worker = spawn_injector(dx, dy, stop.clone());
            let crossed = wait_crossed(&mut watch, me, Duration::from_secs(6)).await;
            if crossed {
                tokio::time::sleep(keep_injecting).await;
            }
            stop.store(true, Ordering::Relaxed);
            let injected = worker
                .join()
                .map_err(|_| anyhow::anyhow!("el hilo de inyección murió"))?;
            injected.map_err(anyhow::Error::msg)?;
            if crossed {
                // Se devuelve el teclado y el mouse a esta máquina para la dirección siguiente.
                let _ = req(&mut ui, UiCommand::SetKeyboard(me)).await;
                return Ok(format!("cruzó por el borde {edge} con entrada sintética"));
            }
        }
        anyhow::bail!("ningún borde cruzó a la otra máquina con movimiento sintético")
    }
    .await;
    match out {
        Err(e) if e.to_string().contains("sin inyección") => {
            timed(r.skip(format!("{e:#}")), started)
        }
        out => done(r, started, out),
    }
}

async fn wait_crossed(watch: &mut Conn, from: Machine, wait: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match tokio::time::timeout_at(deadline, watch.next_event()).await {
            Ok(Ok(Some(AgentToUi::Crossed { from: f, .. }))) if f == from => return true,
            Ok(Ok(Some(_))) => {}
            _ => return false,
        }
    }
}

/// T-093: la máquina que recibe la entrada mide su latencia; se queda mirando `window`
/// y juzga la mejor ventana de 5 s que vio (la de más eventos).
pub async fn latency_watch(ctx: &Ctx, window: Duration) -> CheckResult {
    let started = Instant::now();
    let r = ctx.result("input-latency", &["T-093", "H4"]);
    let out = async {
        let mut ui = conn::connect(&ctx.data_dir, Peer::Ui, IPC_WAIT).await?;
        let mut best: Option<tandem_proto::InputLatency> = None;
        while started.elapsed() < window {
            if let Reply::Input(s) = req(&mut ui, UiCommand::GetInput).await?
                && let Some(l) = s.latency
                && best.is_none_or(|b| l.count > b.count)
            {
                best = Some(l);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        match best.map(|l| latency::judge(&l)) {
            Some(Judgement::Pass(d)) => Ok(d),
            Some(Judgement::Fail(d)) => anyhow::bail!("{d}"),
            Some(Judgement::TooFew(n)) => {
                anyhow::bail!("solo {n} eventos medidos: la entrada no llegó (¿cruzó?)")
            }
            None => anyhow::bail!("el agente no midió ninguna latencia en {window:?}"),
        }
    }
    .await;
    done(r, started, out)
}

// --- Túneles ---------------------------------------------------------------------------------

async fn open_tunnel(
    c: &mut Conn,
    title: &str,
    workspace: Option<WorkspaceSpec>,
) -> anyhow::Result<Tunnel> {
    let target = c.agent().other();
    match req(
        c,
        UiCommand::OpenTunnel {
            title: title.into(),
            project: None,
            target,
            session: "hw-suite".into(),
            workspace,
        },
    )
    .await?
    {
        Reply::Tunnel(t) => Ok(*t),
        other => Err(unexpected(other)),
    }
}

async fn exec(
    c: &mut Conn,
    tunnel: TunnelId,
    cmd: &str,
    args: &[&str],
) -> anyhow::Result<tandem_proto::TunnelProcess> {
    let spec = ExecSpec {
        cmd: cmd.into(),
        args: args.iter().map(|a| (*a).into()).collect(),
        shell: None,
        cwd: None,
        env: Vec::new(),
    };
    match req(c, UiCommand::Exec { tunnel, spec }).await? {
        Reply::Process(p) => Ok(*p),
        other => Err(unexpected(other)),
    }
}

/// Lee la salida de un proceso hasta que termina y devuelve stdout.
async fn stdout_until_done(
    c: &mut Conn,
    t: TunnelId,
    p: &tandem_proto::TunnelProcess,
) -> anyhow::Result<String> {
    let mut from = OutputCursor {
        stdout: 0,
        stderr: 0,
    };
    let mut out = Vec::new();
    let deadline = Instant::now() + STEP;
    loop {
        match req(
            c,
            UiCommand::Output {
                tunnel: t,
                process: p.id,
                from,
                max: 65536,
            },
        )
        .await?
        {
            Reply::Output(o) => {
                out.extend_from_slice(&o.stdout);
                from = o.next;
                if o.done {
                    return Ok(String::from_utf8_lossy(&out).into_owned());
                }
            }
            other => return Err(unexpected(other)),
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "el proceso no terminó en {STEP:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Abre un túnel, corre `body` y lo cierra siempre, aunque `body` falle.
async fn with_tunnel<F>(
    ctx: &Ctx,
    title: &str,
    workspace: Option<WorkspaceSpec>,
    body: F,
) -> anyhow::Result<String>
where
    F: for<'a> FnOnce(
        &'a mut Conn,
        TunnelId,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<String>> + 'a>,
    >,
{
    let mut c = conn::connect(&ctx.data_dir, conn::client(), IPC_WAIT).await?;
    let tunnel = open_tunnel(&mut c, title, workspace).await?;
    let id = tunnel.id;
    let result = body(&mut c, id).await;
    let closed = req(&mut c, UiCommand::CloseTunnel { id }).await;
    let said = result?;
    closed?;
    Ok(said)
}

/// T-104 / H2: un proceso en la otra máquina por el túnel, su salida, y matarlo con su árbol.
pub async fn tunnel_process(ctx: &Ctx) -> CheckResult {
    let started = Instant::now();
    let r = ctx.result("tunnel-process", &["H2", "T-103", "T-104"]);
    let exe = ctx.peer_exe.clone();
    let out = with_tunnel(ctx, "hw-process", None, |c, t| {
        Box::pin(async move {
            let p = exec(c, t, &exe, &["emit", "hw-hello"]).await?;
            let text = stdout_until_done(c, t, &p).await?;
            anyhow::ensure!(text.contains("hw-hello"), "la salida no llegó: {text:?}");
            let long = exec(c, t, &exe, &["sleep", "300"]).await?;
            req(
                c,
                UiCommand::Kill {
                    tunnel: t,
                    process: long.id,
                },
            )
            .await?;
            let state = match req(c, UiCommand::GetProcesses { tunnel: t }).await? {
                Reply::Processes(ps) => ps.into_iter().find(|p| p.id == long.id).map(|p| p.state),
                other => return Err(unexpected(other)),
            };
            anyhow::ensure!(
                state == Some(ProcessState::Killed),
                "el proceso quedó en {state:?}"
            );
            Ok("salida recibida de la otra máquina y proceso matado".into())
        })
    })
    .await;
    done(r, started, out)
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i.wrapping_mul(31) ^ (i >> 8)) as u8)
        .collect()
}

/// T-106: un archivo de 3 MiB va y vuelve por el túnel idéntico.
pub async fn tunnel_files(ctx: &Ctx) -> CheckResult {
    let started = Instant::now();
    let r = ctx.result("tunnel-files", &["H2", "T-106"]);
    let remote = ctx.remote("hw.bin");
    let dir = scratch(&ctx.data_dir, "files");
    let out = async {
        let dir = dir?;
        let sent = dir.join("ida.bin");
        let back = dir.join("vuelta.bin");
        let data = pattern(3 << 20);
        std::fs::write(&sent, &data)?;
        let (s, b) = (
            sent.to_string_lossy().into_owned(),
            back.to_string_lossy().into_owned(),
        );
        let said = with_tunnel(ctx, "hw-files", None, |c, t| {
            Box::pin(async move {
                let put = req(
                    c,
                    UiCommand::FilesPut {
                        tunnel: t,
                        local: s,
                        remote: remote.clone(),
                    },
                )
                .await?;
                let Reply::Files(FilesReport { bytes, .. }) = put else {
                    return Err(unexpected(put));
                };
                req(
                    c,
                    UiCommand::FilesGet {
                        tunnel: t,
                        remote,
                        local: b,
                    },
                )
                .await?;
                Ok(format!("{bytes} bytes de ida y vuelta"))
            })
        })
        .await?;
        anyhow::ensure!(std::fs::read(&back)? == data, "el archivo volvió distinto");
        Ok(said)
    }
    .await;
    done(r, started, out)
}

/// T-106: un puerto reenviado por el túnel llega a un servidor de eco de la otra máquina.
pub async fn tunnel_port(ctx: &Ctx) -> CheckResult {
    let started = Instant::now();
    let r = ctx.result("tunnel-port", &["H2", "T-106"]);
    let exe = ctx.peer_exe.clone();
    let port = 47_900 + (std::process::id() % 90) as u16;
    let out = with_tunnel(ctx, "hw-port", None, |c, t| {
        Box::pin(async move {
            let server = exec(c, t, &exe, &["echo-server", &port.to_string()]).await?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            let fwd = match req(
                c,
                UiCommand::PortForward {
                    tunnel: t,
                    local_port: 0,
                    remote_host: "127.0.0.1".into(),
                    remote_port: port,
                    direction: Direction::LocalToRemote,
                },
            )
            .await?
            {
                Reply::Forward(f) => f,
                other => return Err(unexpected(other)),
            };
            let echoed = echo(fwd.local_port).await;
            let _ = req(c, UiCommand::StopForward { id: fwd.id }).await;
            let _ = req(
                c,
                UiCommand::Kill {
                    tunnel: t,
                    process: server.id,
                },
            )
            .await;
            echoed?;
            Ok(format!(
                "eco por el puerto reenviado ({port} en la otra máquina)"
            ))
        })
    })
    .await;
    done(r, started, out)
}

async fn echo(port: u16) -> anyhow::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s =
        tokio::time::timeout(STEP, tokio::net::TcpStream::connect(("127.0.0.1", port))).await??;
    s.write_all(b"tandem-hw-ping").await?;
    let mut buf = [0u8; 14];
    tokio::time::timeout(STEP, s.read_exact(&mut buf)).await??;
    anyhow::ensure!(&buf == b"tandem-hw-ping", "el eco volvió distinto: {buf:?}");
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> anyhow::Result<()> {
    let st = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=tandem-hw", "-c", "user.email=hw@invalid"])
        .args(args)
        .stdout(std::process::Stdio::null())
        .status()?;
    anyhow::ensure!(st.success(), "git {args:?} falló");
    Ok(())
}

/// T-107: un repo de git viaja con el túnel, se hace un commit allá y vuelve como rama.
pub async fn tunnel_workspace(ctx: &Ctx) -> CheckResult {
    let started = Instant::now();
    let r = ctx.result("tunnel-workspace", &["H2", "T-107"]);
    let repo = scratch(&ctx.data_dir, "repo");
    let out = async {
        let repo = repo?;
        git(&repo, &["init", "-q"])?;
        std::fs::write(repo.join("hola.txt"), "hola\n")?;
        git(&repo, &["add", "."])?;
        git(&repo, &["commit", "-q", "-m", "base"])?;
        let spec = WorkspaceSpec {
            repo_path: repo.to_string_lossy().into_owned(),
            keep: false,
        };
        with_tunnel(ctx, "hw-workspace", Some(spec), |c, t| {
            Box::pin(async move {
                // El agente de la otra máquina corre `git` en la carpeta del workspace.
                let commit = exec(
                    c,
                    t,
                    "git",
                    &[
                        "-c",
                        "user.name=tandem-hw",
                        "-c",
                        "user.email=hw@invalid",
                        "commit",
                        "-q",
                        "--allow-empty",
                        "-m",
                        "desde la otra máquina",
                    ],
                )
                .await?;
                stdout_until_done(c, t, &commit).await?;
                match req(
                    c,
                    UiCommand::BringBack {
                        tunnel: t,
                        commit: None,
                        name: Some("hw-suite".into()),
                    },
                )
                .await?
                {
                    Reply::BroughtBack(rep) if rep.branch.is_some() && !rep.commits.is_empty() => {
                        Ok(format!(
                            "volvió la rama con {} commit(s)",
                            rep.commits.len()
                        ))
                    }
                    other => anyhow::bail!("no volvió una rama con commits: {other:?}"),
                }
            })
        })
        .await
    }
    .await;
    done(r, started, out)
}

/// Carpeta de trabajo propia de una comprobación, dentro del directorio de datos de prueba.
fn scratch(data_dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let dir = data_dir.join("scratch").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub async fn run_tunnel(ctx: &Ctx, name: &str) -> Option<CheckResult> {
    Some(match name {
        "tunnel-process" => tunnel_process(ctx).await,
        "tunnel-files" => tunnel_files(ctx).await,
        "tunnel-port" => tunnel_port(ctx).await,
        "tunnel-workspace" => tunnel_workspace(ctx).await,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_pattern_is_deterministic_and_not_constant() {
        assert_eq!(pattern(1000), pattern(1000));
        let p = pattern(4096);
        assert!(p.iter().any(|b| *b != p[0]));
    }

    #[test]
    fn remote_paths_use_the_target_separator() {
        let ctx = Ctx {
            data_dir: PathBuf::new(),
            label: "pc".into(),
            peer_exe: "x".into(),
            remote_dir: r"C:\Temp\hw".into(),
            remote_sep: '\\',
        };
        assert_eq!(ctx.remote("hw.bin"), r"C:\Temp\hw\hw.bin");
    }

    #[test]
    fn every_edge_is_pushed_in_a_distinct_direction() {
        let mut dirs: Vec<_> = PUSHES
            .iter()
            .map(|(_, x, y)| (*x as i32, *y as i32))
            .collect();
        dirs.sort_unstable();
        dirs.dedup();
        assert_eq!(dirs.len(), 4);
    }

    #[test]
    fn scratch_starts_empty_each_time() {
        let dir = tempfile::tempdir().unwrap();
        let a = scratch(dir.path(), "x").unwrap();
        std::fs::write(a.join("viejo"), "1").unwrap();
        let b = scratch(dir.path(), "x").unwrap();
        assert_eq!(a, b);
        assert!(!b.join("viejo").exists());
    }

    #[tokio::test]
    async fn the_echo_helper_talks_to_an_echo_server() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 14];
            s.read_exact(&mut buf).await.unwrap();
            s.write_all(&buf).await.unwrap();
        });
        echo(port).await.unwrap();
    }
}
