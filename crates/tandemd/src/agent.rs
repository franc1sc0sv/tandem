//! El estado vivo del agente y lo que contesta a la UI.

use std::sync::{Arc, Mutex, PoisonError};

use tandem_core::plan::PlanError;
use tandem_core::{Machine, Mode, Plan, State, Step};
use tandem_display::switcher::{ManualEvent, ManualSwitcher};
use tandem_link::transport::{Connection, ControlStream};
use tandem_proto::{
    Channel, HubReport, InputStatus, PairingStatus, PeerStatus, PowerEvent, ProbeEvent, Reply,
    ScreenRect, TxnEvent, UiCommand,
};
use tokio::sync::{broadcast, mpsc, watch};

use crate::calibration::{DesktopLocator, ProbeBoard};
use crate::config::AgentConfig;
use crate::display::DisplaySteps;
use crate::orchestrator::{LocalSteps, Orchestrator, PeerLink, Unimplemented};
use crate::store::{StateStore, StoreError};

/// Lo que la UI recibe cuando pide algo que este agente todavía no sabe hacer.
pub const NOT_IMPLEMENTED: &str = "todavía no implementado";
/// Lo que la UI recibe si pide un cambio con otro en curso.
pub const BUSY: &str = "ya hay un cambio en curso";
/// Lo que la UI recibe si pide emparejar sin la red en marcha.
pub const NO_NETWORK: &str = "la red del agente no está en marcha";

/// Quien sabe emparejar (la red): el agente le pasa los pedidos de la UI y publica
/// el avance con [`Agent::set_pairing_status`]. Cada método contesta enseguida; lo
/// que tarda sigue en segundo plano. `Err` es el motivo para la UI.
pub trait PairingControl: Send + Sync {
    fn start(&self) -> Result<(), String>;
    fn confirm(&self) -> Result<(), String>;
    fn cancel(&self) -> Result<(), String>;
    fn unpair(&self) -> Result<(), String>;
}

#[derive(Debug)]
pub struct Agent {
    cfg: AgentConfig,
    store: StateStore,
    state: watch::Sender<State>,
    /// Leer el estado y guardarlo tiene que ser una sola operación: si no, un
    /// guardado con el estado viejo puede terminar después de uno con el nuevo.
    persisting: Mutex<()>,
    orch: Orchestrator,
    /// Los cambios de entrada que el usuario hace a mano y confirma desde la UI.
    manual: Arc<ManualSwitcher>,
    /// Las sondas de calibración que muestra la UI de esta máquina (T-078).
    probes: ProbeBoard,
    /// Avisos de energía de la PC que no vienen de una transacción.
    power: broadcast::Sender<PowerEvent>,
    /// Con quién está emparejado y por qué enlace lo ve; lo actualiza la red.
    peer_status: watch::Sender<PeerStatus>,
    /// En qué va el emparejamiento; lo actualiza la red.
    pairing_status: watch::Sender<PairingStatus>,
    pairing: Mutex<Option<Arc<dyn PairingControl>>>,
    /// El teclado y mouse como lo ve la UI; lo actualiza [`crate::input`].
    input_status: watch::Sender<InputStatus>,
    /// Lo que el otro agente manda para el teclado y mouse.
    inbox: PeerInbox,
    /// Sube con cada `UpdateInputGeometry`: hay que volver a leer las pantallas.
    geometry: Arc<watch::Sender<u64>>,
}

impl std::fmt::Debug for dyn PairingControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairingControl")
    }
}

/// Lo último que mandó el otro agente para el teclado y mouse, y a quién pasarle los
/// streams de entrada y portapapeles que abre.
#[derive(Debug)]
pub struct PeerInbox {
    pub hub: watch::Sender<Option<HubReport>>,
    pub screens: watch::Sender<Option<Vec<ScreenRect>>>,
    channels: Mutex<Option<mpsc::UnboundedSender<IncomingChannel>>>,
}

/// Un stream de entrada o de portapapeles que abrió el otro agente, sobre su conexión
/// (los datagramas del mouse llegan por la misma).
pub struct IncomingChannel {
    pub channel: Channel,
    pub conn: Connection,
    pub stream: ControlStream,
}

