//! Los módulos de esta máquina en el agente (T-098): encenderlos y apagarlos desde la UI de
//! cualquiera de las dos y anunciarlos en las capacidades.

use tandem_core::Machine;
use tandem_proto::{Capabilities, ControlMsg, ModuleId, Reply};
use tokio::sync::watch;

use super::Agent;
use crate::capabilities;
use crate::config::AgentConfig;
use crate::modules::{ModuleContext, ModuleSwitches, default_name};

/// Lo apagado al arrancar: lo que la configuración apaga (`[input] clipboard = false`).
pub(super) fn initial_switches(cfg: &AgentConfig) -> ModuleSwitches {
    let mut s = ModuleSwitches::default();
    if !cfg.input.clipboard {
        s.set(ModuleId::Clipboard, false);
    }
    s
}

impl Agent {
    /// Lo que anuncia esta máquina ahora mismo, módulos incluidos.
    pub(super) fn local_capabilities(&self) -> Capabilities {
        let info = self.local_info();
        let input = self.input_status();
        let switches = self.modules.borrow().clone();
        let modules = self.registry.snapshot(&ModuleContext {
            os: info.os,
            input: &input,
            power: &capabilities::power(&self.cfg, info.os),
            switches: &switches,
        });
        capabilities::local(
            &self.cfg,
            self.machine(),
            info,
            &input,
            &self.process,
            modules,
        )
    }

    /// Si el usuario no apagó `id` aquí.
    pub fn module_on(&self, id: ModuleId) -> bool {
        self.modules.borrow().is_on(id)
    }

    /// Avisa cada vez que se enciende o apaga un módulo, o cambia un problema reportado.
    pub fn subscribe_modules(&self) -> watch::Receiver<ModuleSwitches> {
        self.modules.subscribe()
    }

    /// Enciende o apaga un módulo de esta máquina. `Err` con la razón si aquí no existe o
    /// no se puede apagar.
    pub fn set_module(&self, id: ModuleId, on: bool) -> Result<(), String> {
        self.registry.check_switch(id)?;
        if self.modules.send_if_modified(|s| s.set(id, on)) {
            tracing::info!(modulo = ?id, encendido = on, "módulo cambiado");
            self.refresh_capabilities();
        }
        Ok(())
    }

    /// Un módulo en marcha avisa que no puede funcionar (`Some`, con la razón para la UI) o
    /// que ya puede (`None`).
    pub fn set_module_problem(&self, id: ModuleId, problem: Option<String>) {
        if self
            .modules
            .send_if_modified(|s| s.set_problem(id, problem))
        {
            self.refresh_capabilities();
        }
    }

    /// La UI de la otra máquina pidió encender o apagar un módulo de esta.
    pub fn receive_set_module(&self, id: ModuleId, on: bool) {
        if let Err(e) = self.set_module(id, on) {
            tracing::warn!("la otra máquina pidió cambiar un módulo: {e}");
        }
    }

