//! What the window shows of the agent: its state read over the local IPC, or a clear reason
//! why it could not be (agent off, no permission, another version...).
//!
//! The logic is generic over the transport so it can be tested with the in-memory IPC; the app
//! uses `tandemd`'s real Unix socket or named pipe.

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tandem_core::{Machine, State};
use tandem_proto::ProtoError;
use tandemd::ipc::{self, IpcClient, IpcError};
use tandemd::paths::Paths;
use tokio::io::{AsyncRead, AsyncWrite};

/// How long `tandemd` is waited for before considering it hung. Over the local IPC it answers in
/// milliseconds; more than this is a stuck agent and the window must not stay waiting.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

/// The agent's data directory, if `tandemd` runs with `--data-dir` (development).
pub const DATA_DIR_ENV: &str = "TANDEM_DATA_DIR";

/// What the frontend receives (JSON with `kind` as the discriminant).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AgentStatus {
    Connected {
        /// Machine the agent runs on.
        agent: Machine,
        state: State,
    },
    Unavailable {
        /// Where the agent was looked for, if it got to be known.
        endpoint: Option<String>,
        reason: String,
    },
}

/// Where this user's `tandemd` listens, with the same calculation the agent uses.
pub fn endpoint_from_env() -> Result<PathBuf, String> {
    let data_dir = std::env::var_os(DATA_DIR_ENV)
        .filter(|d| !d.is_empty())
        .map(PathBuf::from);
    endpoint_for(data_dir)
}

pub fn endpoint_for(data_dir: Option<PathBuf>) -> Result<PathBuf, String> {
    let paths = Paths::resolve(None, data_dir).map_err(|e| e.to_string())?;
    ipc::default_endpoint(&paths.data_dir)
        .map_err(|e| format!("could not work out where tandemd listens: {e}"))
}

/// Asks the agent listening at `endpoint` for its state.
pub async fn fetch_status(endpoint: &Path, limit: Duration) -> AgentStatus {
    status_with(endpoint, ipc::connect(endpoint), limit).await
}

/// Like [`fetch_status`], with the connection already routed over whatever transport.
pub async fn status_with<S, C>(endpoint: &Path, connect: C, limit: Duration) -> AgentStatus
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Future<Output = Result<IpcClient<S>, IpcError>>,
{
    let query = async {
        let mut client = connect.await?;
        let state = client.get_state().await?;
        Ok::<_, IpcError>((client.agent(), state))
    };
    let reason = match tokio::time::timeout(limit, query).await {
        Ok(Ok((agent, state))) => return AgentStatus::Connected { agent, state },
        Ok(Err(e)) => explain(&e),
        Err(_) => format!("tandemd did not respond in {} s", limit.as_secs_f64()),
    };
    AgentStatus::Unavailable {
        endpoint: Some(endpoint.display().to_string()),
        reason,
    }
}

pub(crate) const CLOSED: &str = "tandemd closed the connection without responding";

