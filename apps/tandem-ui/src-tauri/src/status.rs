//! Lo que la ventana muestra del agente: su estado leído por la IPC local, o una razón
//! clara de por qué no se pudo (agente apagado, sin permiso, otra versión...).
//!
//! La lógica es genérica sobre el transporte para probarla con la IPC en memoria; la app
//! usa el Unix socket o el named pipe reales de `tandemd`.

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

/// Cuánto se espera a `tandemd` antes de darlo por colgado. Por la IPC local responde en
/// milisegundos; más que esto es un agente trabado y la ventana no debe quedarse esperando.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Directorio de datos del agente, si `tandemd` corre con `--data-dir` (desarrollo).
pub const DATA_DIR_ENV: &str = "TANDEM_DATA_DIR";

/// Lo que recibe el frontend (JSON con `kind` como discriminante).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AgentStatus {
    Connected {
        /// Máquina en la que corre el agente.
        agent: Machine,
        state: State,
    },
    Unavailable {
        /// Dónde se buscó al agente, si se llegó a saber.
        endpoint: Option<String>,
        reason: String,
    },
}

/// Dónde escucha el `tandemd` de este usuario, con el mismo cálculo que usa el agente.
pub fn endpoint_from_env() -> Result<PathBuf, String> {
    let data_dir = std::env::var_os(DATA_DIR_ENV)
        .filter(|d| !d.is_empty())
        .map(PathBuf::from);
    endpoint_for(data_dir)
}

pub fn endpoint_for(data_dir: Option<PathBuf>) -> Result<PathBuf, String> {
    let paths = Paths::resolve(None, data_dir).map_err(|e| e.to_string())?;
    ipc::default_endpoint(&paths.data_dir)
        .map_err(|e| format!("no se pudo calcular dónde escucha tandemd: {e}"))
}

/// Pide el estado al agente que escucha en `endpoint`.
pub async fn fetch_status(endpoint: &Path, limit: Duration) -> AgentStatus {
    status_with(endpoint, ipc::connect(endpoint), limit).await
}

/// Como [`fetch_status`], con la conexión ya encaminada por el transporte que sea.
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
        Err(_) => format!("tandemd no respondió en {} s", limit.as_secs_f64()),
    };
    AgentStatus::Unavailable {
        endpoint: Some(endpoint.display().to_string()),
        reason,
    }
}

pub(crate) const CLOSED: &str = "tandemd cerró la conexión sin responder";

