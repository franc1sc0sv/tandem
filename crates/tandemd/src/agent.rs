//! El estado vivo del agente y lo que contesta a la UI.

use std::sync::Mutex;

use tandem_core::{Machine, State};
use tandem_proto::{Reply, UiCommand};
use tokio::sync::watch;

use crate::config::AgentConfig;
use crate::store::{StateStore, StoreError};

/// Lo que la UI recibe cuando pide algo que este agente todavía no sabe hacer.
pub const NOT_IMPLEMENTED: &str = "todavía no implementado";

#[derive(Debug)]
pub struct Agent {
    cfg: AgentConfig,
    store: StateStore,
    state: watch::Sender<State>,
    /// Leer el estado y guardarlo tiene que ser una sola operación: si no, un
    /// guardado con el estado viejo puede terminar después de uno con el nuevo.
    persisting: Mutex<()>,
}

impl Agent {
    /// Arranca desde el estado guardado (o el inicial, si no hay).
    pub fn new(cfg: AgentConfig, store: StateStore) -> Self {
        let state = store.load_or_initial(&cfg.desk);
        Agent {
            cfg,
            store,
            state: watch::Sender::new(state),
            persisting: Mutex::new(()),
        }
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
    /// Los cambios de modo, de monitor y de pantalla integrada son transacciones del
    /// orquestador; mientras no exista, se rechazan con un motivo claro.
    pub fn handle(&self, cmd: UiCommand) -> Reply {
        match cmd {
            UiCommand::GetState => Reply::State(self.state()),
            UiCommand::SetMode(_) | UiCommand::MoveMonitor { .. } | UiCommand::SetBuiltin(_) => {
                Reply::Rejected(NOT_IMPLEMENTED.into())
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use tandem_core::{BuiltinDisplay, Mode};

    use super::*;
    use crate::config::fixtures;

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
        let a = agent(dir.path());
        assert_eq!(a.handle(UiCommand::GetState), Reply::State(a.state()));
    }

    #[test]
    fn changes_are_rejected_until_the_orchestrator_exists() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        for cmd in [
            UiCommand::SetMode(Mode::SoloPc),
            UiCommand::MoveMonitor {
                monitor: "m34".into(),
                to: Machine::Pc,
            },
            UiCommand::SetBuiltin(BuiltinDisplay::Mirror),
        ] {
            assert_eq!(a.handle(cmd), Reply::Rejected(NOT_IMPLEMENTED.into()));
        }
        // Y no tocan el estado.
        assert_eq!(a.state(), State::initial(&a.config().desk));
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
