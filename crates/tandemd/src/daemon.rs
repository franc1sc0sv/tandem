//! El agente armado ([`start`]) y su bucle principal ([`run`]): acepta conexiones de la
//! UI hasta el apagado y lo hace en orden.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tandem_core::Machine;
use tandem_link::identity::{Identity, IdentityError, PublicKey, TrustStore};
use tokio::sync::watch;
use tokio::task::JoinSet;

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
    /// Los de la plataforma ([`crate::PowerSteps::for_machine`] y
    /// [`InputSetup::platform`]).
    Platform,
    /// Otros (en las pruebas, simulados). Sin `input` no hay teclado, mouse ni
    /// portapapeles compartidos.
    Custom {
        local: Arc<dyn LocalSteps>,
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

/// El lado de esta máquina según la identidad (DECISIONS A1): frente al par de la
/// confianza, la clave menor es `A`. `None` sin par (o con la misma clave, que no es un
/// par): entonces vale lo guardado.
pub fn resolve_side(own: &PublicKey, trust: &TrustStore) -> Option<Machine> {
    let peer = trust.peers().next()?;
    Machine::of_keys(own.as_bytes(), peer.public_key.as_bytes())
}

/// Arma el agente completo sobre el directorio de datos de `paths`: la identidad Ed25519
/// (se crea la primera vez), la confianza en el par, el lado que sale de las dos, el estado
/// guardado y la red. Tiene que llamarse dentro de un runtime de tokio.
pub fn start(
    cfg: AgentConfig,
    paths: &Paths,
    backends: Backends,
    net: NetOptions,
) -> Result<Daemon, StartError> {
    let identity =
        Identity::load_or_create(&paths.identity_file()).map_err(StartError::Identity)?;
    tracing::info!(
        llave = %identity.public_key().to_hex(),
        "identidad de esta máquina"
    );
    let trust = TrustStore::open(paths.trust_file()).map_err(StartError::Trust)?;
    let cfg = match resolve_side(&identity.public_key(), &trust) {
        Some(side) => {
            if side != cfg.machine {
                tracing::info!(lado = %side, "por las claves, esta máquina es el otro lado");
            }
            cfg.with_side(side)
        }
        None => cfg,
    };
    let store = StateStore::new(paths.state_file());
    let platform = matches!(backends, Backends::Platform);
    let (agent, input) = match backends {
        Backends::Platform => (
            Agent::with_platform(cfg, store),
            Some(Box::new(InputSetup::platform())),
        ),
        Backends::Custom { local, input } => (Agent::with_local_steps(cfg, store, local), input),
    };
    let agent = Arc::new(agent);
    agent.set_config_file(paths.config.clone());
    if platform {
        start_monitors(&agent);
    }
    // Los entornos se escanean en segundo plano: pueden tardar (WSL arranca su distro).
    agent.rescan();
    let network = Network::start(agent.clone(), &identity, trust, net)?;
    let input =
        input.map(|setup| crate::input::start(agent.clone(), network.active_link(), *setup));
    Ok(Daemon {
        agent,
        network,
        input,
    })
}

/// Los monitores con el bus DDC del sistema (`dxva2` en Windows, `IOAVService` en macOS). Sin
/// backend el módulo queda «no disponible» con su razón.
fn start_monitors(agent: &Arc<Agent>) {
    match monitor_ddc::system_bus() {
        Ok(bus) => {
            let core = tandem_monitor::LocalMonitors::new(
                bus,
                Arc::new(monitor_ddc::SystemClock::new()),
                tandem_monitor::local::system_wall(),
                tandem_monitor::LocalConfig::default(),
            );
            // La tarea vive lo que el agente: termina sola al soltarlo.
            drop(agent.start_monitors(core, crate::monitors::MonitorTimes::default(), false));
        }
        Err(e) => tracing::info!("monitores: {e}"),
    }
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
            Some(State::initial(tandem_core::Machine::A))
        );
    }

    #[test]
    fn the_side_comes_from_the_keys_of_the_trusted_peer() {
        let dir = tempfile::tempdir().unwrap();
        let (low, high) = {
            let (x, y) = (Identity::from_seed([1; 32]), Identity::from_seed([2; 32]));
            if x.public_key().as_bytes() < y.public_key().as_bytes() {
                (x, y)
            } else {
                (y, x)
            }
        };
        let mut trust = TrustStore::open(dir.path().join("trust.toml")).unwrap();
        assert_eq!(resolve_side(&low.public_key(), &trust), None, "sin par");
        trust.add(tandem_link::identity::TrustedPeer {
            public_key: high.public_key(),
            name: "otra".into(),
        });
        assert_eq!(resolve_side(&low.public_key(), &trust), Some(Machine::A));
        let mut theirs = TrustStore::open(dir.path().join("trust-2.toml")).unwrap();
        theirs.add(tandem_link::identity::TrustedPeer {
            public_key: low.public_key(),
            name: "otra".into(),
        });
        assert_eq!(resolve_side(&high.public_key(), &theirs), Some(Machine::B));
    }

    /// Un agente que arranca con un par cuya clave es menor que la suya se queda con el
    /// lado B aunque el archivo dijera A, y lo reescribe.
    #[tokio::test]
    async fn starting_with_a_lower_peer_makes_this_machine_b() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config: dir.path().join("tandem.toml"),
            data_dir: dir.path().to_owned(),
        };
        let (low, high) = {
            let (x, y) = (Identity::from_seed([3; 32]), Identity::from_seed([4; 32]));
            if x.public_key().as_bytes() < y.public_key().as_bytes() {
                (x, y)
            } else {
                (y, x)
            }
        };
        high.save(&paths.identity_file()).unwrap();
        let mut trust = TrustStore::open(paths.trust_file()).unwrap();
        trust
            .add_and_save(tandem_link::identity::TrustedPeer {
                public_key: low.public_key(),
                name: "otra".into(),
            })
            .unwrap();
        let text = "machine = \"a\"\n[input]\nkeyboard = \"a\"\n";
        std::fs::write(&paths.config, text).unwrap();
        let cfg = AgentConfig::from_toml(text).unwrap();
        let net = NetOptions {
            bind: Some("127.0.0.1:0".parse().unwrap()),
            mdns: false,
            fallback_ports: Vec::new(),
            ..NetOptions::default()
        };
        let backends = Backends::Custom {
            local: Arc::new(crate::orchestrator::Unimplemented),
            input: None,
        };
        let d = start(cfg, &paths, backends, net).unwrap();
        assert_eq!(d.agent.machine(), Machine::B);
        assert_eq!(
            d.agent.keyboard_declared(),
            Some(Machine::B),
            "el teclado era de esta"
        );
        let saved = AgentConfig::load(&paths.config).unwrap();
        assert_eq!(saved.machine, Machine::B);
        assert_eq!(saved.input.keyboard, Some(Machine::B));
        d.network.shutdown().await;
    }
}
