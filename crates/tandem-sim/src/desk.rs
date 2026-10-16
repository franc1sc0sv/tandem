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
use crate::world::{Power, World};

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
    principal: Machine,
}

impl Default for DeskBuilder {
    fn default() -> Self {
        DeskBuilder {
            links: LinkKind::ALL.to_vec(),
            times: HardwareTimes::default(),
            principal: Machine::Mac,
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

    /// La máquina que el usuario está usando al empezar: tiene todo y la otra está
    /// apagada (por omisión la Mac; con la PC, los escenarios espejo de DESIGN §3.7).
    pub fn principal(mut self, principal: Machine) -> Self {
        self.principal = principal;
        self
    }

    /// Arranca los dos agentes con todo en la principal y la otra apagada. Llamar dentro
    /// de un runtime de tokio de un hilo, idealmente con el reloj pausado.
    pub async fn build(self) -> Result<Desk, SimError> {
        let dir = tempfile::tempdir().map_err(|e| SimError::Setup(e.to_string()))?;
        let agent_cfg = |machine: Machine| {
            AgentConfig::from_toml(&format!("machine = \"{machine}\"\n{DESK_TOML}"))
                .map_err(|e| SimError::Setup(e.to_string()))
        };
        let (mac_cfg, pc_cfg) = (agent_cfg(Machine::Mac)?, agent_cfg(Machine::Pc)?);
        let cfg = mac_cfg.desk.clone();
        let world = World::with_principal(&cfg, self.principal);
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
        // La principal sabe lo que muestra cada monitor (todo es suyo); la otra arranca
        // con lo que crea y se pone al día con el primer estado que le llegue.
        let principal = match self.principal {
            Machine::Mac => &mac,
            Machine::Pc => &pc,
        };
        let mut start = principal.state();
        start.owner.values_mut().for_each(|o| *o = self.principal);
        if let Err(e) = principal.set_state(start) {
            return Err(SimError::Setup(e.to_string()));
        }

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
        // Cada una concilia el encendido de la otra con lo que ve su gestor de enlaces.
        let (mac_reach, pc_reach) = (mac_up_tx.subscribe(), pc_up_tx.subscribe());
        let mac_links = links(&pc, mac_up_tx)?;
        let pc_links = links(&mac, pc_up_tx)?;
        mac.set_peer(mac_links.clone());
        pc.set_peer(pc_links.clone());
        let watches = [
            tokio::spawn(watch_while_on(world.clone(), mac.clone(), mac_reach)),
            tokio::spawn(watch_while_on(world.clone(), pc.clone(), pc_reach)),
        ];

        Ok(Desk {
            mac,
            pc,
            world,
            net,
            mac_links,
            pc_links,
            faults,
            cfg,
            times: self.times,
            principal: self.principal,
            watches,
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
    times: HardwareTimes,
    principal: Machine,
    watches: [tokio::task::JoinHandle<()>; 2],
    _dir: tempfile::TempDir,
}

impl Drop for Desk {
    fn drop(&mut self) {
        for w in &self.watches {
            w.abort();
        }
        // Rompe el ciclo agente → gestor de enlaces → agente del otro lado.
        self.mac.set_peer(Arc::new(NoPeer));
        self.pc.set_peer(Arc::new(NoPeer));
    }
}

/// La vigilancia del par de `agent` mientras su máquina está prendida: una apagada no
/// corre nada, y al volver arranca de nuevo con lo que tenía guardado.
async fn watch_while_on(world: World, agent: Arc<Agent>, reach: watch::Receiver<bool>) {
    let me = agent.machine();
    let mut power = world.subscribe_power();
    loop {
        if power
            .wait_for(|p| p.get(&me) == Some(&Power::On))
            .await
            .is_err()
        {
            return;
        }
        let task = tokio::spawn(tandemd::peer_watch::watch(agent.clone(), reach.clone()));
        let off = power.wait_for(|p| p.get(&me) != Some(&Power::On)).await;
        task.abort();
        if off.is_err() {
            return;
        }
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

    /// La máquina que el usuario usaba al empezar.
    pub fn principal(&self) -> Machine {
        self.principal
    }

    /// Apagado de la PC orquestado por la Mac (DESIGN §5): todo a la Mac y después apagar.
    pub async fn shutdown_pc(&self) -> Result<TxnRun, SimError> {
        self.shutdown(Machine::Mac, Machine::Pc).await
    }

    /// Apagado limpio de `machine` orquestado por `on`: todo a la otra y después apagar.
    ///
    /// Se arma el plan y se corre en el orquestador como lo haría el agente con
    /// [`UiCommand::SetPower`], para tener los avisos de la transacción.
    pub async fn shutdown(&self, on: Machine, machine: Machine) -> Result<TxnRun, SimError> {
        let agent = self.agent(on);
        let orch = agent.orchestrator();
        let mut rx = agent.subscribe_txns();
        let txn = orch
            .try_begin()
            .ok_or_else(|| SimError::Rejected(tandemd::agent::BUSY.into()))?;
        let mut state = agent.state();
        let plan = match Plan::power_off(&self.cfg, &state, machine, &agent.abilities()) {
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

    /// La PC se queda sin corriente de golpe.
    pub fn pull_pc_plug(&self) {
        self.pull_plug(Machine::Pc);
    }

    /// `machine` se queda sin corriente (o se suspende) de golpe, sin que ningún agente
    /// lo pida: los monitores en su entrada quedan en negro y su agente deja de contestar.
    pub fn pull_plug(&self, machine: Machine) {
        self.world.lock().power_off(machine);
        self.net.drop_all();
    }

    /// Alguien enciende la PC con su botón, sin Wake-on-LAN.
    pub fn press_pc_power_button(&self) {
        self.press_power_button(Machine::Pc);
    }

    /// Alguien enciende (o despierta) `machine` con su botón, sin Wake-on-LAN.
    pub fn press_power_button(&self, machine: Machine) {
        {
            let mut w = self.world.lock();
            if w.power_of(machine) != Power::Off {
                return;
            }
            w.set_power(machine, Power::Booting);
        }
        let (world, boot) = (self.world.clone(), self.times.boot_of(machine));
        tokio::spawn(async move {
            tokio::time::sleep(boot).await;
            world.lock().boot_done(machine);
        });
    }

    /// Deja pasar el tiempo para que termine lo que quedó en el aire.
    pub async fn settle(&self) {
        tokio::time::sleep(SETTLE).await;
    }

    /// El puesto físico coincide con lo que cree la principal, y nada imposible ocurrió.
    pub async fn check(&self) -> Result<(), SimError> {
        self.check_from(self.principal).await
    }

    /// El puesto físico coincide con lo que cree `machine`.
    pub async fn check_from(&self, machine: Machine) -> Result<(), SimError> {
        self.settle().await;
        self.world
            .check(&self.agent(machine).state())
            .map_err(SimError::Mismatch)
    }

    /// Cada motor de entrada usa las pantallas que su máquina muestra de verdad.
    pub fn check_geometry(&self) -> Result<(), SimError> {
        self.world.check_geometry().map_err(SimError::Mismatch)
    }

    /// Espera a que el agente de la PC tenga el mismo estado que el de la Mac.
    pub async fn pc_agrees(&self) -> Result<State, SimError> {
        self.agrees(Machine::Pc).await
    }

    /// Espera a que el agente de `machine` tenga el mismo estado que el de la otra.
    pub async fn agrees(&self, machine: Machine) -> Result<State, SimError> {
        let want = self.agent(machine.other()).state();
        let mut rx = self.agent(machine).subscribe();
        within(
            "que la otra máquina reciba el estado",
            rx.wait_for(|s| *s == want),
        )
        .await?
        .map_err(|_| SimError::Setup("el agente se cerró".into()))?;
        Ok(want)
    }

    /// Espera a que la principal vea al agente de la otra por algún enlace.
    pub async fn wait_link(&self, kind: LinkKind) -> Result<(), SimError> {
        let links = match self.principal {
            Machine::Mac => &self.mac_links,
            Machine::Pc => &self.pc_links,
        };
        let mut rx = links.manager().watch();
        within(
            "el enlace",
            rx.wait_for(|s| s.active.as_ref().is_some_and(|a| a.kind == kind)),
        )
        .await?
        .map(|_| ())
        .map_err(|_| SimError::Setup("el gestor de enlaces se cerró".into()))
    }
}
