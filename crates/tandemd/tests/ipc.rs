//! An IPC client asks a real agent for the state over the platform's transport
//! (Unix socket on Linux and macOS, named pipe on Windows) and receives it.

use std::path::PathBuf;
use std::sync::Arc;

use tandem_core::{Machine, State};
use tandem_proto::{AgentToUi, Reply, TxnEvent, UiCommand};
use tandemd::{Agent, AgentConfig, StateStore, daemon, ipc};
use tokio::sync::oneshot;

const CONFIG: &str = r#"
machine = "a"

# What to wake the PC with: without this the plan rejects powering it on.
[power.wol]
mac = "aa:bb:cc:dd:ee:ff"
"#;

fn endpoint(dir: &std::path::Path) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(format!(r"\\.\pipe\tandemd-test-ipc-{}", std::process::id()))
    } else {
        dir.join("tandemd.sock")
    }
}

#[tokio::test]
async fn client_requests_state_and_receives_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = StateStore::new(dir.path().join("state.json"));
    // A state saved by an earlier run.
    let cfg = AgentConfig::from_toml(CONFIG).unwrap();
    let mut saved = State::initial(Machine::A);
    saved.on.b = true;
    store.save(&saved).unwrap();

    let agent = Arc::new(Agent::new(cfg, store.clone()));
    let path = endpoint(dir.path());
    let listener = match ipc::bind(&path).await {
        Ok(l) => l,
        // The development sandbox (WSL) does not allow creating Unix sockets; in CI this is never skipped.
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
    let daemon = tokio::spawn(daemon::run(agent.clone(), listener, async {
        let _ = stopped.await;
    }));

    let mut client = ipc::connect(&path).await.unwrap();
    assert_eq!(client.agent(), Machine::A);
    assert_eq!(client.get_state().await.unwrap(), saved);

    // A second client at once, and a change that arrives pushed to both.
    let mut other = ipc::connect(&path).await.unwrap();
    assert_eq!(other.get_state().await.unwrap(), saved);
    let mut changed = saved.clone();
    changed.on.b = false;
    agent.set_state(changed.clone()).unwrap();
    assert_eq!(
        client.next_event().await.unwrap(),
        Some(AgentToUi::State(changed.clone()))
    );
    assert_eq!(
        other.next_event().await.unwrap(),
        Some(AgentToUi::State(changed.clone()))
    );
    // Without backends or another agent, powering on machine B starts and fails on its own.
    let reply = client
        .request(UiCommand::SetPower {
            machine: Machine::B,
            on: true,
        })
        .await
        .unwrap();
    let Reply::Accepted { txn } = reply else {
        panic!("Accepted was expected: {reply:?}");
    };
    loop {
        match client.next_event().await.unwrap() {
            Some(AgentToUi::Txn(TxnEvent::Failed { txn: t, state })) if t == txn => {
                assert_eq!(state, changed);
                break;
            }
            Some(AgentToUi::Txn(_)) => {}
            other => panic!("unexpected notice: {other:?}"),
        }
    }

    stop.send(()).unwrap();
    daemon.await.unwrap().unwrap();
    assert!(matches!(client.next_event().await, Ok(None)));
    assert_eq!(store.load().unwrap(), Some(changed));
    #[cfg(unix)]
    assert!(!path.exists(), "the socket is deleted on shutdown");
}
