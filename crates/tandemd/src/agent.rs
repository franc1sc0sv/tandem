//! El estado vivo del agente y lo que contesta a la UI.

use std::sync::{Arc, Mutex};

use tandem_core::{Machine, Mode, Plan, State};
use tandem_proto::{Reply, TxnEvent, UiCommand};
use tokio::sync::{broadcast, watch};

use crate::config::AgentConfig;
use crate::orchestrator::{LocalSteps, Orchestrator, PeerLink, Unimplemented};
use crate::store::{StateStore, StoreError};

/// Lo que la UI recibe cuando pide algo que este agente todavía no sabe hacer.
pub const NOT_IMPLEMENTED: &str = "todavía no implementado";
/// Lo que la UI recibe si pide un cambio con otro en curso.
pub const BUSY: &str = "ya hay un cambio en curso";

#[derive(Debug)]
pub struct Agent {
    cfg: AgentConfig,
    store: StateStore,
    state: watch::Sender<State>,
    /// Leer el estado y guardarlo tiene que ser una sola operación: si no, un
    /// guardado con el estado viejo puede terminar después de uno con el nuevo.
    persisting: Mutex<()>,
    orch: Orchestrator,
}

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
        let state = store.load_or_initial(&cfg.desk);
        let orch = Orchestrator::new(cfg.machine, local, cfg.orchestrator.timeouts());
        Agent {
            cfg,
            store,
            state: watch::Sender::new(state),
            persisting: Mutex::new(()),
            orch,
        }
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
            UiCommand::SetMode(mode) => self.start(mode),
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
        }
    }

    fn start(self: &Arc<Self>, mode: Mode) -> Reply {
        let Some(txn) = self.orch.try_begin() else {
            return Reply::Rejected(BUSY.into());
        };
        let busy = Busy(self.clone());
        let from = self.state();
        let plan = match Plan::to_mode(&self.cfg.desk, &from, &mode) {
            Ok(p) => p,
            Err(e) => return Reply::Rejected(e.to_string()),
        };
        tracing::info!(txn, ?mode, steps = plan.steps.len(), "cambio de modo");
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
