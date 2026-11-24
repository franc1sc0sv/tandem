//! What the agent does for the initial setup wizard (DESIGN §5): the
//! "press a key" test, where the keyboard is and the link ping.

use tandem_core::Machine;
use tandem_proto::{ControlMsg, KeyProbeMsg, KeyProbeStatus, Reply};
use tokio::sync::watch;

use super::Agent;

impl Agent {
    // ---- "Press a key" ----

    pub fn key_probe_status(&self) -> KeyProbeStatus {
        *self.key_probe.borrow()
    }

    pub fn subscribe_key_probe(&self) -> watch::Receiver<KeyProbeStatus> {
        self.key_probe.subscribe()
    }

    /// Both machines listen (or stop listening).
    pub(super) fn key_probe(&self, on: bool) {
        self.key_probe.send_replace(if on {
            KeyProbeStatus {
                armed: true,
                ..Default::default()
            }
        } else {
            KeyProbeStatus::default()
        });
        let msg = if on {
            KeyProbeMsg::Arm
        } else {
            KeyProbeMsg::Disarm
        };
        self.orch.peer().send(ControlMsg::KeyProbe(msg));
    }

    /// A link appeared: the test's notices that went out without one are repeated (what
    /// [`PeerLink::send`](crate::orchestrator::PeerLink::send) promises). Each side only has a
    /// link of its own once its manager installs it, which can be after the other side's: a key
    /// heard in between was never told. Both messages are idempotent on the receiving side.
    pub(super) fn replay_key_probe(&self, peer: &dyn crate::orchestrator::PeerLink) {
        let status = self.key_probe_status();
        if !status.armed {
            return;
        }
        peer.send(ControlMsg::KeyProbe(KeyProbeMsg::Armed));
        let me = self.machine();
        if status.heard == Some(me) || status.second == Some(me) {
            peer.send(ControlMsg::KeyProbe(KeyProbeMsg::Heard));
        }
    }

    /// The window here heard a key: the keyboard is on this machine.
    pub(super) fn heard_key(&self) {
        if self.hear(self.machine()) {
            self.orch
                .peer()
                .send(ControlMsg::KeyProbe(KeyProbeMsg::Heard));
        }
    }

    /// Records the first activity of each machine: the first one is `heard` (the keyboard
    /// question), the other one `second`. False if nobody is listening or it was already heard.
    fn hear(&self, machine: Machine) -> bool {
        self.key_probe.send_if_modified(|s| {
            if !s.armed || s.heard == Some(machine) || s.second == Some(machine) {
                false
            } else if s.heard.is_none() {
                s.heard = Some(machine);
                true
            } else {
                s.second = Some(machine);
                true
            }
        })
    }

    /// What the other machine sent for the test.
    pub fn receive_key_probe(&self, msg: KeyProbeMsg) {
        match msg {
            KeyProbeMsg::Arm => {
                self.key_probe.send_replace(KeyProbeStatus {
                    armed: true,
                    ..Default::default()
                });
            }
            KeyProbeMsg::Disarm => {
                self.key_probe.send_replace(KeyProbeStatus::default());
            }
            KeyProbeMsg::Armed => {
                self.key_probe.send_if_modified(|s| {
                    let was = s.armed;
                    s.armed = true;
                    !was
                });
            }
            KeyProbeMsg::Heard => {
                self.hear(self.machine().other());
            }
        }
    }

    // ---- Where the keyboard is ----

    /// Notifies when the user says where the keyboard is (or the configuration said so).
    pub fn subscribe_keyboard(&self) -> watch::Receiver<Option<Machine>> {
        self.keyboard.subscribe()
    }

    pub fn keyboard_declared(&self) -> Option<Machine> {
        *self.keyboard.borrow()
    }

    /// The user confirmed where the keyboard and mouse are: it is saved, the
    /// other machine is told and the input engine adopts it.
    pub(super) fn set_keyboard(&self, machine: Machine) {
        self.declare_keyboard(machine);
        self.orch.peer().send(ControlMsg::Keyboard(machine));
    }

    /// The same, said by the other machine.
    pub fn receive_keyboard(&self, machine: Machine) {
        self.declare_keyboard(machine);
    }

    fn declare_keyboard(&self, machine: Machine) {
        let editing = self.edit();
        if let Some(path) = self.config_path()
            && let Err(e) = crate::config_edit::set_keyboard(&path, machine)
        {
            tracing::warn!(
                "where the keyboard is was not saved to {}: {e}",
                path.display()
            );
        }
        self.keyboard.send_replace(Some(machine));
        drop(editing);
        // With an input engine, its state task listens to the declaration and publishes the
        // state; without it (tests, simulated backends) it is updated here.
        if !self.input_engine.load(std::sync::atomic::Ordering::Acquire) {
            let mut status = self.input_status();
            status.server = machine;
            status.defined = true;
            self.set_input_status(status);
        }
    }

