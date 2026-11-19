//! Checks against a test agent that is already running, through its IPC. Each one returns a
//! [`CheckResult`]; none leaves a tunnel open or a forward alive when it ends.

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

/// What each check needs to know about its machine and the other one.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub data_dir: PathBuf,
    /// `pc` or `mac`.
    pub label: String,
    /// Path of `tandem-hw` on the other machine (tunnels use it as the test program:
    /// it comes out the same on Windows and macOS).
    pub peer_exe: String,
    /// Working directory of the other machine (temporary, already created) and its separator.
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

/// All the checks that `run all` runs on a machine, in order.
pub const TUNNEL_CHECKS: &[&str] = &[
    "tunnel-process",
    "tunnel-files",
    "tunnel-port",
    "tunnel-workspace",
];

async fn req(c: &mut Conn, cmd: UiCommand) -> anyhow::Result<Reply> {
    match tokio::time::timeout(STEP, c.request(cmd)).await {
        Ok(Ok(Reply::Rejected(why))) => anyhow::bail!("the agent rejected the request: {why}"),
        Ok(Ok(r)) => Ok(r),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => anyhow::bail!("the agent did not answer within {STEP:?}"),
    }
}

fn unexpected(r: Reply) -> anyhow::Error {
    anyhow::anyhow!("unexpected reply: {r:?}")
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

// --- Link -------------------------------------------------------------------------------

/// H1: the test agent sees the other machine paired, with a link.
pub async fn pair(ctx: &Ctx, wait: Duration) -> CheckResult {
    let started = Instant::now();
    let r = ctx.result("pair", &["H1"]);
    let out = async {
        let mut ui = conn::connect(&ctx.data_dir, Peer::Ui, IPC_WAIT).await?;
        loop {
            match req(&mut ui, UiCommand::GetPeer).await? {
                Reply::Peer(PeerStatus::Paired {
                    link: Some(link), ..
                }) => return Ok(format!("paired over {link:?}")),
                Reply::Peer(_) => {}
                other => return Err(unexpected(other)),
            }
            if started.elapsed() >= wait {
                anyhow::bail!("did not see the other machine within {wait:?}");
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    .await;
    done(r, started, out)
}

/// T-093 (control): ping latency over the link. The task sets no limit; it fails only if
/// a ping is lost or p99 exceeds 200 ms (a broken link, not a slow one).
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
            latency::percentiles(&mut millis).ok_or_else(|| anyhow::anyhow!("no pings"))?;
        let link = link.map_or_else(|| "?".to_string(), |l| format!("{l:?}"));
        let said = format!("{link}: p50 {p50} ms, p99 {p99} ms over {count} pings");
        anyhow::ensure!(p99 <= 200, "p99 too high: {said}");
        Ok(said)
    }
    .await;
    done(r, started, out)
}

// --- Synthetic input -----------------------------------------------------------------------

/// Moves the system cursor with real input, in small steps. It only exists on Windows and
/// macOS; on other platforms the check is skipped.
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
        Err("no input injection on this system".into())
    }

    fn motion(&mut self, _: f64, _: f64) -> Result<(), String> {
        Ok(())
    }
}

/// The four edges and the vector used to push against each one.
const PUSHES: [(&str, f64, f64); 4] = [
    ("right", 6.0, 0.0),
    ("left", -6.0, 0.0),
    ("bottom", 0.0, 6.0),
    ("top", 0.0, -6.0),
];

/// How `input-cross` moves the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drive {
    /// Real system injection (`CGEventPost` on macOS): it goes through the capture like the
    /// physical mouse. The macOS tap only lets through, unforwarded, events carrying
    /// `INJECTED_MARKER`, and the check's events do not carry it.
    System,
    /// Motion fed to the agent's input engine (`UiCommand::SyntheticMotion`). The Windows
    /// capture drops whatever `SendInput` injects on purpose (`LLMHF_INJECTED` in the hook, Raw
    /// Input without a device), so system injection never reaches the edge there (T-239).
    Agent,
}