/// Un error de la IPC dicho de forma que el usuario sepa qué hacer.
pub(crate) fn explain(e: &IpcError) -> String {
    match e {
        IpcError::Io(io) => match io.kind() {
            // Sin socket/pipe, o un socket viejo de un agente que ya murió.
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                "tandemd no está corriendo: no hay nadie escuchando en la IPC local".into()
            }
            // Se cayó a mitad de la conversación.
            io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::UnexpectedEof => CLOSED.into(),
            io::ErrorKind::PermissionDenied => {
                "sin permiso para hablar con tandemd: su IPC es solo del usuario que lo corre"
                    .into()
            }
            _ => format!("no se pudo hablar con tandemd: {io}"),
        },
        IpcError::Proto(ProtoError::IncompatibleVersion { .. }) => {
            format!("{e}; actualiza la UI o tandemd para que coincidan")
        }
        IpcError::Closed => CLOSED.into(),
        other => format!("respuesta inesperada de tandemd: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tandem_core::BuiltinDisplay;
    use tandem_proto::{Hello, PROTOCOL_VERSION, Peer};
    use tandemd::ipc::Framed;
    use tandemd::ipc::mem::{self, MemConnector};
    use tandemd::{Agent, AgentConfig, StateStore, daemon};
    use tokio::io::{DuplexStream, duplex};
    use tokio::sync::oneshot;

    use super::*;

    const CONFIG: &str = r#"
machine = "pc"

[[monitors]]
id = "m34"
label = '34"'
wiring = { pc = "displayport", mac = "hdmi" }
"#;

    fn here() -> &'static Path {
        Path::new("/tmp/tandemd.sock")
    }

    /// Un agente real atendiendo por la IPC en memoria.
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
            other => panic!("se esperaba un error, llegó {other:?}"),
        }
    }

    #[tokio::test]
    async fn reads_the_agent_state() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, c, _stop) = agent(dir.path());
        let mut s = agent.state();
        s.pc_on = true;
        agent.set_state(s.clone()).unwrap();
        let status = status_with(here(), connect(&c), QUERY_TIMEOUT).await;
        assert_eq!(
            status,
            AgentStatus::Connected {
                agent: Machine::Pc,
                state: s
            }
        );
    }

    #[tokio::test]
    async fn json_is_what_the_frontend_reads() {
        let mut state = State {
            owner: Default::default(),
            pc_on: true,
            builtin_pref: BuiltinDisplay::Mirror,
        };
        state.owner.insert("m34".into(), Machine::Pc);
        let ok = AgentStatus::Connected {
            agent: Machine::Mac,
            state,
        };
        assert_eq!(
            serde_json::to_value(&ok).unwrap(),
            serde_json::json!({
                "kind": "connected",
                "agent": "mac",
                "state": { "owner": { "m34": "pc" }, "pc_on": true, "builtin_pref": "mirror" }
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
            assert!(r.contains("no está corriendo"), "{r}");
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
        // Apagado, el listener ya no está: conectar falla como un socket sin nadie.
        let r = reason(status_with(here(), connect(&c), QUERY_TIMEOUT).await);
        assert!(r.contains("no está corriendo"), "{r}");
    }

    #[tokio::test]
    async fn permission_denied_is_explained() {
        let failed = async {
            Err::<IpcClient<DuplexStream>, _>(
                io::Error::from(io::ErrorKind::PermissionDenied).into(),
            )
        };
        let r = reason(status_with(here(), failed, QUERY_TIMEOUT).await);
        assert!(r.contains("sin permiso"), "{r}");
    }

    #[tokio::test]
    async fn other_io_errors_keep_the_detail() {
        let failed =
            async { Err::<IpcClient<DuplexStream>, _>(io::Error::other("cable roto").into()) };
        let r = reason(status_with(here(), failed, QUERY_TIMEOUT).await);
        assert!(r.contains("cable roto"), "{r}");
    }

    #[tokio::test(start_paused = true)]
    async fn hung_agent_times_out() {
        // Acepta la conexión pero nunca saluda.
        let (client, _server) = duplex(64);
        let r =
            reason(status_with(here(), IpcClient::handshake(client), Duration::from_secs(2)).await);
        assert_eq!(r, "tandemd no respondió en 2 s");
    }

    #[tokio::test]
    async fn other_protocol_version_is_explained() {
        let (client, server) = duplex(1024);
        tokio::spawn(async move {
            let mut s = Framed::new(server);
            s.send(&Hello {
                version: PROTOCOL_VERSION + 1,
                peer: Peer::Agent(Machine::Mac),
            })
            .await
            .unwrap();
            let _ = s.recv_hello().await;
        });
        let r = reason(status_with(here(), IpcClient::handshake(client), QUERY_TIMEOUT).await);
        assert!(r.contains("versión de protocolo incompatible"), "{r}");
        assert!(r.contains("actualiza"), "{r}");
    }

    #[tokio::test]
    async fn agent_hanging_up_is_explained() {
        // Antes de que la UI salude: escribir falla.
        let (client, server) = duplex(64);
        drop(server);
        let r = reason(status_with(here(), IpcClient::handshake(client), QUERY_TIMEOUT).await);
        assert_eq!(r, CLOSED);

        // Después de leer el saludo, sin contestar.
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
        assert!(r.starts_with("respuesta inesperada de tandemd"), "{r}");
    }

    #[test]
    fn endpoint_follows_the_agent_data_dir() {
        let dir = PathBuf::from("/datos/Tandem");
        let got = endpoint_for(Some(dir.clone())).unwrap();
        assert_eq!(got, ipc::default_endpoint(&dir).unwrap());
        #[cfg(unix)]
        assert_eq!(got, dir.join("tandemd.sock"));
    }

    #[test]
    fn default_endpoint_is_in_the_app_dir() {
        // En CI siempre hay HOME / APPDATA.
        let got = endpoint_for(None).unwrap();
        let data = Paths::resolve(None, None).unwrap().data_dir;
        assert_eq!(got, ipc::default_endpoint(&data).unwrap());
    }
}
