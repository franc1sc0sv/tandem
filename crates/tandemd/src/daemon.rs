//! El agente armado ([`start`]) y su bucle principal ([`run`]): acepta conexiones de la
//! UI hasta el apagado y lo hace en orden.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tandem_display::switcher::ManualSwitcher;
use tandem_link::identity::{Identity, IdentityError, TrustStore};
use tokio::sync::watch;
use tokio::task::JoinSet;

use tandem_display::ddc::profiles;

use crate::add_monitor::{MonitorWizard, platform_bus};
use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::input::{Input, InputSetup};
use crate::ipc::{Listener, serve_connection};
use crate::network::{NetError, NetOptions, Network};
use crate::orchestrator::LocalSteps;
use crate::paths::Paths;
use crate::store::{StateStore, StoreError};

/// Los pasos que esta máquina sabe hacer.
pub enum Backends {
    /// Los de la plataforma ([`crate::DisplaySteps::platform`] y
    /// [`InputSetup::platform`]).
    Platform,
    /// Otros (en las pruebas, simulados). `manual` es el que `local` usa para los
    /// cambios de entrada a mano, como en [`Agent::with_backends`]. Sin `input` no hay
    /// teclado, mouse ni portapapeles compartidos.
    Custom {
        local: Arc<dyn LocalSteps>,
        manual: Arc<ManualSwitcher>,
        input: Option<Box<InputSetup>>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("identidad de esta máquina: {0}")]
    Identity(#[source] IdentityError),
    #[error("confianza en el par: {0}")]
    Trust(#[source] IdentityError),
    #[error(transparent)]
    Net(#[from] NetError),
}

/// Un agente con su red en marcha; falta atender la UI con [`Daemon::serve`].
pub struct Daemon {
    pub agent: Arc<Agent>,
    pub network: Network,
    /// El motor de entrada; `None` sin backends de entrada.
    pub input: Option<Input>,
}

/// Arma el agente completo sobre el directorio de datos de `paths`: el estado
/// guardado, la identidad Ed25519 (se crea la primera vez), la confianza en el par y
/// la red. Tiene que llamarse dentro de un runtime de tokio.
pub fn start(
    cfg: AgentConfig,
    paths: &Paths,
    backends: Backends,
    net: NetOptions,
) -> Result<Daemon, StartError> {
    let store = StateStore::new(paths.state_file());
    let me = cfg.machine;
    let platform = matches!(backends, Backends::Platform);
    let (agent, input) = match backends {
        Backends::Platform => (
            Agent::with_platform(cfg, store),
            Some(Box::new(InputSetup::platform(me))),
        ),
        Backends::Custom {
            local,
            manual,
            input,
        } => (Agent::with_backends(cfg, store, local, manual), input),
    };
    let agent = Arc::new(agent);
    // «Agregar monitor» calibra con el DDC de la PC, que es la que cambia las entradas.
    if platform && me == tandem_core::Machine::Pc {
        agent.set_wizard(Arc::new(MonitorWizard::new(
            platform_bus(),
            Some(profiles::user_dir(&paths.data_dir)),
            Some(paths.config.clone()),
            agent.live_ddc().clone(),
        )));
    }
    let identity =
        Identity::load_or_create(&paths.identity_file()).map_err(StartError::Identity)?;
    tracing::info!(
        llave = %identity.public_key().to_hex(),
        "identidad de esta máquina"
    );
    let trust = TrustStore::open(paths.trust_file()).map_err(StartError::Trust)?;
    let network = Network::start(agent.clone(), &identity, trust, net)?;
    let input =
        input.map(|setup| crate::input::start(agent.clone(), network.active_link(), *setup));
    Ok(Daemon {
        agent,
        network,
        input,
    })
}

impl Daemon {
    /// Atiende la UI hasta que termina `shutdown`; después cierra la red.
    pub async fn serve<L: Listener>(
        self,
        listener: L,
        shutdown: impl Future<Output = ()>,
    ) -> Result<(), StoreError> {
        let result = run(self.agent, listener, shutdown).await;
        // Primero se suelta la captura: el teclado y el mouse vuelven a esta máquina.
        drop(self.input);
        self.network.shutdown().await;
        result
    }
}

/// Lo que se espera a que las conexiones abiertas terminen antes de cortarlas.
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Corre el agente hasta que `shutdown` termina.
///
/// Apagado ordenado: se deja de aceptar (el listener se suelta y, en Unix, borra su
/// socket), se avisa a las conexiones, se les da [`DRAIN_TIMEOUT`] y se guarda el estado.
pub async fn run<L: Listener>(
    agent: Arc<Agent>,
    mut listener: L,
    shutdown: impl Future<Output = ()>,
) -> Result<(), StoreError> {
    let (stop, stop_rx) = watch::channel(false);
    let mut conns = JoinSet::new();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok(stream) => {
                    let (agent, stop_rx) = (agent.clone(), stop_rx.clone());
                    conns.spawn(async move {
                        if let Err(e) = serve_connection(stream, agent, stop_rx).await {
                            tracing::debug!("conexión IPC terminada: {e}");
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!("no se pudo aceptar una conexión IPC: {e}");
                    // Evita girar en vacío si el error se repite.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
        }
    }

    tracing::info!("apagando");
    drop(listener);
    stop.send_replace(true);
    let drained = tokio::time::timeout(DRAIN_TIMEOUT, async {
        while conns.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        tracing::warn!("conexiones IPC que no cerraron a tiempo: se cortan");
        conns.shutdown().await;
    }
    agent.persist()?;
    tracing::info!("estado guardado en {}", agent.store_path().display());
    Ok(())
}

/// Termina con Ctrl+C, o con SIGTERM (launchd) / el apagado del sistema (Windows).
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(e) => {
                tracing::warn!("no se pudo escuchar SIGTERM: {e}");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(windows)]
    {
        use tokio::signal::windows::{ctrl_close, ctrl_shutdown};
        match (ctrl_close(), ctrl_shutdown()) {
            (Ok(mut close), Ok(mut shut)) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = close.recv() => {}
                    _ = shut.recv() => {}
                }
            }
            _ => {
                tracing::warn!("no se pudo escuchar el cierre de Windows");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use tandem_core::State;
    use tokio::sync::oneshot;

    use super::*;
    use crate::agent::tests::agent;
    use crate::ipc::{IpcClient, mem};
    use crate::store::StateStore;

    #[tokio::test]
    async fn serves_clients_and_saves_on_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (listener, connector) = mem::pair();
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(run(agent.clone(), listener, async {
            let _ = stopped.await;
        }));

        let mut a = IpcClient::handshake(connector.connect().unwrap())
            .await
            .unwrap();
        let mut b = IpcClient::handshake(connector.connect().unwrap())
            .await
            .unwrap();
        assert_eq!(a.get_state().await.unwrap(), agent.state());
        assert_eq!(b.get_state().await.unwrap(), agent.state());

        // El estado vive en memoria: se borra el archivo para ver que el apagado lo escribe.
        let store = StateStore::new(dir.path().join("state.json"));
        let _ = std::fs::remove_file(store.path());
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();

        assert_eq!(store.load().unwrap(), Some(agent.state()));
        // Las conexiones se cerraron y no se aceptan nuevas.
        assert!(matches!(a.next_event().await, Ok(None)));
        assert!(matches!(b.next_event().await, Ok(None)));
        assert!(connector.connect().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn stuck_connection_does_not_block_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (listener, connector) = mem::pair();
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(run(agent.clone(), listener, async {
            let _ = stopped.await;
        }));
        // Un cliente que conecta y nunca saluda.
        let _silent = connector.connect().unwrap();
        tokio::task::yield_now().await;
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
        let store = StateStore::new(dir.path().join("state.json"));
        assert_eq!(
            store.load().unwrap(),
            Some(State::initial(&agent.config().desk))
        );
    }
}