impl Drive {
    /// The one that reaches the edge on `os` (`std::env::consts::OS`).
    pub fn for_os(os: &str) -> Self {
        if os == "windows" {
            Drive::Agent
        } else {
            Drive::System
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Drive::System => "synthetic system input",
            Drive::Agent => "synthetic motion fed to the agent",
        }
    }
}

/// What is pushing the cursor, until told to stop.
enum Pusher {
    System(std::thread::JoinHandle<Result<(), String>>),
    Agent(tokio::task::JoinHandle<anyhow::Result<()>>),
}

impl Pusher {
    async fn start(
        drive: Drive,
        ctx: &Ctx,
        dx: f64,
        dy: f64,
        stop: Arc<AtomicBool>,
    ) -> anyhow::Result<Self> {
        Ok(match drive {
            Drive::System => Pusher::System(spawn_injector(dx, dy, stop)),
            Drive::Agent => {
                // Its own connection: the main one keeps answering while this one pushes.
                let mut c = conn::connect(&ctx.data_dir, Peer::Ui, IPC_WAIT).await?;
                let (dx, dy) = (dx.round() as i32, dy.round() as i32);
                Pusher::Agent(tokio::spawn(async move {
                    while !stop.load(Ordering::Relaxed) {
                        req(&mut c, UiCommand::SyntheticMotion { dx, dy }).await?;
                        tokio::time::sleep(Duration::from_millis(2)).await;
                    }
                    Ok(())
                }))
            }
        })
    }

    async fn finish(self) -> anyhow::Result<()> {
        match self {
            Pusher::System(t) => tokio::task::spawn_blocking(move || t.join())
                .await?
                .map_err(|_| anyhow::anyhow!("the injection thread died"))?
                .map_err(anyhow::Error::msg),
            Pusher::Agent(t) => t.await?,
        }
    }
}

/// H4: this machine owns the keyboard and mouse; synthetic motion is pushed against each
/// edge until the agent reports `Crossed` toward the other. It does not move the physical
/// mouse. It keeps pushing for `keep_injecting` after crossing so the other machine can
/// measure its latency (`latency-watch` runs over there at the same time). How it pushes
/// depends on the system ([`Drive::for_os`]).
pub async fn input_cross(ctx: &Ctx, keep_injecting: Duration) -> CheckResult {
    input_cross_with(ctx, keep_injecting, Drive::for_os(std::env::consts::OS)).await
}

/// [`input_cross`] with the given way of pushing.
pub async fn input_cross_with(ctx: &Ctx, keep_injecting: Duration, drive: Drive) -> CheckResult {
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
            anyhow::bail!("the input engine does not work here: {problem}");
        }
        if !status.session {
            anyhow::bail!("no input session with the other machine");
        }
        if status.server != me {
            req(&mut ui, UiCommand::SetKeyboard(me)).await?;
        }
        // The crossing notice arrives on another connection, which is closed at the end.
        let mut watch = conn::connect(&ctx.data_dir, Peer::Ui, IPC_WAIT).await?;
        req(&mut watch, UiCommand::WatchCursor { on: true }).await?;
        for (edge, dx, dy) in PUSHES {
            let stop = Arc::new(AtomicBool::new(false));
            let pusher = Pusher::start(drive, ctx, dx, dy, stop.clone()).await?;
            let crossed = wait_crossed(&mut watch, me, Duration::from_secs(6)).await;
            if crossed {
                tokio::time::sleep(keep_injecting).await;
            }
            stop.store(true, Ordering::Relaxed);
            pusher.finish().await?;
            if crossed {
                // The keyboard and mouse are handed back to this machine for the next direction.
                let _ = req(&mut ui, UiCommand::SetKeyboard(me)).await;
                return Ok(format!("crossed at edge {edge} with {}", drive.describe()));
            }
        }
        anyhow::bail!(
            "no edge crossed to the other machine with {}",
            drive.describe()
        )
    }
    .await;
    match out {
        Err(e) if e.to_string().contains("no input injection") => {
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

/// T-093: the machine that receives the input measures its latency; it keeps watching for
/// `window` and judges the best 5 s window it saw (the one with the most events).
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
                anyhow::bail!("only {n} events measured: the input did not arrive (did it cross?)")
            }
            None => anyhow::bail!("the agent did not measure any latency within {window:?}"),
        }
    }
    .await;
    done(r, started, out)
}

