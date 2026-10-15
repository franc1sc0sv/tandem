//! Un cliente IPC pide el estado a un agente real por el transporte de la plataforma
//! (Unix socket en Linux y macOS, named pipe en Windows) y lo recibe.

use std::path::PathBuf;
use std::sync::Arc;

use tandem_core::{Machine, State};
use tandem_proto::{AgentToUi, Reply, UiCommand};
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
    let mut saved = State::initial(&cfg.desk);
    saved.owner.insert("m34".into(), Machine::Pc);
    saved.pc_on = true;
    store.save(&saved).unwrap();

    let agent = Arc::new(Agent::new(cfg, store.clone()));
    let path = endpoint(dir.path());
    let listener = ipc::bind(&path).await.unwrap();
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
    changed.pc_on = false;
    agent.set_state(changed.clone()).unwrap();
    assert_eq!(
        client.next_event().await.unwrap(),
        Some(AgentToUi::State(changed.clone()))
    );
    assert_eq!(
        other.next_event().await.unwrap(),
        Some(AgentToUi::State(changed.clone()))
    );
    assert!(matches!(
        client
            .request(UiCommand::SetMode(tandem_core::Mode::SoloMac))
            .await
            .unwrap(),
        Reply::Rejected(_)
    ));

    stop.send(()).unwrap();
    daemon.await.unwrap().unwrap();
    assert!(matches!(client.next_event().await, Ok(None)));
    assert_eq!(store.load().unwrap(), Some(changed));
    #[cfg(unix)]
    assert!(!path.exists(), "el socket se borra al apagar");
}
