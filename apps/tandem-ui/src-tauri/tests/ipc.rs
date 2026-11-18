//! The UI reads the state of a real `tandemd` over the platform's transport (Unix
//! socket on Linux and macOS, named pipe on Windows), and explains when there is no agent.

use std::path::PathBuf;
use std::sync::Arc;

use tandem_core::{Machine, State};
use tandem_ui::status::{AgentStatus, QUERY_TIMEOUT, fetch_status};
use tandemd::{Agent, AgentConfig, StateStore, daemon, ipc};
use tokio::sync::oneshot;

const CONFIG: &str = r#"
machine = "a"
"#;

fn endpoint(dir: &std::path::Path, name: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(format!(
            r"\\.\pipe\tandem-ui-test-{name}-{}",
            std::process::id()
        ))
    } else {
        dir.join(format!("{name}.sock"))
    }
}

#[tokio::test]
async fn reads_state_from_a_running_agent() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = AgentConfig::from_toml(CONFIG).unwrap();
    let mut saved = State::initial(Machine::A);
    saved.on.b = true;
    let store = StateStore::new(dir.path().join("state.json"));
    store.save(&saved).unwrap();
    let agent = Arc::new(Agent::new(cfg, store));

    let path = endpoint(dir.path(), "up");
    let listener = match ipc::bind(&path).await {
        Ok(l) => l,
        // The development sandbox (WSL) does not allow creating Unix sockets; on CI this is never skipped.
        Err(ipc::IpcError::Io(e))
            if e.kind() == std::io::ErrorKind::PermissionDenied
                && std::env::var_os("CI").is_none() =>
        {
            eprintln!("skipped: this environment does not allow Unix sockets");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    let (stop, stopped) = oneshot::channel::<()>();
    let daemon = tokio::spawn(daemon::run(agent, listener, async {
        let _ = stopped.await;
    }));

    assert_eq!(
        fetch_status(&path, QUERY_TIMEOUT).await,
        AgentStatus::Connected {
            agent: Machine::A,
            state: saved
        }
    );

    stop.send(()).unwrap();
    daemon.await.unwrap().unwrap();
    // Already off, the same window explains there is no agent.
    assert!(matches!(
        fetch_status(&path, QUERY_TIMEOUT).await,
        AgentStatus::Unavailable { .. }
    ));
}

#[tokio::test]
async fn no_agent_is_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = endpoint(dir.path(), "down");
    match fetch_status(&path, QUERY_TIMEOUT).await {
        // The development sandbox (WSL) does not allow using Unix sockets; on CI this is never skipped.
        AgentStatus::Unavailable { reason, .. }
            if reason.starts_with("no permission") && std::env::var_os("CI").is_none() =>
        {
            eprintln!("skipped: this environment does not allow Unix sockets");
        }
        AgentStatus::Unavailable { endpoint, reason } => {
            assert_eq!(endpoint, Some(path.display().to_string()));
            assert!(reason.contains("tandemd is not running"), "{reason}");
        }
        other => panic!("no agent cannot have a state: {other:?}"),
    }
}