impl std::fmt::Debug for IncomingChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncomingChannel")
            .field("channel", &self.channel)
            .field("from", &self.conn.remote_address())
            .finish_non_exhaustive()
    }
}

impl PeerInbox {
    fn new() -> Self {
        PeerInbox {
            hub: watch::Sender::new(None),
            screens: watch::Sender::new(None),
            channels: Mutex::new(None),
        }
    }

    /// Quien atiende los streams de entrada y portapapeles; reemplaza al anterior.
    pub fn set_channels(&self, tx: mpsc::UnboundedSender<IncomingChannel>) {
        *self.channels.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
    }

    /// Entrega el stream; sin nadie que lo atienda se descarta (se cierra).
    pub fn deliver(&self, incoming: IncomingChannel) {
        let tx = self.channels.lock().unwrap_or_else(PoisonError::into_inner);
        match tx.as_ref() {
            Some(tx) => {
                if let Err(e) = tx.send(incoming) {
                    tracing::debug!(canal = ?e.0.channel, "stream del otro agente sin quien lo atienda");
                }
            }
            None => tracing::debug!(
                canal = ?incoming.channel,
                "stream del otro agente sin motor de entrada: se cierra"
            ),
        }
    }
}

/// Avisos de energía que se guardan para un suscriptor lento.
const POWER_EVENT_BUFFER: usize = 16;

impl Agent {
    /// Arranca desde el estado guardado (o el inicial, si no hay), sin backends de
    /// plataforma: todo paso local responde «no implementado».
    pub fn new(cfg: AgentConfig, store: StateStore) -> Self {
        Self::with_local_steps(cfg, store, Arc::new(Unimplemented))
    }

    /// Como [`Agent::new`], con los pasos que esta máquina sabe hacer.
    pub fn with_local_steps(
        cfg: AgentConfig,
        store: StateStore,
        local: Arc<dyn LocalSteps>,
    ) -> Self {
        Self::with_backends(cfg, store, local, Arc::new(ManualSwitcher::new()))
    }

    /// Con los backends reales de esta máquina ([`DisplaySteps::platform`]).
    pub fn with_platform(cfg: AgentConfig, store: StateStore) -> Self {
        let manual = Arc::new(ManualSwitcher::new());
        let geometry = Arc::new(watch::Sender::new(0));
        let local = Arc::new(DisplaySteps::platform(
            &cfg,
            manual.clone(),
            geometry.clone(),
        ));
        let locator = tandem_display::desktop::native(cfg.machine)
            .map(|d| Arc::new(DesktopLocator(d)) as Arc<dyn crate::calibration::ScreenLocator>);
        let mut agent = Self::with_backends(cfg, store, local, manual);
        agent.geometry = geometry;
        agent.probes = ProbeBoard::new(locator);
        agent
    }

    /// `manual` es el backend que `local` usa para los cambios a mano: sus pedidos
    /// llegan a la UI y las respuestas de la UI vuelven a él.
    pub fn with_backends(
        cfg: AgentConfig,
        store: StateStore,
        local: Arc<dyn LocalSteps>,
        manual: Arc<ManualSwitcher>,
    ) -> Self {
        let state = store.load_or_initial(&cfg.desk);
        let orch = Orchestrator::new(cfg.machine, local, cfg.orchestrator.timeouts());
        Agent {
            store,
            state: watch::Sender::new(state),
            persisting: Mutex::new(()),
            orch,
            manual,
            // Sin escritorio que consultar, la UI busca el monitor por su nombre.
            probes: ProbeBoard::new(None),
            power: broadcast::channel(POWER_EVENT_BUFFER).0,
            // Hasta que la red diga otra cosa, el agente trabaja solo.
            peer_status: watch::Sender::new(PeerStatus::Unpaired),
            pairing_status: watch::Sender::new(PairingStatus::Idle),
            pairing: Mutex::new(None),
            input_status: watch::Sender::new(InputStatus::new(cfg.input.server)),
            inbox: PeerInbox::new(),
            geometry: Arc::new(watch::Sender::new(0)),
            cfg,
        }
    }

    /// El teclado y mouse, para la UI.
    pub fn input_status(&self) -> InputStatus {
        self.input_status.borrow().clone()
    }

    /// Avisa cada vez que cambia el teclado y mouse.
    pub fn subscribe_input(&self) -> watch::Receiver<InputStatus> {
        self.input_status.subscribe()
    }

