//! One agent per data directory (T-343), with real `tandemd` processes (the binary built
//! with `--sim-backends`): a second one on the same directory refuses to start without
//! touching the first, and the OS gives the lock back when the first dies, even with SIGKILL.
//!
//! It runs on the three platforms. On Windows the pipe's name does not depend on the data
//! directory, so the tests take turns instead of running in parallel.

use std::fs::File;
use std::net::{Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tandemd::ipc;
use tokio::sync::{Mutex, MutexGuard};

const TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(50);

/// Takes turns for the IPC endpoint (see the module's note).
async fn one_at_a_time() -> MutexGuard<'static, ()> {
    static TURN: Mutex<()> = Mutex::const_new(());
    TURN.lock().await
}

/// A short directory: on macOS the Unix socket's path is limited to 104 bytes.
fn short_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("t")
        .tempdir_in(std::env::temp_dir())
        .unwrap()
}

fn free_port() -> u16 {
    UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A configuration with a port of its own, so the agents never meet on the default one.
fn write_config(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::write(
        &path,
        format!("machine = \"a\"\n[network]\nport = {}\n", free_port()),
    )
    .unwrap();
    path
}

/// A `tandemd` process; if the test fails, it shows its output.
struct Proc {
    child: Child,
    out: PathBuf,
    err: PathBuf,
}

impl Proc {
    fn spawn(root: &Path, tag: &str, config: &Path, data: &Path) -> Self {
        let out = root.join(format!("{tag}.out"));
        let err = root.join(format!("{tag}.err"));
        let child = Command::new(env!("CARGO_BIN_EXE_tandemd"))
            .arg("--config")
            .arg(config)
            .arg("--data-dir")
            .arg(data)
            .arg("--sim-backends")
            .stdin(Stdio::null())
            .stdout(File::create(&out).unwrap())
            .stderr(File::create(&err).unwrap())
            .spawn()
            .unwrap();
        Proc { child, out, err }
    }

    fn stderr(&self) -> String {
        std::fs::read_to_string(&self.err).unwrap_or_default()
    }

    /// Waits for the process to end on its own.
    fn exit(&mut self) -> std::process::ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(start.elapsed() < TIMEOUT, "the process did not end");
            std::thread::sleep(POLL);
        }
    }

    /// Without an orderly shutdown: SIGKILL, or `TerminateProcess` on Windows.
    fn kill(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }

    /// Connects when the endpoint is ready; if the process died before, it fails.
    async fn serving(&mut self, data: &Path) {
        let endpoint = ipc::default_endpoint(data).unwrap();
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!("the agent ended at startup: {status}");
            }
            if let Ok(mut ui) = ipc::connect(&endpoint).await {
                ui.get_state().await.unwrap();
                return;
            }
            assert!(start.elapsed() < TIMEOUT, "the agent never served IPC");
            tokio::time::sleep(POLL).await;
        }
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            let out = std::fs::read_to_string(&self.out).unwrap_or_default();
            eprintln!(
                "--- stdout ---\n{out}\n--- stderr ---\n{}",
                std::fs::read_to_string(&self.err).unwrap_or_default()
            );
        }
    }
}

/// Opens a new connection and asks for the state: the agent answers as before.
async fn answers(data: &Path) {
    let endpoint = ipc::default_endpoint(data).unwrap();
    let mut ui = ipc::connect(&endpoint).await.unwrap();
    ui.get_state().await.unwrap();
}

#[tokio::test]
async fn a_second_agent_on_the_same_data_dir_refuses_and_the_first_keeps_serving() {
    let _turn = one_at_a_time().await;
    let root = short_tempdir();
    let data = root.path().join("data");
    let mut first = Proc::spawn(
        root.path(),
        "first",
        &write_config(root.path(), "a.toml"),
        &data,
    );
    first.serving(&data).await;

    // Its configuration does not exist: a refusal before anything else must not generate it.
    let config = root.path().join("second.toml");
    let mut second = Proc::spawn(root.path(), "second", &config, &data);
    let status = second.exit();

    assert!(!status.success(), "the second agent must fail: {status}");
    let err = second.stderr();
    assert!(
        err.contains(&format!("pid {}", first.child.id())),
        "the error must name the PID that holds the data directory: {err}"
    );
    assert!(
        !config.exists(),
        "the second agent touched its configuration"
    );
    assert!(
        first.child.try_wait().unwrap().is_none(),
        "the first agent ended"
    );
    answers(&data).await;
}

#[tokio::test]
async fn a_new_agent_starts_after_the_first_one_is_killed() {
    let _turn = one_at_a_time().await;
    let root = short_tempdir();
    let data = root.path().join("data");
    let config = write_config(root.path(), "a.toml");
    let mut first = Proc::spawn(root.path(), "first", &config, &data);
    first.serving(&data).await;

    first.kill();

    let mut next = Proc::spawn(root.path(), "next", &config, &data);
    next.serving(&data).await;
    assert!(next.child.try_wait().unwrap().is_none());
}

#[tokio::test]
async fn one_shot_commands_do_not_take_the_lock() {
    let _turn = one_at_a_time().await;
    let root = short_tempdir();
    let data = root.path().join("data");
    let config = write_config(root.path(), "a.toml");
    let mut agent = Proc::spawn(root.path(), "agent", &config, &data);
    agent.serving(&data).await;

    let check = Command::new(env!("CARGO_BIN_EXE_tandemd"))
        .arg("--check-config")
        .arg(&config)
        .output()
        .unwrap();
    assert!(check.status.success(), "{check:?}");
    let version = Command::new(env!("CARGO_BIN_EXE_tandemd"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(version.status.success(), "{version:?}");
    answers(&data).await;
}
