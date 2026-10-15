//! El estado vivo del agente y lo que contesta a la UI.

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
}

impl Agent {
    /// Arranca desde el estado guardado (o el inicial, si no hay).
    pub fn new(cfg: AgentConfig, store: StateStore) -> Self {
        let state = store.load_or_initial(&cfg.desk);
        Agent {
            cfg,
            store,
            state: watch::Sender::new(state),
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
        self.store.save(&self.state.borrow())
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

    #[test]
    fn same_state_does_not_notify() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        let rx = a.subscribe();
        a.set_state(a.state()).unwrap();
        assert!(!rx.has_changed().unwrap());
    }
}
