//! El puesto completo: dos `tandemd` reales en el mismo proceso, unidos por la red
//! simulada, con los backends simulados y los fallos de un [`FaultPlan`].

use std::sync::Arc;
use std::time::Duration;

use tandem_core::plan::PlanError;
use tandem_core::{Config, Machine, Mode, Plan, State};
use tandem_link::manager::{HealthConfig, LinkKind, LinkManager};
use tandem_proto::{Reply, TxnEvent, UiCommand};
use tandemd::{Agent, AgentConfig, NoPeer, StateStore};
use tokio::sync::{broadcast, watch};

use crate::fault::{FaultPlan, Faults};
use crate::net::{ManagedPeer, SimNet, SimProber};
use crate::steps::{HardwareTimes, SimSteps};
use crate::world::World;

/// El puesto real: 34" (LC34G55T, con su perfil DDC) a la izquierda y 32" a la
/// derecha; DisplayPort a la PC y HDMI a la Mac.
pub const DESK_TOML: &str = r#"
[[monitors]]
id = "m34"
label = '34"'
edid_model = "LC34G55T"
wiring = { pc = "displayport", mac = "hdmi" }
ddc = { hdmi = 0x06, display_port = 0x0F }

[[monitors]]
id = "m32"
label = '32"'
wiring = { pc = "displayport", mac = "hdmi" }
"#;

/// Tope de espera para cualquier cosa de la simulación, en tiempo de tokio (pausado).
pub const SIM_LIMIT: Duration = Duration::from_secs(3600);

/// Tiempo para que lo que queda en el aire termine (apagado, cambio de enlace, avisos).
pub const SETTLE: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum SimError {
    #[error("no se pudo armar la simulación: {0}")]
    Setup(String),
    #[error("el agente rechazó el pedido: {0}")]
    Rejected(String),
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error("se superó el tiempo máximo esperando {0}")]
    Timeout(&'static str),
    #[error("el puesto no coincide con lo esperado:\n  {}", .0.join("\n  "))]
    Mismatch(Vec<String>),
}

/// Cómo terminó una transacción.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Committed,
    RolledBack,
    Inconsistent,
}

/// Los avisos de una transacción, de `Started` al desenlace.
#[derive(Debug, Clone)]
pub struct TxnRun {
    pub events: Vec<TxnEvent>,
}

impl TxnRun {
    pub fn outcome(&self) -> Option<Outcome> {
        match self.events.last()? {
            TxnEvent::Committed { .. } => Some(Outcome::Committed),
            TxnEvent::RolledBack { .. } => Some(Outcome::RolledBack),
            TxnEvent::Inconsistent { .. } => Some(Outcome::Inconsistent),
            _ => None,
        }
    }

    /// Índice y motivo del paso que falló, si alguno.
    pub fn failure(&self) -> Option<(u32, &str)> {
        self.events.iter().find_map(|e| match e {
            TxnEvent::StepFailed { index, error, .. } => Some((*index, error.as_str())),
            _ => None,
        })
    }
}

fn is_end(e: &TxnEvent) -> Option<u64> {
    match e {
        TxnEvent::Committed { txn, .. }
        | TxnEvent::RolledBack { txn, .. }
        | TxnEvent::Inconsistent { txn, .. } => Some(*txn),
        _ => None,
    }
}

/// Opciones del puesto.
#[derive(Debug, Clone)]
pub struct DeskBuilder {
    links: Vec<LinkKind>,
    times: HardwareTimes,
}

impl Default for DeskBuilder {
    fn default() -> Self {
        DeskBuilder {
            links: LinkKind::ALL.to_vec(),
            times: HardwareTimes::default(),
        }
    }
}

impl DeskBuilder {
    /// Los enlaces cableados (por omisión los tres).
    pub fn links(mut self, links: &[LinkKind]) -> Self {
        self.links = links.to_vec();
        self
    }

    pub fn times(mut self, times: HardwareTimes) -> Self {
        self.times = times;
        self
    }