    pub fn set_input_status(&self, status: InputStatus) {
        self.input_status.send_if_modified(|s| {
            let changed = *s != status;
            *s = status;
            changed
        });
    }

    /// Lo que el otro agente manda para el teclado y mouse.
    pub fn inbox(&self) -> &PeerInbox {
        &self.inbox
    }

    /// Cambia con cada paso `UpdateInputGeometry` hecho en esta máquina.
    pub fn subscribe_geometry(&self) -> watch::Receiver<u64> {
        self.geometry.subscribe()
    }

    pub fn orchestrator(&self) -> &Orchestrator {
        &self.orch
    }

    /// Conexión con el otro agente para los pasos que le tocan.
    pub fn set_peer(&self, peer: Arc<dyn PeerLink>) {
        self.orch.set_peer(peer);
    }

    /// Avance de las transacciones, para la UI.
    pub fn subscribe_txns(&self) -> broadcast::Receiver<TxnEvent> {
        self.orch.subscribe()
    }

    /// Pedidos de cambio manual de entrada, para la UI.
    pub fn subscribe_manual(&self) -> broadcast::Receiver<ManualEvent> {
        self.manual.subscribe()
    }

    /// Las sondas de calibración de esta máquina: las muestra su UI.
    pub fn probes(&self) -> &ProbeBoard {
        &self.probes
    }

    /// Sondas de calibración que mostrar o quitar, para la UI.
    pub fn subscribe_probes(&self) -> broadcast::Receiver<ProbeEvent> {
        self.probes.subscribe()
    }

    /// Avisos de energía de la PC (por ejemplo, que se la dio por apagada), para la UI.
    pub fn subscribe_power(&self) -> broadcast::Receiver<PowerEvent> {
        self.power.subscribe()
    }

    /// El par y el enlace con él, para la UI.
    pub fn peer_status(&self) -> PeerStatus {
        self.peer_status.borrow().clone()
    }

    /// Avisa cada vez que cambia el par o su enlace.
    pub fn subscribe_peer(&self) -> watch::Receiver<PeerStatus> {
        self.peer_status.subscribe()
    }

    pub fn set_peer_status(&self, status: PeerStatus) {
        self.peer_status.send_if_modified(|s| {
            let changed = *s != status;
            *s = status;
            changed
        });
    }

    /// Quién atiende los pedidos de emparejamiento (la red, al arrancar).
    pub fn set_pairing(&self, control: Arc<dyn PairingControl>) {
        *self.pairing.lock().unwrap_or_else(|e| e.into_inner()) = Some(control);
    }

    pub fn pairing_status(&self) -> PairingStatus {
        self.pairing_status.borrow().clone()
    }

    /// Avisa con cada paso del emparejamiento.
    pub fn subscribe_pairing(&self) -> watch::Receiver<PairingStatus> {
        self.pairing_status.subscribe()
    }

    pub fn set_pairing_status(&self, status: PairingStatus) {
        self.pairing_status.send_if_modified(|s| {
            let changed = *s != status;
            *s = status;
            changed
        });
    }

    fn pair(&self, f: impl FnOnce(&dyn PairingControl) -> Result<(), String>) -> Reply {
        let control = self
            .pairing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match control {
            Some(c) => match f(c.as_ref()) {
                Ok(()) => Reply::Done,
                Err(why) => Reply::Rejected(why),
            },
            None => Reply::Rejected(NO_NETWORK.into()),
        }
    }

    pub(crate) fn emit_power(&self, event: PowerEvent) {
        // Sin UI conectada no hay a quién avisar, y eso no es un error.
        let _ = self.power.send(event);
    }

    pub fn machine(&self) -> Machine {
        self.cfg.machine
    }

    pub fn config(&self) -> &AgentConfig {
        &self.cfg
    }

    pub fn store_path(&self) -> &std::path::Path {
        self.store.path()
    }

    pub fn state(&self) -> State {
        self.state.borrow().clone()
    }

    /// Avisa cada vez que el estado cambia.
    pub fn subscribe(&self) -> watch::Receiver<State> {
        self.state.subscribe()
    }

