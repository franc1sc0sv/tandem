//! What the agent does to update both machines (T-122, DESIGN §13).
//!
//! The agent downloads and installs nothing: the UI does that with Tauri's updater. Here it
//! only tracks which version each machine runs and passes the "install this version" request
//! from one UI to the other machine's.

use tandem_proto::{ControlMsg, PeerStatus, Reply, UpdateMsg, UpdateStatus};
use tokio::sync::watch;

use super::Agent;

/// What [`tandem_proto::UiCommand::UpdatePeer`] answers without the other machine in view.
pub const PEER_UNREACHABLE: &str = "the other machine is not connected";

impl Agent {
    pub fn update_status(&self) -> UpdateStatus {
        self.update.borrow().clone()
    }

    /// Notifies every time some version changes or an update request arrives.
    pub fn subscribe_update(&self) -> watch::Receiver<UpdateStatus> {
        self.update.subscribe()
    }

    /// What the other agent sent.
    pub fn receive_update(&self, msg: UpdateMsg) {
        self.update.send_if_modified(|u| match msg {
            UpdateMsg::Version(v) => {
                let changed = u.peer.as_deref() != Some(v.as_str()) || u.peer_protocol.is_some();
                u.peer = Some(v);
                // If they are understood, it no longer speaks another protocol.
                u.peer_protocol = None;
                changed
            }
            UpdateMsg::Install { version } => {
                tracing::info!(%version, "the other machine asks to install a new version");
                let changed = u.install.as_deref() != Some(version.as_str());
                u.install = Some(version);
                changed
            }
        });
    }

    /// The other agent connected with another protocol version: it is another Tandem version.
    /// While both update, it is the signal that the other came back with the new one even if
    /// it can no longer be spoken to.
    pub fn peer_protocol_mismatch(&self, theirs: u16) {
        self.update.send_if_modified(|u| {
            let changed = u.peer_protocol != Some(theirs);
            u.peer_protocol = Some(theirs);
            changed
        });
    }

    /// Asks the other machine to install `version`. That it comes back with it is told by its
    /// version announcement on reconnecting.
    pub(super) fn update_peer(&self, version: String) -> Reply {
        if !matches!(self.peer_status(), PeerStatus::Paired { link: Some(_), .. }) {
            return Reply::Rejected(PEER_UNREACHABLE.into());
        }
        tracing::info!(%version, "the other machine is asked to update");
        self.orch
            .peer()
            .send(ControlMsg::Update(UpdateMsg::Install { version }));
        Reply::Done
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tandem_core::{Machine, State, Step};
    use tandem_proto::{Link, UiCommand};

    use super::*;
    use crate::agent::VERSION;
    use crate::agent::tests::agent;
    use crate::orchestrator::{PeerLink, StepFuture};

    #[derive(Default)]
    struct Recorder(Mutex<Vec<ControlMsg>>);

    impl PeerLink for Recorder {
        fn execute<'a>(&'a self, _: &'a Step) -> StepFuture<'a> {
            Box::pin(async { Err("no steps".into()) })
        }
        fn publish_state(&self, _: &State) {}
        fn send(&self, msg: ControlMsg) {
            self.0.lock().unwrap().push(msg);
        }
    }

    fn paired(link: Option<Link>) -> PeerStatus {
        PeerStatus::Paired {
            name: "PC".into(),
            link,
        }
    }

    #[test]
    fn starts_with_its_own_version_and_nothing_from_the_peer() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        assert_eq!(
            a.update_status(),
            UpdateStatus {
                here: VERSION.into(),
                peer: None,
                install: None,
                peer_protocol: None,
            }
        );
    }

    #[test]
    fn announces_its_version_when_the_peer_connects() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        let peer = Arc::new(Recorder::default());
        a.set_peer(peer.clone());
        let sent = peer.0.lock().unwrap().clone();
        assert_eq!(
            sent.last(),
            Some(&ControlMsg::Update(UpdateMsg::Version(VERSION.into())))
        );
    }

    #[test]
    fn learns_the_peer_version_and_its_install_request() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        let mut rx = a.subscribe_update();
        a.receive_update(UpdateMsg::Version("0.3.0".into()));
        assert!(rx.has_changed().unwrap());
        assert_eq!(rx.borrow_and_update().peer.as_deref(), Some("0.3.0"));
        // The same again is not a change.
        a.receive_update(UpdateMsg::Version("0.3.0".into()));
        assert!(!rx.has_changed().unwrap());
        a.receive_update(UpdateMsg::Install {
            version: "0.3.1".into(),
        });
        assert_eq!(rx.borrow_and_update().install.as_deref(), Some("0.3.1"));
    }

    #[test]
    fn another_protocol_is_remembered_until_the_peer_speaks_ours() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        a.receive_update(UpdateMsg::Version("0.3.0".into()));
        a.peer_protocol_mismatch(99);
        assert_eq!(a.update_status().peer_protocol, Some(99));
        assert_eq!(a.update_status().peer.as_deref(), Some("0.3.0"));
        a.receive_update(UpdateMsg::Version("0.3.0".into()));
        assert_eq!(a.update_status().peer_protocol, None);
    }

    #[tokio::test]
    async fn update_peer_needs_the_peer_in_sight() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        let peer = Arc::new(Recorder::default());
        a.set_peer(peer.clone());
        peer.0.lock().unwrap().clear();
        let cmd = || UiCommand::UpdatePeer {
            version: "0.3.1".into(),
        };

        assert_eq!(a.handle(cmd()), Reply::Rejected(PEER_UNREACHABLE.into()));
        a.set_peer_status(paired(None));
        assert_eq!(a.handle(cmd()), Reply::Rejected(PEER_UNREACHABLE.into()));
        assert!(peer.0.lock().unwrap().is_empty());

        a.set_peer_status(paired(Some(Link::Wifi)));
        assert_eq!(a.handle(cmd()), Reply::Done);
        assert_eq!(
            peer.0.lock().unwrap().as_slice(),
            &[ControlMsg::Update(UpdateMsg::Install {
                version: "0.3.1".into()
            })]
        );
        // Asking the other does not touch what is known about this one.
        assert_eq!(a.update_status().install, None);
        assert_eq!(a.machine(), Machine::A);
    }
}
