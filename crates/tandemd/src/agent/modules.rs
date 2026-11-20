//! This machine's modules in the agent (T-098): turning them on and off from the UI of
//! either machine and announcing them in the capabilities.
//!
//! Which module is on is part of the shared desk (`Desk.modules`, D4): it is changed
//! with a patch, saved in `tandem.toml` and reaches the other with the desk. Each agent
//! copies those of its side to its switches ([`Agent::sync_switches`]).

use tandem_core::{DeskPatch, Machine};
use tandem_proto::{Capabilities, ModuleId, Reply};
use tokio::sync::watch;

use super::Agent;
use crate::capabilities;
use crate::config::AgentConfig;
use crate::modules::{ModuleContext, ModuleSwitches, default_name};

/// What is off at startup: what the desk turns off on this machine (`[modules.<side>]`).
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
    /// What this machine announces right now, modules included.
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
        caps.local_network = self.local_network();
        self.add_environments(&mut caps);
        caps
    }

    /// Whether the user did not turn `id` off here.
    pub fn module_on(&self, id: ModuleId) -> bool {
        self.modules.borrow().is_on(id)
    }

    /// Notifies every time a module is turned on or off, or a reported problem changes.
    pub fn subscribe_modules(&self) -> watch::Receiver<ModuleSwitches> {
        self.modules.subscribe()
    }

    /// Turns a module of this machine on or off: a desk change. `Err` with the reason
    /// if it does not exist here or cannot be turned off.
    pub fn set_module(&self, id: ModuleId, on: bool) -> Result<(), String> {
        self.registry.check_switch(id)?;
        self.edit_desk(DeskPatch::Module {
            machine: self.machine(),
            module: id.as_str().to_owned(),
            on,
        })
        .map(|_| ())
    }

    /// A running module reports that it cannot work (`Some`, with the reason for the UI) or
    /// that it now can (`None`).
    pub fn set_module_problem(&self, id: ModuleId, problem: Option<String>) {
        if self
            .modules
            .send_if_modified(|s| s.set_problem(id, problem))
        {
            self.refresh_capabilities();
        }
    }

    /// [`tandem_proto::UiCommand::SetModule`]: here it is applied; for the other it is checked against
    /// its last announcement and changed in the desk, which reaches it like any change. Its
    /// new state arrives with its next announcement.
    pub(super) fn switch_module(&self, machine: Machine, module: ModuleId, on: bool) -> Reply {
        if machine == self.machine() {
            return match self.set_module(module, on) {
                Ok(()) => Reply::Done,
                Err(why) => Reply::Rejected(why),
            };
        }
        let name = self.machine_name(machine);
        if !*self.orch.peer_connected().borrow() {
            return Reply::Rejected(format!("the {name} is not connected"));
        }
        let Some(peer) = self.capabilities().peer else {
            return Reply::Rejected(format!("it is not yet known which modules the {name} has"));
        };
        match peer.modules.iter().find(|m| m.id == module) {
            None => Reply::Rejected(format!(
                "\"{}\" does not exist on the {name}",
                default_name(module)
            )),
            Some(m) if !m.switchable => {
                Reply::Rejected(format!("\"{}\" cannot be turned on or off", m.name))
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

    /// A peer that keeps what it is sent.
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

        /// Only the capabilities announcements (the desk also travels with each change).
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
            .unwrap_or_else(|| panic!("missing {id:?}"))
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
        // It is a desk change (D4): it is saved, with a new version, and the other receives it.
        assert_eq!(a.desk().modules.get(a.machine(), "clipboard"), Some(false));
        // Without a runtime, the announcement goes out right away, after the desk.
        assert_eq!(
            peer.taken(),
            [
                ControlMsg::Desk(Box::new(a.desk_sync())),
                ControlMsg::Capabilities(Box::new(here))
            ]
        );
        // Asking for the same again neither changes nor announces anything.
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
            panic!("the secure desktop does not exist on macOS");
        };
        assert!(why.contains("does not exist"), "{why}");
        let Reply::Rejected(why) = a.handle(UiCommand::SetModule {
            machine: a.machine(),
            module: ModuleId::Tunnels,
            on: false,
        }) else {
            panic!("tunnels cannot be turned off from here");
        };
        assert!(why.contains("cannot"), "{why}");
    }

    #[test]
    fn a_reported_problem_is_announced_and_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_on(dir.path(), Os::Windows);
        a.set_module_problem(
            ModuleId::Clipboard,
            Some("Could not open the system clipboard".into()),
        );
        assert_eq!(
            state(&a.capabilities().here, ModuleId::Clipboard),
            ModuleState::Unavailable {
                reason: "Could not open the system clipboard".into()
            }
        );
        a.set_module_problem(ModuleId::Clipboard, None);
        assert_eq!(
            state(&a.capabilities().here, ModuleId::Clipboard),
            ModuleState::Active
        );
    }

    /// The other side, as if it had announced its modules.
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
        // Without a link there is nobody to ask.
        let Reply::Rejected(why) = set(ModuleId::Clipboard, false) else {
            panic!("without the other connected");
        };
        assert!(why.contains("is not connected"), "{why}");

        let peer = Arc::new(Recorder::default());
        a.set_peer(peer.clone());
        peer.taken();
        let Reply::Rejected(why) = set(ModuleId::Clipboard, false) else {
            panic!("without its announcement it is not known what it has");
        };
        assert!(why.contains("not yet known"), "{why}");

        learn_peer(&a, Os::Windows);
        // Learning of it records both machines in the desk, which also travels.
        peer.taken();
        assert_eq!(set(ModuleId::Clipboard, false), Reply::Done);
        // It goes in the desk, on the other's side; nothing is turned off here.
        assert_eq!(a.desk().modules.get(other, "clipboard"), Some(false));
        assert!(a.module_on(ModuleId::Clipboard));
        assert_eq!(peer.taken(), [ControlMsg::Desk(Box::new(a.desk_sync()))]);
        // What the other cannot turn off is not asked of it either.
        assert!(matches!(set(ModuleId::Power, false), Reply::Rejected(_)));
        assert!(peer.taken().is_empty());
        // And what does not exist on its system neither.
        learn_peer(&a, Os::MacOs);
        let Reply::Rejected(why) = set(ModuleId::SecureDesktop, true) else {
            panic!("a Mac has no secure desktop");
        };
        assert!(why.contains("does not exist"), "{why}");
    }

    /// What the other's UI turned off here arrives with its desk and is applied.
    #[test]
    fn a_desk_from_the_peer_switches_the_modules_here() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent_on(dir.path(), Os::Windows);
        let me = a.machine();
        let mut theirs = a.desk_sync();
        theirs.stamp = theirs.stamp.next(me.other(), 1);
        theirs.desk.modules.set(me, "input", false);
        // What cannot be changed is ignored.
        theirs.desk.modules.set(me, "power", false);
        a.receive_desk(theirs.clone());
        assert!(!a.module_on(ModuleId::Input));
        assert!(a.module_on(ModuleId::Power));
        assert_eq!(
            state(&a.capabilities().here, ModuleId::Input),
            ModuleState::Off
        );
        // And it turns on again with the next one.
        theirs.stamp = theirs.stamp.next(me.other(), 2);
        theirs.desk.modules.set(me, "input", true);
        a.receive_desk(theirs);
        assert!(a.module_on(ModuleId::Input));
    }

    /// An `EditDesk` with a module goes through the same checks as `SetModule`.
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
        assert!(matches!(edit("nothing", true), Reply::Rejected(_)));
    }

    /// Several consecutive changes reach the other machine as a single announcement, with the latest.
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
        assert!(peer.announced().is_empty(), "still inside the window");
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

    /// Finding from the review of PR #72: a change inside the window, with a link that
    /// appeared in the meantime, was sent to the peer from when the window opened (none).
    #[tokio::test(start_paused = true)]
    async fn a_change_in_the_window_reaches_a_peer_that_connected_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent_on(dir.path(), Os::Windows));
        a.set_module(ModuleId::Clipboard, false).unwrap(); // opens the window without a peer
        tokio::time::sleep(Duration::from_millis(50)).await;
        let peer = Arc::new(Recorder::default());
        a.set_peer(peer.clone());
        peer.taken();
        a.set_module(ModuleId::Input, false).unwrap(); // inside the window
        tokio::time::sleep(capabilities::ANNOUNCE_DEBOUNCE * 2).await;
        assert_eq!(
            peer.announced(),
            [ControlMsg::Capabilities(Box::new(a.capabilities().here))]
        );
    }
}