    /// Publica el estado nuevo y lo guarda.
    ///
    /// Se publica aunque falle el guardado: el cambio ya ocurrió en el puesto y la UI
    /// tiene que verlo; el error se devuelve para registrarlo.
    pub fn set_state(&self, new: State) -> Result<(), StoreError> {
        let changed = self.state.send_if_modified(|s| {
            if *s == new {
                false
            } else {
                *s = new;
                true
            }
        });
        if changed { self.persist() } else { Ok(()) }
    }

    pub fn persist(&self) -> Result<(), StoreError> {
        // Si otro hilo entró en pánico guardando, el candado no protege datos: se sigue.
        let _guard = self.persisting.lock().unwrap_or_else(|e| e.into_inner());
        let state = self.state.borrow().clone();
        self.store.save(&state)
    }

    /// Contesta un pedido de la UI.
    ///
    /// Los cambios de modo y de monitor arrancan una transacción en segundo plano y
    /// se contestan con [`Reply::Accepted`]; el avance llega como [`TxnEvent`]. Tiene
    /// que llamarse dentro de un runtime de tokio.
    pub fn handle(self: &Arc<Self>, cmd: UiCommand) -> Reply {
        match cmd {
            UiCommand::GetState => Reply::State(self.state()),
            UiCommand::GetDesk => Reply::Desk(self.cfg.desk.clone()),
            UiCommand::GetPeer => Reply::Peer(self.peer_status()),
            UiCommand::GetPairing => Reply::Pairing(self.pairing_status()),
            UiCommand::StartPairing => self.pair(|p| p.start()),
            UiCommand::ConfirmPairing => self.pair(|p| p.confirm()),
            UiCommand::CancelPairing => self.pair(|p| p.cancel()),
            UiCommand::Unpair => self.pair(|p| p.unpair()),
            UiCommand::GetInput => Reply::Input(self.input_status()),
            UiCommand::SetMode(mode) => self.start(mode),
            UiCommand::SetPcPower(on) => self.start_power(on),
            UiCommand::MoveMonitor { monitor, to } => {
                if self.cfg.desk.monitor(&monitor).is_none() {
                    return Reply::Rejected(format!(
                        "el monitor `{monitor}` no está en la configuración"
                    ));
                }
                let mut assign = self.state().owner;
                assign.insert(monitor, to);
                self.start(Mode::Shared(assign))
            }
            // La preferencia de la pantalla integrada no es un modo: llega con su tarea.
            UiCommand::SetBuiltin(_) => Reply::Rejected(NOT_IMPLEMENTED.into()),
            UiCommand::AnswerManualSwitch { id, done } => match self.manual.answer(id, done) {
                Ok(()) => Reply::Done,
                Err(e) => Reply::Rejected(e.to_string()),
            },
            UiCommand::AnswerProbe { id, seen } => match self.probes.answer(id, seen) {
                Ok(()) => Reply::Done,
                Err(e) => Reply::Rejected(e.to_string()),
            },
        }
    }

    fn start(self: &Arc<Self>, mode: Mode) -> Reply {
        self.start_plan(&format!("{mode:?}"), |cfg, from| {
            Plan::to_mode(cfg, from, &mode)
        })
    }

    /// Encender es solo despertar la PC; apagar devuelve antes todo a la Mac
    /// ([`Plan::shutdown_pc`]). Pedir lo que ya está es una transacción vacía.
    fn start_power(self: &Arc<Self>, on: bool) -> Reply {
        let what = if on { "encender la PC" } else { "apagar la PC" };
        self.start_plan(what, |cfg, from| {
            if !on {
                Plan::shutdown_pc(cfg, from)
            } else if from.pc_on {
                Ok(Plan::default())
            } else {
                Ok(Plan {
                    steps: vec![Step::PowerOnPc],
                })
            }
        })
    }

