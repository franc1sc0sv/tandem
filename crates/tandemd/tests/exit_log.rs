//! Every end of the agent leaves a line in `tandemd.log` before the process exits (T-334).
//! The compiled binary is run, so the line has to reach the file before `exit`.

use std::path::Path;
use std::process::{Command, Output};

fn log_of(data_dir: &Path) -> String {
    std::fs::read_to_string(data_dir.join("logs").join("tandemd.log")).unwrap_or_default()
}

fn run_agent(config: &Path, data_dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tandemd"))
        .arg("--config")
        .arg(config)
        .arg("--data-dir")
        .arg(data_dir)
        .arg("--sim-backends")
        .output()
        .unwrap()
}

/// On macOS this is the agent thread's `process::exit(1)`, past the AppKit run loop.
#[test]
fn a_failed_start_logs_the_exit_code_before_exiting() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("tandem.toml");
    std::fs::write(&config, "machine = \n").unwrap();
    let data = dir.path().join("data");

    let out = run_agent(&config, &data);

    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let log = log_of(&data);
    let last = log.lines().last().unwrap_or_default();
    assert!(
        last.contains("tandemd exiting with code 1: error:"),
        "last line: {last}\nlog:\n{log}"
    );
}

/// launchd's SIGTERM: the orderly shutdown says why it started and the last line is the exit.
/// Linux only, like `two_processes.rs`: the IPC socket lives in the data directory, and macOS's
/// temporary paths can exceed the Unix socket length.
#[cfg(target_os = "linux")]
#[test]
fn sigterm_logs_the_reason_and_the_exit() {
    use std::net::{Ipv4Addr, UdpSocket};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let dir = tempfile::tempdir().unwrap();
    if std::os::unix::net::UnixListener::bind(dir.path().join("probe.sock")).is_err()
        && std::env::var_os("CI").is_none()
    {
        eprintln!("skipped: this environment does not allow Unix sockets");
        return;
    }
    let port = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = dir.path().join("tandem.toml");
    std::fs::write(
        &config,
        format!("machine = \"a\"\n[network]\nport = {port}\n"),
    )
    .unwrap();
    let data = dir.path().join("data");
    let mut child = Command::new(env!("CARGO_BIN_EXE_tandemd"))
        .arg("--config")
        .arg(&config)
        .arg("--data-dir")
        .arg(&data)
        .arg("--sim-backends")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(30);
    while !log_of(&data).contains("IPC listening") {
        assert!(
            Instant::now() < deadline,
            "the agent did not start:\n{}",
            log_of(&data)
        );
        assert!(child.try_wait().unwrap().is_none(), "{}", log_of(&data));
        std::thread::sleep(Duration::from_millis(50));
    }
    let killed = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .unwrap();
    assert!(killed.success());
    let status = child.wait().unwrap();

    assert_eq!(status.code(), Some(0));
    let log = log_of(&data);
    assert!(log.contains("shutting down (SIGTERM)"), "{log}");
    let last = log.lines().last().unwrap_or_default();
    assert!(
        last.contains("tandemd exiting with code 0: stopped by SIGTERM"),
        "last line: {last}\nlog:\n{log}"
    );
}