// --- Tunnels ---------------------------------------------------------------------------------

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
        Reply::Tunnel { tunnel, .. } => Ok(*tunnel),
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
        allow_secret_env: false,
    };
    match req(c, UiCommand::Exec { tunnel, spec }).await? {
        Reply::Process(p) => Ok(*p),
        other => Err(unexpected(other)),
    }
}

/// Reads a process's output until it ends and returns stdout.
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
            "the process did not finish within {STEP:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

type StateRead<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Option<ProcessState>>> + 'a>>;

/// Reads a process's state with `fetch` until it is no longer running, or until `limit` passes, and
/// returns the last state read.
///
/// `Kill` returns when the destination has killed the tree, but the origin only learns the new
/// state over the tunnel's data channel, a different stream: reading it once right after `Kill`
/// can still see `Running`, so it is polled within a bounded wait.
async fn settled_state<C, F>(
    conn: &mut C,
    limit: Duration,
    poll: Duration,
    mut fetch: F,
) -> anyhow::Result<Option<ProcessState>>
where
    F: for<'a> FnMut(&'a mut C) -> StateRead<'a>,
{
    let deadline = Instant::now() + limit;
    loop {
        let state = fetch(conn).await?;
        if state.as_ref().is_none_or(|s| *s != ProcessState::Running) || Instant::now() >= deadline
        {
            return Ok(state);
        }
        tokio::time::sleep(poll).await;
    }
}

/// Opens a tunnel, runs `body` and always closes it, even if `body` fails.
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

/// T-104 / H2: a process on the other machine through the tunnel, its output, and killing it with its tree.
pub async fn tunnel_process(ctx: &Ctx) -> CheckResult {
    let started = Instant::now();
    let r = ctx.result("tunnel-process", &["H2", "T-103", "T-104"]);
    let exe = ctx.peer_exe.clone();
    let out = with_tunnel(ctx, "hw-process", None, |c, t| {
        Box::pin(async move {
            let p = exec(c, t, &exe, &["emit", "hw-hello"]).await?;
            let text = stdout_until_done(c, t, &p).await?;
            anyhow::ensure!(
                text.contains("hw-hello"),
                "the output did not arrive: {text:?}"
            );
            let long = exec(c, t, &exe, &["sleep", "300"]).await?;
            req(
                c,
                UiCommand::Kill {
                    tunnel: t,
                    process: long.id,
                },
            )
            .await?;
            let state = settled_state(c, STEP, Duration::from_millis(100), |c| {
                Box::pin(async move {
                    match req(c, UiCommand::GetProcesses { tunnel: t }).await? {
                        Reply::Processes(ps) => {
                            Ok(ps.into_iter().find(|p| p.id == long.id).map(|p| p.state))
                        }
                        other => Err(unexpected(other)),
                    }
                })
            })
            .await?;
            anyhow::ensure!(
                state == Some(ProcessState::Killed),
                "the process stayed in {state:?} for {STEP:?} after being killed"
            );
            Ok("output received from the other machine and process killed".into())
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

/// T-106: a 3 MiB file goes there and back through the tunnel identical.
pub async fn tunnel_files(ctx: &Ctx) -> CheckResult {
    let started = Instant::now();
    let r = ctx.result("tunnel-files", &["H2", "T-106"]);
    let remote = ctx.remote("hw.bin");
    let dir = scratch(&ctx.data_dir, "files");
    let out = async {
        let dir = dir?;
        let sent = dir.join("there.bin");
        let back = dir.join("back.bin");
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
                        include: Vec::new(),
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
                Ok(format!("{bytes} bytes round trip"))
            })
        })
        .await?;
        anyhow::ensure!(
            std::fs::read(&back)? == data,
            "the file came back different"
        );
        Ok(said)
    }
    .await;
    done(r, started, out)
}

/// T-106: a port forwarded through the tunnel reaches an echo server on the other machine.
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
                "echo through the forwarded port ({port} on the other machine)"
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
    anyhow::ensure!(
        &buf == b"tandem-hw-ping",
        "the echo came back different: {buf:?}"
    );
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
    anyhow::ensure!(st.success(), "git {args:?} failed");
    Ok(())
}

