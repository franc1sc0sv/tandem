//! Los módulos de esta máquina en el agente (T-098): encenderlos y apagarlos desde la UI de
//! cualquiera de las dos y anunciarlos en las capacidades.
//!
//! Qué módulo está encendido es parte del puesto compartido (`Desk.modules`, D4): se cambia
//! con un parche, se guarda en `tandem.toml` y llega a la otra con el puesto. Cada agente
//! copia los de su lado a sus interruptores ([`Agent::sync_switches`]).

use tandem_core::{DeskPatch, Machine};
use tandem_proto::{Capabilities, ModuleId, Reply};
use tokio::sync::watch;

use super::Agent;
use crate::capabilities;
use crate::config::AgentConfig;
use crate::modules::{ModuleContext, ModuleSwitches, default_name};

/// Lo apagado al arrancar: lo que el puesto apaga en esta máquina (`[modules.<lado>]`).
pub(super) fn initial_switches(cfg: &AgentConfig) -> ModuleSwitches {
    let mut s = ModuleSwitches::default();
    for id in ModuleId::ALL {
        if cfg.desk.modules.get(cfg.machine, id.as_str()) == Some(false) {
            s.set(id, false);
        }
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
            power: &self.power_caps(info.os),
            switches: &switches,
        });
        let mut caps = capabilities::local(
            &self.cfg,
            self.machine(),
            info,
            &input,
            &self.process,
            modules,
        );
        caps.power = self.power_caps(caps.info.os);
        caps.monitors = self.monitors.local_channels();
        self.add_environments(&mut caps);
        caps
    }

    /// Si el usuario no apagó `id` aquí.
    pub fn module_on(&self, id: ModuleId) -> bool {
        self.modules.borrow().is_on(id)
    }

    /// Avisa cada vez que se enciende o apaga un módulo, o cambia un problema reportado.
    pub fn subscribe_modules(&self) -> watch::Receiver<ModuleSwitches> {
        self.modules.subscribe()
    }

    /// Enciende o apaga un módulo de esta máquina: un cambio del puesto. `Err` con la razón
    /// si aquí no existe o no se puede apagar.
    pub fn set_module(&self, id: ModuleId, on: bool) -> Result<(), String> {
        self.registry.check_switch(id)?;
        self.edit_desk(DeskPatch::Module {
            machine: self.machine(),
            module: id.as_str().to_owned(),
            on,
        })
        .map(|_| ())
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

    /// [`tandem_proto::UiCommand::SetModule`]: aquí se aplica; para la otra se comprueba con
    /// su último anuncio y se cambia en el puesto, que le llega como cualquier cambio. Su
    /// estado nuevo llega con su próximo anuncio.
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
            Some(_) => match self.edit_desk(DeskPatch::Module {
                machine,
                module: module.as_str().to_owned(),
                on,
            }) {
                Ok(_) => Reply::Done,
                Err(why) => Reply::Rejected(why),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tandem_core::{Os, State, Step};
    use tandem_proto::{ControlMsg, ModuleState, UiCommand};

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

        /// Solo los anuncios de capacidades (el puesto también viaja con cada cambio).
        fn announced(&self) -> Vec<ControlMsg> {
            self.taken()
                .into_iter()
                .filter(|m| matches!(m, ControlMsg::Capabilities(_)))
                .collect()
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
    fn the_clipboard_starts_off_if_the_desk_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = fixtures::desk();
        cfg.desk.modules.set(cfg.machine, "clipboard", false);
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
        // Es un cambio del puesto (D4): va guardado, con versión nueva, y la otra lo recibe.
        assert_eq!(a.desk().modules.get(a.machine(), "clipboard"), Some(false));
        // Sin runtime, el anuncio sale enseguida, después del puesto.
        assert_eq!(
            peer.taken(),
            [
                ControlMsg::Desk(Box::new(a.desk_sync())),
                ControlMsg::Capabilities(Box::new(here))
            ]
        );
        // Pedir lo mismo otra vez no cambia ni anuncia nada.
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
        // Va en el puesto, del lado de la otra; aquí no se apaga nada.
        assert_eq!(a.desk().modules.get(other, "clipboard"), Some(false));
        assert!(a.module_on(ModuleId::Clipboard));
        assert_eq!(peer.taken(), [ControlMsg::Desk(Box::new(a.desk_sync()))]);
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

    /// Lo que la UI de la otra apagó aquí llega con su puesto y se aplica.
    #[test]
    fn a_desk_from_the_peer_switches_the_modules_here() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_on(dir.path(), Os::Windows);
        let me = a.machine();
        let mut theirs = a.desk_sync();
        theirs.stamp = theirs.stamp.next(me.other(), 1);
        theirs.desk.modules.set(me, "input", false);
        // Lo que no se puede cambiar se ignora.
        theirs.desk.modules.set(me, "power", false);
        a.receive_desk(theirs.clone());
        assert!(!a.module_on(ModuleId::Input));
        assert!(a.module_on(ModuleId::Power));
        assert_eq!(
            state(&a.capabilities().here, ModuleId::Input),
            ModuleState::Off
        );
        // Y vuelve a encenderse con el siguiente.
        theirs.stamp = theirs.stamp.next(me.other(), 2);
        theirs.desk.modules.set(me, "input", true);
        a.receive_desk(theirs);
        assert!(a.module_on(ModuleId::Input));
    }

    /// Un `EditDesk` con un módulo pasa por las mismas comprobaciones que `SetModule`.
    #[test]
    fn a_module_patch_is_checked_like_set_module() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent_on(dir.path(), Os::MacOs));
        let edit = |module: &str, on| {
            a.handle(UiCommand::EditDesk {
                patch: DeskPatch::Module {
                    machine: a.machine(),
                    module: module.into(),
                    on,
                },
            })
        };
        assert!(matches!(edit("clipboard", false), Reply::Desk(_)));
        assert!(!a.module_on(ModuleId::Clipboard));
        assert!(matches!(edit("secure_desktop", true), Reply::Rejected(_)));
        assert!(matches!(edit("nada", true), Reply::Rejected(_)));
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
        assert!(peer.announced().is_empty(), "todavía dentro de la ventana");
        tokio::time::sleep(capabilities::ANNOUNCE_DEBOUNCE).await;
        let sent = peer.announced();
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
            peer.announced(),
            [ControlMsg::Capabilities(Box::new(a.capabilities().here))]
        );
    }
}