    /// Arranca los dos agentes con todo en la Mac y la PC apagada. Llamar dentro de
    /// un runtime de tokio de un hilo, idealmente con el reloj pausado.
    pub async fn build(self) -> Result<Desk, SimError> {
        let dir = tempfile::tempdir().map_err(|e| SimError::Setup(e.to_string()))?;
        let agent_cfg = |machine: Machine| {
            AgentConfig::from_toml(&format!("machine = \"{machine}\"\n{DESK_TOML}"))
                .map_err(|e| SimError::Setup(e.to_string()))
        };
        let (mac_cfg, pc_cfg) = (agent_cfg(Machine::Mac)?, agent_cfg(Machine::Pc)?);
        let cfg = mac_cfg.desk.clone();
        let world = World::new(&cfg);
        let net = SimNet::new(world.clone(), &self.links);
        let faults = Faults::default();

        let (mac_up_tx, mac_up) = watch::channel(false);
        let (pc_up_tx, pc_up) = watch::channel(false);
        let agent = |agent_cfg: AgentConfig, up| {
            let machine = agent_cfg.machine;
            let steps = SimSteps::new(
                machine,
                cfg.clone(),
                world.clone(),
                net.clone(),
                faults.clone(),
                self.times,
                up,
            );
            let store = StateStore::new(dir.path().join(format!("{machine}.json")));
            Arc::new(Agent::with_local_steps(agent_cfg, store, Arc::new(steps)))
        };
        let mac = agent(mac_cfg, mac_up);
        let pc = agent(pc_cfg, pc_up);

        // Cada agente con su gestor de enlaces real, que pinguea al otro por cada cable.
        let links = |far: &Arc<Agent>, up: watch::Sender<bool>| -> Result<_, SimError> {
            let (manager, _events) = LinkManager::spawn(
                &net.candidates(),
                SimProber::new(net.clone(), far.clone()),
                HealthConfig::default(),
            )
            .map_err(|e| SimError::Setup(e.to_string()))?;
            let mut snapshots = manager.watch();
            tokio::spawn(async move {
                loop {
                    let active = snapshots.borrow_and_update().active.is_some();
                    up.send_replace(active);
                    if snapshots.changed().await.is_err() {
                        return;
                    }
                }
            });
            Ok(Arc::new(ManagedPeer::new(manager)))
        };
        let mac_links = links(&pc, mac_up_tx)?;
        let pc_links = links(&mac, pc_up_tx)?;
        mac.set_peer(mac_links.clone());
        pc.set_peer(pc_links.clone());

        Ok(Desk {
            mac,
            pc,
            world,
            net,
            mac_links,
            pc_links,
            faults,
            cfg,
            _dir: dir,
        })
    }
}

/// Dos agentes en el mismo proceso sobre el puesto simulado.
pub struct Desk {
    pub mac: Arc<Agent>,
    pub pc: Arc<Agent>,
    pub world: World,
    pub net: SimNet,
    mac_links: Arc<ManagedPeer>,
    pc_links: Arc<ManagedPeer>,
    faults: Faults,
    cfg: Config,
    _dir: tempfile::TempDir,
}

impl Drop for Desk {
    fn drop(&mut self) {
        // Rompe el ciclo agente → gestor de enlaces → agente del otro lado.
        self.mac.set_peer(Arc::new(NoPeer));
        self.pc.set_peer(Arc::new(NoPeer));
    }
}

async fn within<F: Future>(what: &'static str, f: F) -> Result<F::Output, SimError> {
    tokio::time::timeout(SIM_LIMIT, f)
        .await
        .map_err(|_| SimError::Timeout(what))
}

impl Desk {
    pub fn builder() -> DeskBuilder {
        DeskBuilder::default()
    }