    /// [`tandem_proto::UiCommand::SetModule`]: aquí se aplica; para la otra se comprueba con
    /// su último anuncio y se le pide por el plano de control. Su estado nuevo llega con su
    /// próximo anuncio.
    pub(super) fn switch_module(&self, machine: Machine, module: ModuleId, on: bool) -> Reply {
        if machine == self.machine() {
            return match self.set_module(module, on) {
                Ok(()) => Reply::Done,
                Err(why) => Reply::Rejected(why),
            };
        }
        let name = self.machine_name(machine);
        if !*self.orch.peer_connected().borrow() {
            return Reply::Rejected(format!("la {name} no está conectada"));
        }
        let Some(peer) = self.capabilities().peer else {
            return Reply::Rejected(format!("todavía no se sabe qué módulos tiene la {name}"));
        };
        match peer.modules.iter().find(|m| m.id == module) {
            None => Reply::Rejected(format!("«{}» no existe en la {name}", default_name(module))),
            Some(m) if !m.switchable => {
                Reply::Rejected(format!("«{}» no se puede encender ni apagar", m.name))
            }
            Some(_) => {
                self.orch.peer().send(ControlMsg::SetModule { module, on });
                Reply::Done
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tandem_core::{Os, State, Step};
    use tandem_proto::{ModuleState, UiCommand};

    use super::*;
    use crate::StateStore;
    use crate::config::fixtures;
    use crate::orchestrator::{PeerLink, StepFuture};

    /// Un par que guarda lo que se le manda.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<ControlMsg>>);

    impl PeerLink for Recorder {
        fn execute<'a>(&'a self, _step: &'a Step) -> StepFuture<'a> {
            Box::pin(async { Ok(()) })
        }
        fn publish_state(&self, _state: &State) {}
        fn send(&self, msg: ControlMsg) {
            self.0.lock().unwrap().push(msg);
        }
    }

    impl Recorder {
        fn taken(&self) -> Vec<ControlMsg> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    fn agent_on(dir: &std::path::Path, os: Os) -> Agent {
        let mut cfg = fixtures::desk_with_wol();
        cfg.os = os;
        Agent::new(cfg, StateStore::new(dir.join("state.json")))
    }

    fn state(caps: &Capabilities, id: ModuleId) -> ModuleState {
        caps.modules
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.state.clone())
            .unwrap_or_else(|| panic!("falta {id:?}"))
    }

    #[test]
    fn each_machine_announces_the_modules_of_its_system() {
        let dir = tempfile::tempdir().unwrap();
        let mac = agent_on(dir.path(), Os::MacOs).capabilities().here;
        assert!(!mac.modules.iter().any(|m| m.id == ModuleId::SecureDesktop));
        let pc = agent_on(dir.path(), Os::Windows).capabilities().here;
        assert!(pc.modules.iter().any(|m| m.id == ModuleId::SecureDesktop));
        assert_eq!(state(&pc, ModuleId::Input), ModuleState::Active);
    }

    #[test]
    fn the_clipboard_starts_off_if_the_config_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = fixtures::desk();
        cfg.input.clipboard = false;
        let a = Agent::new(cfg, StateStore::new(dir.path().join("state.json")));
        assert!(!a.module_on(ModuleId::Clipboard));
        assert_eq!(
            state(&a.capabilities().here, ModuleId::Clipboard),
            ModuleState::Off
        );
    }

    #[test]
    fn switching_a_local_module_changes_the_announcement() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent_on(dir.path(), Os::Windows));
        let peer = Arc::new(Recorder::default());
        a.set_peer(peer.clone());
        peer.taken();
        let mut modules = a.subscribe_modules();
        let reply = a.handle(UiCommand::SetModule {
            machine: a.machine(),
            module: ModuleId::Clipboard,
            on: false,
        });
        assert_eq!(reply, Reply::Done);
        assert!(modules.has_changed().unwrap());
        assert!(!modules.borrow_and_update().is_on(ModuleId::Clipboard));
        let here = a.capabilities().here;
        assert_eq!(state(&here, ModuleId::Clipboard), ModuleState::Off);
        // Sin runtime, el anuncio sale enseguida.
        assert_eq!(peer.taken(), [ControlMsg::Capabilities(Box::new(here))]);
        // Pedir lo mismo otra vez no anuncia nada.
        a.set_module(ModuleId::Clipboard, false).unwrap();
        assert!(peer.taken().is_empty());
    }