/// T-107: a git repo travels with the tunnel, a commit is made over there and it returns as a branch.
pub async fn tunnel_workspace(ctx: &Ctx) -> CheckResult {
    let started = Instant::now();
    let r = ctx.result("tunnel-workspace", &["H2", "T-107"]);
    let repo = scratch(&ctx.data_dir, "repo");
    let out = async {
        let repo = repo?;
        git(&repo, &["init", "-q"])?;
        std::fs::write(repo.join("hello.txt"), "hello\n")?;
        git(&repo, &["add", "."])?;
        git(&repo, &["commit", "-q", "-m", "base"])?;
        let spec = WorkspaceSpec {
            repo_path: repo.to_string_lossy().into_owned(),
            keep: false,
            include: Vec::new(),
        };
        with_tunnel(ctx, "hw-workspace", Some(spec), |c, t| {
            Box::pin(async move {
                // The agent on the other machine runs `git` in the workspace folder.
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
                        "from the other machine",
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
                            "the branch came back with {} commit(s)",
                            rep.commits.len()
                        ))
                    }
                    other => anyhow::bail!("no branch with commits came back: {other:?}"),
                }
            })
        })
        .await
    }
    .await;
    done(r, started, out)
}

/// Working directory of a check, inside the test data directory.
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

    /// T-239: on Windows system injection never reaches the edge (the capture drops it), so the
    /// check feeds the agent; on macOS it keeps going through the real tap.
    #[test]
    fn windows_pushes_through_the_agent_and_macos_through_the_system() {
        assert_eq!(Drive::for_os("windows"), Drive::Agent);
        assert_eq!(Drive::for_os("macos"), Drive::System);
        assert_eq!(Drive::for_os("linux"), Drive::System);
    }

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
        std::fs::write(a.join("old"), "1").unwrap();
        let b = scratch(dir.path(), "x").unwrap();
        assert_eq!(a, b);
        assert!(!b.join("old").exists());
    }

    /// A process whose state reaches the origin `after` reads late.
    fn lagging(
        after: usize,
        reads: Arc<std::sync::atomic::AtomicUsize>,
    ) -> impl FnMut(&mut ()) -> StateRead<'_> {
        move |_| {
            let n = reads.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::ready(Ok(Some(if n < after {
                ProcessState::Running
            } else {
                ProcessState::Killed
            }))))
        }
    }

    #[tokio::test]
    async fn a_state_that_arrives_late_is_waited_for() {
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let state = settled_state(
            &mut (),
            Duration::from_secs(5),
            Duration::from_millis(1),
            lagging(3, reads.clone()),
        )
        .await
        .unwrap();
        assert_eq!(state, Some(ProcessState::Killed));
        assert_eq!(
            reads.load(Ordering::SeqCst),
            4,
            "it stops reading once settled"
        );
    }

    #[tokio::test]
    async fn a_process_that_never_stops_is_reported_after_the_limit() {
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let started = Instant::now();
        let state = settled_state(
            &mut (),
            Duration::from_millis(50),
            Duration::from_millis(5),
            lagging(usize::MAX, reads.clone()),
        )
        .await
        .unwrap();
        assert_eq!(state, Some(ProcessState::Running));
        assert!(started.elapsed() >= Duration::from_millis(50));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the wait is bounded"
        );
        assert!(reads.load(Ordering::SeqCst) > 2, "it polled, not read once");
    }

    #[tokio::test]
    async fn a_missing_process_is_not_waited_for_forever() {
        let state = settled_state(
            &mut (),
            Duration::from_millis(30),
            Duration::from_millis(5),
            |_| Box::pin(std::future::ready(Ok(None))),
        )
        .await
        .unwrap();
        assert_eq!(state, None);
    }

    #[tokio::test]
    async fn a_failed_read_is_an_error() {
        let r = settled_state(
            &mut (),
            Duration::from_secs(1),
            Duration::from_millis(5),
            |_| Box::pin(std::future::ready(Err(anyhow::anyhow!("link down")))),
        )
        .await;
        assert!(r.is_err());
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