    /// El puesto con las opciones por omisión: tres enlaces, todo en la Mac, PC apagada.
    pub async fn new() -> Result<Desk, SimError> {
        DeskBuilder::default().build().await
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn agent(&self, machine: Machine) -> &Arc<Agent> {
        match machine {
            Machine::Mac => &self.mac,
            Machine::Pc => &self.pc,
        }
    }

    /// El enlace por el que `machine` habla ahora con el otro agente.
    pub fn active_link(&self, machine: Machine) -> Option<LinkKind> {
        match machine {
            Machine::Mac => self.mac_links.active_link(),
            Machine::Pc => self.pc_links.active_link(),
        }
    }

    /// Reemplaza los fallos pendientes y programa las caídas de enlace del plan.
    pub fn arm(&self, plan: FaultPlan) {
        self.faults.arm(plan.faults);
        for (after, kind) in plan.cuts {
            let net = self.net.clone();
            tokio::spawn(async move {
                tokio::time::sleep(after).await;
                net.cut(kind);
            });
        }
    }

    /// Pide un modo por la misma vía que la UI y espera el desenlace.
    pub async fn set_mode(&self, on: Machine, mode: Mode) -> Result<TxnRun, SimError> {
        let agent = self.agent(on);
        let mut rx = agent.subscribe_txns();
        let txn = match agent.handle(UiCommand::SetMode(mode)) {
            Reply::Accepted { txn } => txn,
            Reply::Rejected(why) => return Err(SimError::Rejected(why)),
            other => return Err(SimError::Rejected(format!("{other:?}"))),
        };
        let mut events = Vec::new();
        loop {
            let e = match within("el desenlace del cambio de modo", rx.recv()).await? {
                Ok(e) => e,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    return Err(SimError::Setup(format!("se perdieron {n} avisos")));
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(SimError::Setup("el orquestador se cerró".into()));
                }
            };
            let end = is_end(&e) == Some(txn);
            events.push(e);
            if end {
                return Ok(TxnRun { events });
            }
        }
    }

    /// Apagado de la PC orquestado por la Mac (DESIGN §5): todo a la Mac y después apagar.
    ///
    /// La UI todavía no tiene un pedido para esto, así que se arma el plan y se corre en
    /// el orquestador como lo haría el agente.
    pub async fn shutdown_pc(&self) -> Result<TxnRun, SimError> {
        let agent = &self.mac;
        let orch = agent.orchestrator();
        let mut rx = agent.subscribe_txns();
        let txn = orch
            .try_begin()
            .ok_or_else(|| SimError::Rejected(tandemd::agent::BUSY.into()))?;
        let mut state = agent.state();
        let plan = match Plan::shutdown_pc(&self.cfg, &state) {
            Ok(p) => p,
            Err(e) => {
                orch.end();
                return Err(e.into());
            }
        };
        let outcome = within("el apagado", orch.run(txn, &self.cfg, &mut state, &plan)).await;
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                orch.end();
                return Err(e);
            }
        };
        if let Err(e) = agent.set_state(state.clone()) {
            tracing::warn!("{e}");
        }
        orch.end();
        orch.finish(txn, &state, &outcome);
        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        Ok(TxnRun { events })
    }

    /// Deja pasar el tiempo para que termine lo que quedó en el aire.
    pub async fn settle(&self) {
        tokio::time::sleep(SETTLE).await;
    }

    /// El puesto físico coincide con lo que cree la Mac, y nada imposible ocurrió.
    pub async fn check(&self) -> Result<(), SimError> {
        self.settle().await;
        self.world
            .check(&self.mac.state())
            .map_err(SimError::Mismatch)
    }

    /// Cada motor de entrada usa las pantallas que su máquina muestra de verdad.
    pub fn check_geometry(&self) -> Result<(), SimError> {
        self.world.check_geometry().map_err(SimError::Mismatch)
    }

    /// Espera a que el agente de la PC tenga el mismo estado que el de la Mac.
    pub async fn pc_agrees(&self) -> Result<State, SimError> {
        let want = self.mac.state();
        let mut rx = self.pc.subscribe();
        within("que la PC reciba el estado", rx.wait_for(|s| *s == want))
            .await?
            .map_err(|_| SimError::Setup("el agente de la PC se cerró".into()))?;
        Ok(want)
    }

    /// Espera a que la Mac vea al agente de la PC por algún enlace.
    pub async fn wait_link(&self, kind: LinkKind) -> Result<(), SimError> {
        let mut rx = self.mac_links.manager().watch();
        within(
            "el enlace",
            rx.wait_for(|s| s.active.as_ref().is_some_and(|a| a.kind == kind)),
        )
        .await?
        .map(|_| ())
        .map_err(|_| SimError::Setup("el gestor de enlaces se cerró".into()))
    }
}