/// An IPC error worded so the user knows what to do.
pub(crate) fn explain(e: &IpcError) -> String {
    match e {
        IpcError::Io(io) => match io.kind() {
            // No socket/pipe, or an old socket of an agent that already died.
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                "tandemd is not running: nobody is listening on the local IPC".into()
            }
            // It dropped midway through the conversation.
            io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::UnexpectedEof => CLOSED.into(),
            io::ErrorKind::PermissionDenied => {
                "no permission to talk to tandemd: its IPC is only for the user running it".into()
            }
            _ => format!("could not talk to tandemd: {io}"),
        },
        IpcError::Proto(ProtoError::IncompatibleVersion { .. }) => {
            format!("{e}; update the UI or tandemd so they match")
        }
        IpcError::Closed => CLOSED.into(),
        other => format!("unexpected reply from tandemd: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tandem_proto::{Hello, PROTOCOL_VERSION, Peer};
    use tandemd::ipc::Framed;
    use tandemd::ipc::mem::{self, MemConnector};
    use tandemd::{Agent, AgentConfig, StateStore, daemon};
    use tokio::io::{DuplexStream, duplex};
    use tokio::sync::oneshot;

    use super::*;

    const CONFIG: &str = r#"
machine = "b"
"#;

    fn here() -> &'static Path {
        Path::new("/tmp/tandemd.sock")
    }

    /// A real agent serving over the in-memory IPC.
    fn agent(dir: &Path) -> (Arc<Agent>, MemConnector, Daemon) {
        let cfg = AgentConfig::from_toml(CONFIG).unwrap();
        let agent = Arc::new(Agent::new(cfg, StateStore::new(dir.join("state.json"))));
        let (listener, connector) = mem::pair();
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(daemon::run(agent.clone(), listener, async {
            let _ = stopped.await;
        }));
        (agent, connector, Daemon { stop, task })
    }

    struct Daemon {
        stop: oneshot::Sender<()>,
        task: tokio::task::JoinHandle<Result<(), tandemd::StoreError>>,
    }

    async fn connect(c: &MemConnector) -> Result<IpcClient<DuplexStream>, IpcError> {
        IpcClient::handshake(c.connect()?).await
    }

    fn reason(s: AgentStatus) -> String {
        match s {
            AgentStatus::Unavailable { endpoint, reason } => {
                assert_eq!(
                    endpoint.as_deref(),
                    Some(here().display().to_string().as_str())
                );
                reason
            }
            other => panic!("an error was expected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reads_the_agent_state() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, c, _stop) = agent(dir.path());
        let mut s = agent.state();
        s.on.b = true;
        agent.set_state(s.clone()).unwrap();
        let status = status_with(here(), connect(&c), QUERY_TIMEOUT).await;
        assert_eq!(
            status,
            AgentStatus::Connected {
                agent: Machine::B,
                state: s
            }
        );
    }

    #[tokio::test]
    async fn json_is_what_the_frontend_reads() {
        let state = State {
            on: tandem_core::Powered::both(),
            scene: None,
        };
        let ok = AgentStatus::Connected {
            agent: Machine::A,
            state,
        };
        assert_eq!(
            serde_json::to_value(&ok).unwrap(),
            serde_json::json!({
                "kind": "connected",
                "agent": "a",
                "state": { "on": { "a": true, "b": true }, "scene": null }
            })
        );
        let down = AgentStatus::Unavailable {
            endpoint: None,
            reason: "x".into(),
        };
        assert_eq!(
            serde_json::to_value(&down).unwrap(),
            serde_json::json!({ "kind": "unavailable", "endpoint": null, "reason": "x" })
        );
    }

    #[tokio::test]
    async fn agent_not_running_is_explained() {
        for kind in [io::ErrorKind::NotFound, io::ErrorKind::ConnectionRefused] {
            let failed =
                async move { Err::<IpcClient<DuplexStream>, _>(io::Error::from(kind).into()) };
            let r = reason(status_with(here(), failed, QUERY_TIMEOUT).await);
            assert!(r.contains("is not running"), "{r}");
        }
    }

    #[tokio::test]
    async fn stopped_agent_is_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let (_agent, c, daemon) = agent(dir.path());
        assert!(matches!(
            status_with(here(), connect(&c), QUERY_TIMEOUT).await,
            AgentStatus::Connected { .. }
        ));
        daemon.stop.send(()).unwrap();
        daemon.task.await.unwrap().unwrap();
        // Off, the listener is gone: connecting fails like a socket with nobody.
        let r = reason(status_with(here(), connect(&c), QUERY_TIMEOUT).await);
        assert!(r.contains("is not running"), "{r}");
    }

    #[tokio::test]
    async fn permission_denied_is_explained() {
        let failed = async {
            Err::<IpcClient<DuplexStream>, _>(
                io::Error::from(io::ErrorKind::PermissionDenied).into(),
            )
        };
        let r = reason(status_with(here(), failed, QUERY_TIMEOUT).await);
        assert!(r.contains("no permission"), "{r}");
    }

    #[tokio::test]
    async fn other_io_errors_keep_the_detail() {
        let failed =
            async { Err::<IpcClient<DuplexStream>, _>(io::Error::other("broken cable").into()) };
        let r = reason(status_with(here(), failed, QUERY_TIMEOUT).await);
        assert!(r.contains("broken cable"), "{r}");
    }

    #[tokio::test(start_paused = true)]
    async fn hung_agent_times_out() {
        // Accepts the connection but never greets.
        let (client, _server) = duplex(64);
        let r =
            reason(status_with(here(), IpcClient::handshake(client), Duration::from_secs(2)).await);
        assert_eq!(r, "tandemd did not respond in 2 s");
    }

    #[tokio::test]
    async fn other_protocol_version_is_explained() {
        let (client, server) = duplex(1024);
        tokio::spawn(async move {
            let mut s = Framed::new(server);
            s.send(&Hello {
                version: PROTOCOL_VERSION + 1,
                peer: Peer::Agent(Machine::A),
            })
            .await
            .unwrap();
            let _ = s.recv_hello().await;
        });
        let r = reason(status_with(here(), IpcClient::handshake(client), QUERY_TIMEOUT).await);
        assert!(r.contains("incompatible protocol version"), "{r}");
        assert!(r.contains("update the UI or tandemd"), "{r}");
    }

    #[tokio::test]
    async fn agent_hanging_up_is_explained() {
        // Before the UI greets: writing fails.
        let (client, server) = duplex(64);
        drop(server);
        let r = reason(status_with(here(), IpcClient::handshake(client), QUERY_TIMEOUT).await);
        assert_eq!(r, CLOSED);

        // After reading the greeting, without answering.
        let (client, server) = duplex(1024);
        tokio::spawn(async move {
            let mut s = Framed::new(server);
            let _ = s.recv_hello().await;
        });
        let r = reason(status_with(here(), IpcClient::handshake(client), QUERY_TIMEOUT).await);
        assert_eq!(r, CLOSED);
    }

    #[tokio::test]
    async fn non_agent_peer_is_unexpected() {
        let (client, server) = duplex(1024);
        tokio::spawn(async move {
            let mut s = Framed::new(server);
            s.send(&Hello::new(Peer::Ui)).await.unwrap();
            let _ = s.recv_hello().await;
        });
        let r = reason(status_with(here(), IpcClient::handshake(client), QUERY_TIMEOUT).await);
        assert!(r.starts_with("unexpected reply from tandemd"), "{r}");
    }

    #[test]
    fn endpoint_follows_the_agent_data_dir() {
        let dir = PathBuf::from("/data/Tandem");
        let got = endpoint_for(Some(dir.clone())).unwrap();
        assert_eq!(got, ipc::default_endpoint(&dir).unwrap());
        #[cfg(unix)]
        assert_eq!(got, dir.join("tandemd.sock"));
    }

    #[test]
    fn default_endpoint_is_in_the_app_dir() {
        // On CI there is always HOME / APPDATA.
        let got = endpoint_for(None).unwrap();
        let data = Paths::resolve(None, None).unwrap().data_dir;
        assert_eq!(got, ipc::default_endpoint(&data).unwrap());
    }
}