    fn start_plan(
        self: &Arc<Self>,
        what: &str,
        plan_for: impl FnOnce(&tandem_core::Config, &State) -> Result<Plan, PlanError>,
    ) -> Reply {
        let Some(txn) = self.orch.try_begin() else {
            return Reply::Rejected(BUSY.into());
        };
        let busy = Busy(self.clone());
        let from = self.state();
        let plan = match plan_for(&self.cfg.desk, &from) {
            Ok(p) => p,
            Err(e) => return Reply::Rejected(e.to_string()),
        };
        tracing::info!(txn, what, steps = plan.steps.len(), "cambio de modo");
        tokio::spawn(async move {
            let agent = busy.0.clone();
            let mut state = from;
            let outcome = agent
                .orch
                .run(txn, &agent.cfg.desk, &mut state, &plan)
                .await;
            match &outcome {
                Ok(_) => tracing::info!(txn, "cambio de modo aplicado"),
                Err(e) => tracing::warn!(txn, "cambio de modo fallido: {e}"),
            }
            // Primero el estado, después el aviso: quien lo reciba ya ve el estado nuevo.
            if let Err(e) = agent.set_state(state.clone()) {
                tracing::warn!("{e}");
            }
            // Libre antes del aviso: quien lo reciba ya puede pedir el siguiente cambio.
            drop(busy);
            agent.orch.finish(txn, &state, &outcome);
        });
        Reply::Accepted { txn }
    }
}

