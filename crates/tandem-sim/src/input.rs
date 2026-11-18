//! Dos `tandemd` con el motor de entrada, emparejados y unidos por QUIC real en loopback,
//! con captura e inyección simuladas ([`FakeInputBackends`]): el banco de latencia de la
//! entrada (T-093) y las pruebas de punta a punta del teclado y el mouse.
//!
//! A diferencia del resto de `tandem-sim`, aquí el reloj es el real: lo que se mide es
//! tiempo de verdad.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tandem_core::{Machine, Os, Step};
use tandem_link::identity::{Identity, TrustStore, TrustedPeer};
use tandem_proto::InputStatus;
use tandemd::daemon::{self, Backends, Daemon};
use tandemd::input::fake::FakeInputBackends;
use tandemd::input::{InputOptions, InputSetup};
use tandemd::ipc::mem;
use tandemd::network::NetOptions;
use tandemd::orchestrator::{LocalSteps, StepFuture};
use tandemd::paths::Paths;
use tandemd::test_ports::{PortTaken, port_taken, two_ports};
use tandemd::{Agent, AgentConfig};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::SimError;

/// Pasos locales que siempre salen bien: el banco no aplica planes.
struct NoSteps;

impl LocalSteps for NoSteps {
    fn execute<'a>(&'a self, _step: &'a Step) -> StepFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

/// Un agente en marcha.
pub struct SimAgent {
    pub agent: Arc<Agent>,
    pub fake: FakeInputBackends,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl SimAgent {
    pub fn machine(&self) -> Machine {
        self.agent.machine()
    }

    pub fn input(&self) -> InputStatus {
        self.agent.input_status()
    }

    /// Espera a que el estado de la entrada cumpla `f`, como mucho `limit`.
    pub async fn until_input(
        &self,
        limit: Duration,
        f: impl Fn(&InputStatus) -> bool,
    ) -> Result<InputStatus, SimError> {
        let mut rx = self.agent.subscribe_input();
        let got = tokio::time::timeout(limit, rx.wait_for(|s| f(s)))
            .await
            .map_err(|_| SimError::Timeout("estado de la entrada"))?
            .map_err(|_| SimError::Timeout("el motor de entrada se detuvo"))?
            .clone();
        Ok(got)
    }

    pub async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for SimAgent {
    fn drop(&mut self) {
        if let Some(t) = &self.task {
            t.abort();
        }
    }
}

/// Las dos máquinas del banco: `a` y `b` según sus claves (DECISIONS A1).
pub struct InputPair {
    pub a: SimAgent,
    pub b: SimAgent,
}

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

impl InputPair {
    /// Arranca las dos con estas pantallas (las que lee cada una) y estos tiempos. El
    /// teclado arranca en `A`; el enlace es «USB4» (en loopback).
    ///
    /// Las identidades son nuevas en cada par: dos bancos que corren a la vez (otra suite,
    /// otro checkout) no se pueden confundir de agente aunque se crucen de puerto. Los
    /// puertos se abren sin reserva y, si otra suite tomó alguno, se eligen otros
    /// ([`tandemd::test_ports`]).
    pub async fn start(
        screens_a: Vec<tandem_input::Screen>,
        screens_b: Vec<tandem_input::Screen>,
        opts: InputOptions,
    ) -> Result<Self, SimError> {
        let setup = |e: &dyn std::fmt::Display| SimError::Setup(e.to_string());
        let (x, y) = (
            Identity::generate().map_err(|e| setup(&e))?,
            Identity::generate().map_err(|e| setup(&e))?,
        );
        let (id_a, id_b) = if x.public_key().as_bytes() < y.public_key().as_bytes() {
            (x, y)
        } else {
            (y, x)
        };
        for _ in 0..tandemd::test_ports::ATTEMPTS {
            let (port_a, port_b) = two_ports().map_err(|e| setup(&e))?;
            let fake_a = FakeInputBackends::new(screens_a.clone());
            let Ok(a) = start_one(Machine::A, &id_a, &id_b, port_a, port_b, fake_a, opts).await?
            else {
                continue;
            };
            let fake_b = FakeInputBackends::new(screens_b.clone());
            match start_one(Machine::B, &id_b, &id_a, port_b, port_a, fake_b, opts).await? {
                Ok(b) => return Ok(InputPair { a, b }),
                Err(PortTaken) => a.stop().await,
            }
        }
        Err(SimError::Setup("no se encontraron puertos libres".into()))
    }

    pub async fn stop(self) {
        self.b.stop().await;
        self.a.stop().await;
    }
}

async fn start_one(
    machine: Machine,
    me: &Identity,
    peer: &Identity,
    port: u16,
    peer_port: u16,
    fake: FakeInputBackends,
    opts: InputOptions,
) -> Result<Result<SimAgent, PortTaken>, SimError> {
    let setup = |e: &dyn std::fmt::Display| SimError::Setup(e.to_string());
    let dir = tempfile::tempdir().map_err(|e| setup(&e))?;
    let paths = Paths {
        config: dir.path().join("tandem.toml"),
        data_dir: dir.path().to_owned(),
    };
    me.save(&paths.identity_file()).map_err(|e| setup(&e))?;
    let mut trust = TrustStore::open(paths.trust_file()).map_err(|e| setup(&e))?;
    trust
        .add_and_save(TrustedPeer {
            public_key: peer.public_key(),
            name: machine.other().to_string(),
        })
        .map_err(|e| setup(&e))?;
    let text = format!(
        "machine = \"{machine}\"\n[network]\nport = {port}\npeers = [{{ kind = \"usb4\", addr = \"{}\" }}]\n[input]\nkeyboard = \"a\"\n",
        loopback(peer_port)
    );
    std::fs::write(&paths.config, text).map_err(|e| setup(&e))?;
    let mut cfg = AgentConfig::load(&paths.config).map_err(|e| setup(&e))?;
    cfg.os = match machine {
        Machine::A => Os::MacOs,
        Machine::B => Os::Windows,
    };
    let started = daemon::start(
        cfg,
        &paths,
        Backends::Custom {
            local: Arc::new(NoSteps),
            input: Some(Box::new(InputSetup {
                backends: Arc::new(fake.clone()),
                opts,
                file_drag: None,
            })),
            env: None,
        },
        NetOptions {
            bind: Some(loopback(port)),
            mdns: false,
            // Sin reserva: un puerto tomado falla enseguida y se eligen otros.
            fallback_ports: Vec::new(),
            ..NetOptions::default()
        },
    );
    let Daemon {
        agent,
        network,
        input,
    } = match port_taken(started).map_err(|e| setup(&e))? {
        Ok(d) => d,
        Err(PortTaken) => return Ok(Err(PortTaken)),
    };
    let (listener, _connector) = mem::pair();
    let (stop, stopped) = oneshot::channel::<()>();
    let daemon = Daemon {
        agent: agent.clone(),
        network,
        input,
    };
    let task = tokio::spawn(async move {
        let _ = daemon
            .serve(listener, async {
                let _ = stopped.await;
            })
            .await;
    });
    Ok(Ok(SimAgent {
        agent,
        fake,
        stop: Some(stop),
        task: Some(task),
        _dir: dir,
    }))
}