    #[test]
    fn a_module_that_cannot_be_switched_is_rejected_with_its_reason() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent_on(dir.path(), Os::MacOs));
        let Reply::Rejected(why) = a.handle(UiCommand::SetModule {
            machine: a.machine(),
            module: ModuleId::SecureDesktop,
            on: true,
        }) else {
            panic!("el escritorio seguro no existe en macOS");
        };
        assert!(why.contains("no existe"), "{why}");
        let Reply::Rejected(why) = a.handle(UiCommand::SetModule {
            machine: a.machine(),
            module: ModuleId::Tunnels,
            on: false,
        }) else {
            panic!("los túneles no se apagan desde aquí");
        };
        assert!(why.contains("no se puede"), "{why}");
    }

    #[test]
    fn a_reported_problem_is_announced_and_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_on(dir.path(), Os::Windows);
        a.set_module_problem(
            ModuleId::Clipboard,
            Some("No se pudo abrir el portapapeles del sistema".into()),
        );
        assert_eq!(
            state(&a.capabilities().here, ModuleId::Clipboard),
            ModuleState::Unavailable {
                reason: "No se pudo abrir el portapapeles del sistema".into()
            }
        );
        a.set_module_problem(ModuleId::Clipboard, None);
        assert_eq!(
            state(&a.capabilities().here, ModuleId::Clipboard),
            ModuleState::Active
        );
    }

    /// El otro lado, como si hubiera anunciado sus módulos.
    fn learn_peer(a: &Agent, os: Os) {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = fixtures::desk_with_wol();
        cfg.os = os;
        cfg.machine = a.machine().other();
        let other = Agent::new(cfg, StateStore::new(dir.path().join("state.json")));
        a.set_peer_capabilities(other.capabilities().here);
    }

    #[test]
    fn a_remote_module_goes_through_the_control_plane() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent_on(dir.path(), Os::MacOs));
        let other = a.machine().other();
        let set = |module, on| {
            a.handle(UiCommand::SetModule {
                machine: other,
                module,
                on,
            })
        };
        // Sin enlace no hay a quién pedírselo.
        let Reply::Rejected(why) = set(ModuleId::Clipboard, false) else {
            panic!("sin la otra conectada");
        };
        assert!(why.contains("no está conectada"), "{why}");

        let peer = Arc::new(Recorder::default());
        a.set_peer(peer.clone());
        peer.taken();
        let Reply::Rejected(why) = set(ModuleId::Clipboard, false) else {
            panic!("sin su anuncio no se sabe qué tiene");
        };
        assert!(why.contains("todavía no se sabe"), "{why}");

        learn_peer(&a, Os::Windows);
        // Conocerla anota las dos máquinas en el puesto, que también viaja.
        peer.taken();
        assert_eq!(set(ModuleId::Clipboard, false), Reply::Done);
        assert_eq!(
            peer.taken(),
            [ControlMsg::SetModule {
                module: ModuleId::Clipboard,
                on: false
            }]
        );
        // Lo que la otra no puede apagar ni se le pide.
        assert!(matches!(set(ModuleId::Power, false), Reply::Rejected(_)));
        assert!(peer.taken().is_empty());
        // Y lo que no existe en su sistema tampoco.
        learn_peer(&a, Os::MacOs);
        let Reply::Rejected(why) = set(ModuleId::SecureDesktop, true) else {
            panic!("una Mac no tiene escritorio seguro");
        };
        assert!(why.contains("no existe"), "{why}");
    }

    #[test]
    fn the_peer_request_is_applied_here() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_on(dir.path(), Os::Windows);
        a.receive_set_module(ModuleId::Input, false);
        assert!(!a.module_on(ModuleId::Input));
        // Lo que no se puede cambiar se ignora (con un aviso en el log).
        a.receive_set_module(ModuleId::Power, false);
        assert!(a.module_on(ModuleId::Power));
    }

    /// Varios cambios seguidos llegan a la otra máquina como un solo anuncio, con lo último.
    #[tokio::test(start_paused = true)]
    async fn changes_reach_the_peer_debounced() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent_on(dir.path(), Os::Windows));
        let peer = Arc::new(Recorder::default());
        a.set_peer(peer.clone());
        peer.taken();
        a.set_module(ModuleId::Clipboard, false).unwrap();
        a.set_module(ModuleId::Input, false).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        a.set_module(ModuleId::Input, true).unwrap();
        assert!(peer.taken().is_empty(), "todavía dentro de la ventana");
        tokio::time::sleep(capabilities::ANNOUNCE_DEBOUNCE).await;
        let sent = peer.taken();
        assert_eq!(
            sent,
            [ControlMsg::Capabilities(Box::new(a.capabilities().here))]
        );
        let ControlMsg::Capabilities(c) = &sent[0] else {
            unreachable!()
        };
        assert_eq!(state(c, ModuleId::Clipboard), ModuleState::Off);
        assert_eq!(state(c, ModuleId::Input), ModuleState::Active);
    }

    /// Hallazgo de la revisión del PR #72: un cambio dentro de la ventana, con un enlace que
    /// apareció mientras tanto, se mandaba al par de cuando se abrió la ventana (ninguno).
    #[tokio::test(start_paused = true)]
    async fn a_change_in_the_window_reaches_a_peer_that_connected_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent_on(dir.path(), Os::Windows));
        a.set_module(ModuleId::Clipboard, false).unwrap(); // abre la ventana sin par
        tokio::time::sleep(Duration::from_millis(50)).await;
        let peer = Arc::new(Recorder::default());
        a.set_peer(peer.clone());
        peer.taken();
        a.set_module(ModuleId::Input, false).unwrap(); // dentro de la ventana
        tokio::time::sleep(capabilities::ANNOUNCE_DEBOUNCE * 2).await;
        assert_eq!(
            peer.taken(),
            [ControlMsg::Capabilities(Box::new(a.capabilities().here))]
        );
    }
}