/// Libera el orquestador al terminar la transacción, aunque la tarea entre en pánico.
struct Busy(Arc<Agent>);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.orch.end();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use std::time::Duration;

    use tandem_core::{BuiltinDisplay, Step};

    use super::*;
    use crate::config::fixtures;
    use crate::orchestrator::STEP_NOT_IMPLEMENTED;
    use crate::orchestrator::tests::{FakePeer, FakeSteps};

    pub fn agent(dir: &Path) -> Agent {
        Agent::new(fixtures::desk(), StateStore::new(dir.join("state.json")))
    }

    #[test]
    fn starts_from_initial_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        assert_eq!(a.machine(), Machine::Mac);
        assert_eq!(a.state(), State::initial(&a.config().desk));
    }

    #[test]
    fn get_state_replies_with_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        assert_eq!(a.handle(UiCommand::GetState), Reply::State(a.state()));
    }

    #[test]
    fn starts_unpaired_and_reports_the_peer() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        assert_eq!(
            a.handle(UiCommand::GetPeer),
            Reply::Peer(PeerStatus::Unpaired)
        );
        let mut rx = a.subscribe_peer();
        let paired = PeerStatus::Paired {
            name: "PC".into(),
            link: None,
        };
        a.set_peer_status(paired.clone());
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), paired);
        // Lo mismo otra vez no es un cambio.
        a.set_peer_status(paired.clone());
        assert!(!rx.has_changed().unwrap());
        assert_eq!(a.handle(UiCommand::GetPeer), Reply::Peer(paired));
    }

    #[test]
    fn pairing_without_network_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        assert_eq!(
            a.handle(UiCommand::GetPairing),
            Reply::Pairing(PairingStatus::Idle)
        );
        for cmd in [
            UiCommand::StartPairing,
            UiCommand::ConfirmPairing,
            UiCommand::CancelPairing,
            UiCommand::Unpair,
        ] {
            assert_eq!(a.handle(cmd), Reply::Rejected(NO_NETWORK.into()));
        }
    }

    /// Anota lo que se le pide; `confirm` falla para ver que el motivo llega a la UI.
    #[derive(Default)]
    struct FakePairing(Mutex<Vec<&'static str>>);

    impl PairingControl for FakePairing {
        fn start(&self) -> Result<(), String> {
            self.0.lock().unwrap().push("start");
            Ok(())
        }
        fn confirm(&self) -> Result<(), String> {
            self.0.lock().unwrap().push("confirm");
            Err("no hay código".into())
        }
        fn cancel(&self) -> Result<(), String> {
            self.0.lock().unwrap().push("cancel");
            Ok(())
        }
        fn unpair(&self) -> Result<(), String> {
            self.0.lock().unwrap().push("unpair");
            Ok(())
        }
    }

    #[test]
    fn pairing_commands_go_to_the_network_and_status_is_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        let fake = Arc::new(FakePairing::default());
        a.set_pairing(fake.clone());
        assert_eq!(a.handle(UiCommand::StartPairing), Reply::Done);
        assert_eq!(
            a.handle(UiCommand::ConfirmPairing),
            Reply::Rejected("no hay código".into())
        );
        assert_eq!(a.handle(UiCommand::CancelPairing), Reply::Done);
        assert_eq!(a.handle(UiCommand::Unpair), Reply::Done);
        assert_eq!(
            *fake.0.lock().unwrap(),
            ["start", "confirm", "cancel", "unpair"]
        );

        let mut rx = a.subscribe_pairing();
        a.set_pairing_status(PairingStatus::Searching);
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), PairingStatus::Searching);
        a.set_pairing_status(PairingStatus::Searching);
        assert!(!rx.has_changed().unwrap());
        assert_eq!(
            a.handle(UiCommand::GetPairing),
            Reply::Pairing(PairingStatus::Searching)
        );
    }

    #[test]
    fn get_desk_replies_with_the_configured_desk() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        assert_eq!(
            a.handle(UiCommand::GetDesk),
            Reply::Desk(a.config().desk.clone())
        );
    }

    fn steps_of(events: &[TxnEvent]) -> Vec<Step> {
        match events.first() {
            Some(TxnEvent::Started { steps, .. }) => steps.clone(),
            other => panic!("se esperaba Started: {other:?}"),
        }
    }

    #[tokio::test]
    async fn power_on_only_wakes_the_pc() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(dir.path(), FakeSteps::default(), FakeSteps::default());
        let mut rx = a.subscribe_txns();
        assert!(matches!(
            a.handle(UiCommand::SetPcPower(true)),
            Reply::Accepted { .. }
        ));
        let events = until_outcome(&mut rx).await;
        assert_eq!(steps_of(&events), [Step::PowerOnPc]);
        let mut expected = State::initial(&a.config().desk);
        expected.pc_on = true;
        assert_eq!(a.state(), expected);

        // Ya encendida, pedirlo otra vez no hace nada.
        a.handle(UiCommand::SetPcPower(true));
        assert!(steps_of(&until_outcome(&mut rx).await).is_empty());
    }

    #[tokio::test]
    async fn power_off_returns_monitors_to_the_mac_first() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(dir.path(), FakeSteps::default(), FakeSteps::default());
        let mut s = a.state();
        s.pc_on = true;
        s.owner.insert("m34".into(), Machine::Pc);
        a.set_state(s).unwrap();
        let mut rx = a.subscribe_txns();
        assert!(matches!(
            a.handle(UiCommand::SetPcPower(false)),
            Reply::Accepted { .. }
        ));
        let steps = steps_of(&until_outcome(&mut rx).await);
        assert!(matches!(
            steps.first(),
            Some(Step::SwitchInput {
                to: Machine::Mac,
                ..
            })
        ));
        assert_eq!(steps.last(), Some(&Step::PowerOffPc));
        assert_eq!(a.state(), State::initial(&a.config().desk));
    }

    /// Avisos de una transacción hasta su desenlace.
    pub async fn until_outcome(rx: &mut broadcast::Receiver<TxnEvent>) -> Vec<TxnEvent> {
        let mut events = Vec::new();
        loop {
            let ev = rx.recv().await.unwrap();
            let end = matches!(
                ev,
                TxnEvent::Committed { .. }
                    | TxnEvent::RolledBack { .. }
                    | TxnEvent::Inconsistent { .. }
            );
            events.push(ev);
            if end {
                return events;
            }
        }
    }

    fn with_fakes(dir: &Path, local: FakeSteps, peer: FakeSteps) -> Arc<Agent> {
        let a = Agent::with_local_steps(
            fixtures::desk(),
            StateStore::new(dir.join("state.json")),
            Arc::new(local),
        );
        a.set_peer(Arc::new(FakePeer(Arc::new(peer))));
        Arc::new(a)
    }

    #[tokio::test]
    async fn without_backends_a_mode_change_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        let mut rx = a.subscribe_txns();
        let Reply::Accepted { txn } = a.handle(UiCommand::SetMode(Mode::SoloPc)) else {
            panic!("se esperaba Accepted");
        };
        let events = until_outcome(&mut rx).await;
        let initial = State::initial(&a.config().desk);
        assert!(events.iter().any(|e| matches!(
            e,
            TxnEvent::StepFailed { error, .. } if error.contains(STEP_NOT_IMPLEMENTED)
        )));
        assert_eq!(
            events.last(),
            Some(&TxnEvent::RolledBack {
                txn,
                state: initial.clone()
            })
        );
        assert_eq!(a.state(), initial);
    }

    #[tokio::test]
    async fn move_monitor_is_a_shared_mode() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(dir.path(), FakeSteps::default(), FakeSteps::default());
        let mut s = a.state();
        s.pc_on = true;
        a.set_state(s.clone()).unwrap();
        let mut rx = a.subscribe_txns();
        let reply = a.handle(UiCommand::MoveMonitor {
            monitor: "m32".into(),
            to: Machine::Pc,
        });
        assert!(matches!(reply, Reply::Accepted { .. }));
        until_outcome(&mut rx).await;
        s.owner.insert("m32".into(), Machine::Pc);
        assert_eq!(a.state(), s);
        // Y queda guardado.
        assert_eq!(agent(dir.path()).state(), s);
    }

    #[tokio::test]
    async fn same_mode_commits_without_steps() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        let mut rx = a.subscribe_txns();
        let Reply::Accepted { txn } = a.handle(UiCommand::SetMode(Mode::SoloMac)) else {
            panic!("se esperaba Accepted");
        };
        assert_eq!(
            until_outcome(&mut rx).await,
            [
                TxnEvent::Started { txn, steps: vec![] },
                TxnEvent::Committed {
                    txn,
                    state: a.state()
                }
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn one_change_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(
            dir.path(),
            FakeSteps {
                delays: vec![(Step::PowerOnPc, Duration::from_secs(30))],
                ..Default::default()
            },
            FakeSteps::default(),
        );
        let mut rx = a.subscribe_txns();
        assert!(matches!(
            a.handle(UiCommand::SetMode(Mode::SoloPc)),
            Reply::Accepted { .. }
        ));
        assert_eq!(
            a.handle(UiCommand::SetMode(Mode::SoloMac)),
            Reply::Rejected(BUSY.into())
        );
        until_outcome(&mut rx).await;
        // Terminada la primera, se acepta la siguiente.
        assert!(matches!(
            a.handle(UiCommand::SetMode(Mode::SoloMac)),
            Reply::Accepted { .. }
        ));
    }

    #[tokio::test]
    async fn bad_requests_are_rejected_without_touching_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        let mut rx = a.subscribe_txns();
        for cmd in [
            UiCommand::MoveMonitor {
                monitor: "no-existe".into(),
                to: Machine::Pc,
            },
            UiCommand::SetBuiltin(BuiltinDisplay::Mirror),
        ] {
            assert!(matches!(a.handle(cmd), Reply::Rejected(_)));
        }
        assert_eq!(a.state(), State::initial(&a.config().desk));
        assert!(rx.try_recv().is_err());
        // Un rechazo no deja el orquestador ocupado.
        assert!(matches!(
            a.handle(UiCommand::SetMode(Mode::SoloMac)),
            Reply::Accepted { .. }
        ));
    }

    #[test]
    fn set_state_publishes_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        let mut rx = a.subscribe();
        let mut s = a.state();
        s.pc_on = true;
        a.set_state(s.clone()).unwrap();
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), s);
        // Otro agente sobre el mismo directorio arranca con ese estado.
        assert_eq!(agent(dir.path()).state(), s);
    }

    // QA ronda 1: `persist` leía el estado y lo guardaba sin exclusión. Con dos
    // `set_state` a la vez, el que leyó el estado viejo podía escribir último y el
    // archivo quedaba con un estado que ya no era el vigente.
    #[test]
    fn concurrent_set_state_persists_the_final_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        let base = a.state();
        for round in 0..20 {
            std::thread::scope(|scope| {
                for i in 0..8u32 {
                    let mut s = base.clone();
                    s.pc_on = (i + round) % 2 == 0;
                    s.builtin_pref = [
                        BuiltinDisplay::Off,
                        BuiltinDisplay::Mirror,
                        BuiltinDisplay::Extend,
                    ][(i % 3) as usize];
                    let a = &a;
                    scope.spawn(move || a.set_state(s).unwrap());
                }
            });
            let saved = StateStore::new(dir.path().join("state.json"))
                .load()
                .unwrap();
            assert_eq!(saved, Some(a.state()), "ronda {round}");
        }
    }

    #[test]
    fn same_state_does_not_notify() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        let rx = a.subscribe();
        a.set_state(a.state()).unwrap();
        assert!(!rx.has_changed().unwrap());
    }
}
