//! La UI lee el estado de un `tandemd` real por el transporte de la plataforma (Unix
//! socket en Linux y macOS, named pipe en Windows), y explica cuando no hay agente.

use std::path::PathBuf;
use std::sync::Arc;

use tandem_core::{Machine, State};
use tandem_ui::status::{AgentStatus, QUERY_TIMEOUT, fetch_status};
use tandemd::{Agent, AgentConfig, StateStore, daemon, ipc};
use tokio::sync::oneshot;

const CONFIG: &str = r#"
machine = "mac"

[[monitors]]
id = "m34"
label = '34"'
wiring = { pc = "displayport", mac = "hdmi" }
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
    let mut saved = State::initial(&cfg.desk);
    saved.owner.insert("m34".into(), Machine::Pc);
    let store = StateStore::new(dir.path().join("state.json"));
    store.save(&saved).unwrap();
    let agent = Arc::new(Agent::new(cfg, store));

    let path = endpoint(dir.path(), "up");
    let listener = match ipc::bind(&path).await {
        Ok(l) => l,
        // El sandbox de desarrollo (WSL) no deja crear sockets Unix; en CI esto nunca se omite.
        Err(ipc::IpcError::Io(e))
            if e.kind() == std::io::ErrorKind::PermissionDenied
                && std::env::var_os("CI").is_none() =>
        {
            eprintln!("omitida: este entorno no permite sockets Unix");
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
            agent: Machine::Mac,
            state: saved
        }
    );

    stop.send(()).unwrap();
    daemon.await.unwrap().unwrap();
    // Ya apagado, la misma ventana explica que no hay agente.
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
        // El sandbox de desarrollo (WSL) no deja usar sockets Unix; en CI esto nunca se omite.
        AgentStatus::Unavailable { reason, .. }
            if reason.starts_with("sin permiso") && std::env::var_os("CI").is_none() =>
        {
            eprintln!("omitida: este entorno no permite sockets Unix");
        }
        AgentStatus::Unavailable { endpoint, reason } => {
            assert_eq!(endpoint, Some(path.display().to_string()));
            assert!(reason.contains("tandemd no está corriendo"), "{reason}");
        }
        other => panic!("sin agente no puede haber estado: {other:?}"),
    }
}