    /// There is an input engine building the state: [`Agent::subscribe_keyboard`] wakes it.
    pub fn set_input_engine(&self) {
        self.input_engine
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// The link latency with the other machine, to show it in the wizard.
    pub(super) fn ping_peer(&self) -> Reply {
        match self.orch.peer().rtt() {
            Some((link, rtt)) => Reply::Pong {
                link,
                millis: u32::try_from(rtt.as_millis().max(1)).unwrap_or(u32::MAX),
            },
            None => Reply::Rejected("no link sees the other machine".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::config::AgentConfig;
    use crate::store::StateStore;

    /// A freshly installed PC agent, with its `tandem.toml` generated.
    fn fresh(dir: &std::path::Path) -> Arc<Agent> {
        let text = crate::autoconfig::generate(Machine::B);
        let file = dir.join("tandem.toml");
        std::fs::write(&file, &text).unwrap();
        let cfg = AgentConfig::from_toml(&text).unwrap();
        let agent = Agent::new(cfg, StateStore::new(dir.join("state.json")));
        agent.set_config_file(file);
        Arc::new(agent)
    }

    #[test]
    fn a_clean_install_has_an_empty_desk() {
        let dir = tempfile::tempdir().unwrap();
        let a = fresh(dir.path());
        assert_eq!(a.desk(), tandem_core::Config::default());
        assert!(!a.input_status().defined);
        assert_eq!(a.desk_sync().stamp.rev, 0);
    }

    #[tokio::test]
    async fn without_a_link_there_is_no_ping() {
        let dir = tempfile::tempdir().unwrap();
        let a = fresh(dir.path());
        assert!(matches!(
            a.handle(tandem_proto::UiCommand::PingPeer),
            Reply::Rejected(_)
        ));
    }

    #[tokio::test]
    async fn the_keyboard_is_undefined_until_someone_says_and_never_assumed_mac() {
        let dir = tempfile::tempdir().unwrap();
        let a = fresh(dir.path());
        assert!(!a.input_status().defined);
        assert_eq!(a.keyboard_declared(), None);
        // Listening without a key arriving defines nothing.
        a.handle(tandem_proto::UiCommand::KeyProbe { on: true });
        assert_eq!(a.key_probe_status().heard, None);
        // A key without listening is ignored.
        a.handle(tandem_proto::UiCommand::KeyProbe { on: false });
        a.handle(tandem_proto::UiCommand::HeardKey);
        assert_eq!(a.key_probe_status(), KeyProbeStatus::default());
        a.handle(tandem_proto::UiCommand::KeyProbe { on: true });
        a.handle(tandem_proto::UiCommand::HeardKey);
        assert_eq!(a.key_probe_status().heard, Some(Machine::B));
        a.handle(tandem_proto::UiCommand::SetKeyboard(Machine::B));
        assert!(a.input_status().defined);
        let saved = AgentConfig::load(&dir.path().join("tandem.toml")).unwrap();
        assert_eq!(saved.input.keyboard, Some(Machine::B));
    }

    /// What the agent sent to its peer, to see which notices a new link brings.
    #[derive(Default)]
    struct Recorder(std::sync::Mutex<Vec<tandem_proto::ControlMsg>>);

    impl Recorder {
        fn probes(&self) -> Vec<KeyProbeMsg> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter_map(|m| match m {
                    ControlMsg::KeyProbe(p) => Some(*p),
                    _ => None,
                })
                .collect()
        }
    }

    impl crate::orchestrator::PeerLink for Recorder {
        fn execute<'a>(&'a self, _: &'a tandem_core::Step) -> crate::orchestrator::StepFuture<'a> {
            Box::pin(async { Ok(()) })
        }
        fn publish_state(&self, _: &tandem_core::State) {}
        fn send(&self, msg: ControlMsg) {
            self.0.lock().unwrap().push(msg);
        }
    }

    /// The race of the wizard test on CI: the other side's own link was not installed yet when
    /// its key was heard, so the notice went nowhere and nobody repeated it (T-240).
    #[tokio::test]
    async fn a_key_heard_before_the_link_is_told_when_it_appears() {
        let dir = tempfile::tempdir().unwrap();
        let b = fresh(dir.path());
        // The other side armed us over its own link; ours is not there yet.
        b.receive_key_probe(KeyProbeMsg::Arm);
        b.handle(tandem_proto::UiCommand::HeardKey);
        assert_eq!(b.key_probe_status().heard, Some(Machine::B));
        let link = Arc::new(Recorder::default());
        b.set_peer(link.clone());
        assert_eq!(link.probes(), [KeyProbeMsg::Armed, KeyProbeMsg::Heard]);
    }

    #[tokio::test]
    async fn a_new_link_says_nothing_about_a_test_that_is_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let a = fresh(dir.path());
        let link = Arc::new(Recorder::default());
        a.set_peer(link.clone());
        assert!(link.probes().is_empty());
        // Armed but nothing heard here: only that it listens.
        a.handle(tandem_proto::UiCommand::KeyProbe { on: true });
        let again = Arc::new(Recorder::default());
        a.set_peer(again.clone());
        assert_eq!(again.probes(), [KeyProbeMsg::Armed]);
    }

    #[tokio::test]
    async fn repeating_that_it_listens_keeps_what_was_heard() {
        let dir = tempfile::tempdir().unwrap();
        let a = fresh(dir.path());
        a.receive_key_probe(KeyProbeMsg::Arm);
        a.handle(tandem_proto::UiCommand::HeardKey);
        a.receive_key_probe(KeyProbeMsg::Armed);
        assert_eq!(a.key_probe_status().heard, Some(Machine::B));
        // And it arms whoever did not get the first notice, without inventing a key.
        let dir2 = tempfile::tempdir().unwrap();
        let c = fresh(dir2.path());
        c.receive_key_probe(KeyProbeMsg::Armed);
        assert!(c.key_probe_status().armed);
        assert_eq!(c.key_probe_status().heard, None);
        // A repeated `Heard` (the replay) does not count twice.
        c.receive_key_probe(KeyProbeMsg::Heard);
        c.receive_key_probe(KeyProbeMsg::Heard);
        assert_eq!(c.key_probe_status().heard, Some(Machine::A));
        assert_eq!(c.key_probe_status().second, None);
    }
}
