//! El puesto completo: dos `tandemd` reales en el mismo proceso, unidos por la red
//! simulada, con los backends simulados y los fallos de un [`FaultPlan`].

use std::sync::Arc;
use std::time::Duration;

use tandem_core::{Machine, MonitorInput, Os, Plan, State};
use tandem_link::iface::{FakeInterfaces, InterfaceKind, NetInterface, follow_wired};
use tandem_link::manager::{HealthConfig, LinkKind, LinkManager};
use tandem_link::metrics;
use tandem_monitor::{LocalConfig, LocalMonitors};
use tandem_proto::{MonitorView, Reply, TxnEvent, UiCommand};
use tandemd::monitors::MonitorTimes;
use tandemd::{Agent, AgentConfig, NoPeer, StateStore};
use tokio::sync::{broadcast, watch};

use crate::fault::{FaultPlan, Faults};
use crate::monitors::{MonitorSpec, SimMonitors};
use crate::net::{ManagedPeer, SimNet, SimProber};
use crate::scenes::SceneScreens;
use crate::steps::{HardwareTimes, SimSteps};
use crate::world::{Power, World};

/// La tarjeta de red a la que va el Wake-on-LAN (simulado) de cada agente.
const WOL_MAC: &str = "9c:6b:00:1a:2b:ff";

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
    #[error("se superó el tiempo máximo esperando {0}")]
    Timeout(&'static str),
    #[error("el puesto no coincide con lo esperado:\n  {}", .0.join("\n  "))]
    Mismatch(Vec<String>),
}

/// Cómo terminó una transacción.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Committed,
    Failed,
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
            TxnEvent::Failed { .. } => Some(Outcome::Failed),
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
        TxnEvent::Committed { txn, .. } | TxnEvent::Failed { txn, .. } => Some(*txn),
        _ => None,
    }
}

/// Opciones del puesto.
#[derive(Debug, Clone)]
pub struct DeskBuilder {
    links: Vec<LinkKind>,
    times: HardwareTimes,
    principal: Machine,
    /// El sistema de A y el de B. Cualquier combinación vale (DECISIONS D2).
    os: [Os; 2],
    /// Los monitores del puesto y quién tiene cable a cada uno (T-110).
    monitors: Vec<MonitorSpec>,
    scene_screens: SceneScreens,
}

impl Default for DeskBuilder {
    fn default() -> Self {
        DeskBuilder {
            links: LinkKind::ALL.to_vec(),
            times: HardwareTimes::default(),
            principal: Machine::A,
            os: [Os::MacOs, Os::Windows],
            monitors: Vec::new(),
            scene_screens: SceneScreens::default(),
        }
    }
}

