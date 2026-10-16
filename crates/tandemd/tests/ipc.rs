//! Un cliente IPC pide el estado a un agente real por el transporte de la plataforma
//! (Unix socket en Linux y macOS, named pipe en Windows) y lo recibe.

use std::path::PathBuf;
use std::sync::Arc;

use tandem_core::{Machine, State};
use tandem_proto::{AgentToUi, Reply, TxnEvent, UiCommand};
use tandemd::{Agent, AgentConfig, StateStore, daemon, ipc};
use tokio::sync::oneshot;

const CONFIG: &str = r#"
machine = "mac"

[[monitors]]
id = "m34"
label = '34"'
wiring = { pc = "displayport", mac = "hdmi" }

[[monitors]]
id = "m32"
label = '32"'
wiring = { pc = "displayport", mac = "hdmi" }
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
    // Un estado guardado por una corrida anterior.
    let cfg = AgentConfig::from_toml(CONFIG).unwrap();
    let mut saved = State::initial(&cfg.desk, Machine::Mac);
    saved.owner.insert("m34".into(), Machine::Pc);
    saved.on.pc = true;
    store.save(&saved).unwrap();

    let agent = Arc::new(Agent::new(cfg, store.clone()));
    let path = endpoint(dir.path());
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
    let daemon = tokio::spawn(daemon::run(agent.clone(), listener, async {
        let _ = stopped.await;
    }));

    let mut client = ipc::connect(&path).await.unwrap();
    assert_eq!(client.agent(), Machine::Mac);
    assert_eq!(client.get_state().await.unwrap(), saved);

    // Un segundo cliente a la vez, y un cambio que llega empujado a los dos.
    let mut other = ipc::connect(&path).await.unwrap();
    assert_eq!(other.get_state().await.unwrap(), saved);
    let mut changed = saved.clone();
    changed.on.pc = false;
    agent.set_state(changed.clone()).unwrap();
    assert_eq!(
        client.next_event().await.unwrap(),
        Some(AgentToUi::State(changed.clone()))
    );
    assert_eq!(
        other.next_event().await.unwrap(),
        Some(AgentToUi::State(changed.clone()))
    );
    // Sin backends ni otro agente, el cambio de modo arranca y vuelve atrás solo.
    let reply = client
        .request(UiCommand::SetMode(tandem_core::Mode::SoloMac))
        .await
        .unwrap();
    let Reply::Accepted { txn } = reply else {
        panic!("se esperaba Accepted: {reply:?}");
    };
    loop {
        match client.next_event().await.unwrap() {
            Some(AgentToUi::Txn(TxnEvent::RolledBack { txn: t, state })) if t == txn => {
                assert_eq!(state, changed);
                break;
            }
            Some(AgentToUi::Txn(_)) => {}
            other => panic!("aviso inesperado: {other:?}"),
        }
    }

    stop.send(()).unwrap();
    daemon.await.unwrap().unwrap();
    assert!(matches!(client.next_event().await, Ok(None)));
    assert_eq!(store.load().unwrap(), Some(changed));
    #[cfg(unix)]
    assert!(!path.exists(), "el socket se borra al apagar");
}