/// Cada cuánto miran el bus los agentes de la simulación.
pub const SIM_MONITOR_TIMES: MonitorTimes = MonitorTimes {
    scan_every: Duration::from_secs(2),
    poll_every: Duration::from_millis(500),
};

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

    /// La máquina que el usuario está usando al empezar: la otra está apagada (por
    /// omisión la Mac; con la PC, los escenarios espejo de DESIGN §3.7).
    pub fn principal(mut self, principal: Machine) -> Self {
        self.principal = principal;
        self
    }

    /// El sistema de cada lado (por omisión A con macOS y B con Windows).
    pub fn os(mut self, a: Os, b: Os) -> Self {
        self.os = [a, b];
        self
    }

    /// Monitores conectados a las máquinas (por omisión, ninguno: el módulo queda sin canal).
    /// Los monitores de escena (el doble del paso abstracto de T-100) y qué muestra cada uno
    /// al empezar.
    pub fn scene_monitors(mut self, monitors: &[(&str, Machine)]) -> Self {
        self.scene_screens = SceneScreens::new(monitors);
        self
    }

    pub fn monitors(mut self, specs: Vec<MonitorSpec>) -> Self {
        self.monitors = specs;
        self
    }

    /// Arranca los dos agentes con la principal prendida y la otra apagada. Llamar dentro
    /// de un runtime de tokio de un hilo, idealmente con el reloj pausado.
    pub async fn build(self) -> Result<Desk, SimError> {
        let dir = tempfile::tempdir().map_err(|e| SimError::Setup(e.to_string()))?;
        // Las dos con `[power.wol]` para despertar a la otra: el Wake-on-LAN es simulado.
        let os = self.os;
        let agent_cfg = |machine: Machine| {
            let mut cfg = AgentConfig::from_toml(&format!(
                "machine = \"{}\"\n[power.wol]\nmac = \"{WOL_MAC}\"\n",
                machine.as_str()
            ))
            .map_err(|e| SimError::Setup(e.to_string()))?;
            cfg.os = match machine {
                Machine::A => os[0],
                Machine::B => os[1],
            };
            Ok::<_, SimError>(cfg)
        };
        let (mac_cfg, pc_cfg) = (agent_cfg(Machine::A)?, agent_cfg(Machine::B)?);
        let world = World::with_principal(self.principal);
        let net = SimNet::new(world.clone(), &self.links);
        let faults = Faults::default();

        let (mac_up_tx, mac_up) = watch::channel(false);
        let (pc_up_tx, pc_up) = watch::channel(false);
        let agent = |agent_cfg: AgentConfig, up| {
            let machine = agent_cfg.machine;
            let steps = SimSteps::new(
                machine,
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
        // Como la red real: cada vez que aparece un enlace, el agente vuelve a presentarse
        // (su puesto y sus capacidades) por él.
        let links =
            |near: &Arc<Agent>, far: &Arc<Agent>, up: watch::Sender<bool>| -> Result<_, SimError> {
                let (manager, events) = LinkManager::spawn(
                    &net.candidates(),
                    SimProber::new(net.clone(), far.clone()),
                    HealthConfig::default(),
                )
                .map_err(|e| SimError::Setup(e.to_string()))?;
                // Las métricas de la pantalla Red, como en `tandemd::network` (T-102).
                tokio::spawn(metrics::follow(
                    near.net_metrics().clone(),
                    manager.watch(),
                    events,
                    near.link_qos().clone(),
                    metrics::SAMPLE_EVERY,
                ));
                let mut snapshots = manager.watch();
                let peer = Arc::new(ManagedPeer::new(manager));
                let (near, announce) = (near.clone(), peer.clone());
                tokio::spawn(async move {
                    let mut was = false;
                    loop {
                        let active = snapshots.borrow_and_update().active.is_some();
                        up.send_replace(active);
                        if active && !was {
                            near.set_peer(announce.clone());
                        }
                        was = active;
                        if snapshots.changed().await.is_err() {
                            return;
                        }
                    }
                });
                Ok(peer)
            };
        // Cada una concilia el encendido de la otra con lo que ve su gestor de enlaces.
        let (mac_reach, pc_reach) = (mac_up_tx.subscribe(), pc_up_tx.subscribe());
        let mac_links = links(&mac, &pc, mac_up_tx)?;
        let pc_links = links(&pc, &mac, pc_up_tx)?;
        mac.set_peer(mac_links.clone());
        pc.set_peer(pc_links.clone());
        // Las interfaces de cada máquina: los cables que se enchufan después (T-094) las
        // cambian y el gestor de enlaces las sigue, como en el agente de verdad.
        let cables = [FakeInterfaces::new(vec![]), FakeInterfaces::new(vec![])];
        let follow = |ifaces: &FakeInterfaces, links: &ManagedPeer| {
            follow_wired(ifaces, links.manager().control(), |i: &NetInterface| {
                Some(crate::net::link_addr(i.kind.link_kind()))
            })
        };
        let mut watches = vec![
            follow(&cables[0], &mac_links),
            follow(&cables[1], &pc_links),
            tokio::spawn(watch_while_on(world.clone(), mac.clone(), mac_reach)),
            tokio::spawn(watch_while_on(world.clone(), pc.clone(), pc_reach)),
        ];
        // Con monitores de verdad (los dobles de `monitor-ddc`), las escenas van a ellos.
        if self.monitors.is_empty() {
            for agent in [&mac, &pc] {
                let backend = self.scene_screens.backend(agent.machine(), faults.clone());
                agent.set_scene_backend(Arc::new(backend));
            }
        }
        let monitors = (!self.monitors.is_empty()).then(|| SimMonitors::new(&self.monitors));
        if let Some(sim) = &monitors {
            for agent in [&mac, &pc] {
                let clock = sim.clock();
                let wall = clock.clone();
                let core = LocalMonitors::new(
                    Arc::new(sim.bus_of(agent.machine(), world.clone())),
                    Arc::new(clock),
                    // Una hora fija más el reloj del doble: las horas de los eventos son exactas.
                    Arc::new(move || {
                        use monitor_ddc::Clock;
                        1_763_000_000_000 + u64::try_from(wall.now().as_millis()).unwrap_or(0)
                    }),
                    LocalConfig::default(),
                );
                // El bus falso no bloquea: se llama en el lugar, con el reloj pausado quieto.
                watches.push(agent.start_monitors(core, SIM_MONITOR_TIMES, true));
            }
        }

        Ok(Desk {
            mac,
            pc,
            world,
            net,
            cables,
            mac_links,
            pc_links,
            faults,
            times: self.times,
            principal: self.principal,
            watches,
            monitors,
            scene_screens: self.scene_screens,
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
    cables: [FakeInterfaces; 2],
    mac_links: Arc<ManagedPeer>,
    pc_links: Arc<ManagedPeer>,
    faults: Faults,
    times: HardwareTimes,
    principal: Machine,
    watches: Vec<tokio::task::JoinHandle<()>>,
    monitors: Option<SimMonitors>,
    scene_screens: SceneScreens,
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

/// La interfaz que ve cada máquina cuando el cable de `kind` está puesto.
fn cable_interface(kind: LinkKind) -> NetInterface {
    let (name, iface_kind) = match kind {
        LinkKind::Usb4 => ("USB4 P2P Network Adapter", InterfaceKind::Usb4),
        LinkKind::Ethernet => ("Ethernet", InterfaceKind::Ethernet),
        LinkKind::Wifi => ("Wi-Fi", InterfaceKind::Wifi),
    };
    NetInterface {
        index: 100 + u32::from(kind.priority()),
        name: name.into(),
        kind: iface_kind,
        addrs: vec![],
        mac: None,
        speed_mbps: None,
        up: true,
    }
}

impl Desk {
    /// Las interfaces simuladas de una máquina, para enchufar y desenchufar a mano.
    pub fn interfaces(&self, machine: Machine) -> &FakeInterfaces {
        &self.cables[usize::from(machine == Machine::B)]
    }

    /// Enchufa el cable de `kind` entre las dos máquinas: el enlace existe y la interfaz
    /// aparece en las dos, que se enteran solas.
    pub fn plug_cable(&self, kind: LinkKind) {
        self.net.plug(kind);
        for ifaces in &self.cables {
            ifaces.plug(cable_interface(kind));
        }
    }

    /// Desconecta el cable de `kind`: la interfaz baja en las dos máquinas y nada pasa por él.
    pub fn unplug_cable(&self, kind: LinkKind) {
        let index = cable_interface(kind).index;
        for ifaces in &self.cables {
            ifaces.unplug(index);
        }
        self.net.cut(kind);
    }

    pub fn builder() -> DeskBuilder {
        DeskBuilder::default()
    }

    /// El puesto con las opciones por omisión: tres enlaces, la Mac prendida, la PC apagada.
    pub async fn new() -> Result<Desk, SimError> {
        DeskBuilder::default().build().await
    }

    pub fn agent(&self, machine: Machine) -> &Arc<Agent> {
        match machine {
            Machine::A => &self.mac,
            Machine::B => &self.pc,
        }
    }

    /// El enlace por el que `machine` habla ahora con el otro agente.
    pub fn active_link(&self, machine: Machine) -> Option<LinkKind> {
        match machine {
            Machine::A => self.mac_links.active_link(),
            Machine::B => self.pc_links.active_link(),
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

    /// Encender (o apagar) `machine` desde la UI de `on`, por la misma vía que la UI, y
    /// esperar el desenlace.
    pub async fn set_power(
        &self,
        on: Machine,
        machine: Machine,
        up: bool,
    ) -> Result<TxnRun, SimError> {
        self.request(on, UiCommand::SetPower { machine, on: up })
            .await
    }

    /// Aplicar la escena `id` desde la UI de `on` y esperar el desenlace.
    pub async fn apply_scene(&self, on: Machine, id: &str) -> Result<TxnRun, SimError> {
        self.request(on, UiCommand::ApplyScene { id: id.to_owned() })
            .await
    }

    /// Los monitores de escena ([`DeskBuilder::scene_monitors`]).
    pub fn scene_screens(&self) -> &SceneScreens {
        &self.scene_screens
    }

    /// Encender `machine` desde la otra.
    pub async fn power_on(&self, machine: Machine) -> Result<TxnRun, SimError> {
        self.set_power(machine.other(), machine, true).await
    }

    /// Un pedido de la UI que arranca una transacción; espera su desenlace.
    async fn request(&self, on: Machine, cmd: UiCommand) -> Result<TxnRun, SimError> {
        let agent = self.agent(on);
        let mut rx = agent.subscribe_txns();
        let txn = match agent.handle(cmd) {
            Reply::Accepted { txn } => txn,
            Reply::Rejected(why) => return Err(SimError::Rejected(why)),
            other => return Err(SimError::Rejected(format!("{other:?}"))),
        };
        let mut events = Vec::new();
        loop {
            let e = match within("el desenlace de la transacción", rx.recv()).await? {
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

    /// Los monitores físicos, si el puesto tiene ([`DeskBuilder::monitors`]).
    pub fn monitors(&self) -> Option<&SimMonitors> {
        self.monitors.as_ref()
    }

    /// Cambiar la entrada de un monitor desde la UI de `on` y esperar el desenlace.
    pub async fn switch_monitor(
        &self,
        on: Machine,
        monitor: &str,
        input: MonitorInput,
    ) -> Result<TxnRun, SimError> {
        let cmd = UiCommand::SwitchMonitor {
            monitor: monitor.to_owned(),
            input,
        };
        self.request(on, cmd).await
    }

    /// Espera a que los monitores que ve `machine` cumplan `ok`; devuelve esa vista.
    pub async fn wait_monitors(
        &self,
        machine: Machine,
        what: &'static str,
        ok: impl Fn(&[MonitorView]) -> bool,
    ) -> Result<Vec<MonitorView>, SimError> {
        let mut rx = self.agent(machine).subscribe_monitors();
        let views = within(what, rx.wait_for(|v| ok(v)))
            .await?
            .map_err(|_| SimError::Setup("el agente se cerró".into()))?
            .clone();
        Ok(views)
    }

    /// Apagado de la PC orquestado por la Mac (DESIGN §5).
    pub async fn shutdown_pc(&self) -> Result<TxnRun, SimError> {
        self.shutdown(Machine::A, Machine::B).await
    }

    /// Apagado limpio de `machine` orquestado por `on`.
    ///
    /// Se arma el plan y se corre en el orquestador como lo haría el agente con
    /// [`UiCommand::SetPower`], para tener los avisos de la transacción aunque la máquina
    /// que orquesta sea la que se apaga.
    pub async fn shutdown(&self, on: Machine, machine: Machine) -> Result<TxnRun, SimError> {
        let agent = self.agent(on);
        let orch = agent.orchestrator();
        let mut rx = agent.subscribe_txns();
        let txn = orch
            .try_begin()
            .ok_or_else(|| SimError::Rejected(tandemd::agent::BUSY.into()))?;
        let mut state = agent.state();
        let plan = Plan::power_off(&state, machine);
        let outcome = within("el apagado", orch.run(txn, &mut state, &plan)).await;
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
        self.pull_plug(Machine::B);
    }

    /// `machine` se queda sin corriente (o se suspende) de golpe, sin que ningún agente
    /// lo pida: su agente deja de contestar.
    pub fn pull_plug(&self, machine: Machine) {
        self.world.lock().power_off(machine);
        self.net.drop_all();
    }

    /// Alguien enciende la PC con su botón, sin Wake-on-LAN.
    pub fn press_pc_power_button(&self) {
        self.press_power_button(Machine::B);
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

    /// La energía coincide con lo que cree la principal, y nada imposible ocurrió.
    pub async fn check(&self) -> Result<(), SimError> {
        self.check_from(self.principal).await
    }

    /// La energía coincide con lo que cree `machine`.
    pub async fn check_from(&self, machine: Machine) -> Result<(), SimError> {
        self.settle().await;
        self.world
            .check(&self.agent(machine).state())
            .map_err(SimError::Mismatch)
    }

    /// Espera a que el agente de la PC tenga el mismo estado que el de la Mac.
    pub async fn pc_agrees(&self) -> Result<State, SimError> {
        self.agrees(Machine::B).await
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
            Machine::A => &self.mac_links,
            Machine::B => &self.pc_links,
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
