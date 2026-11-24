//! A pairing offer from another machine while the paired one is silent (T-345).
//!
//! After a reinstall that kept the old data (or a reset on the other machine) this agent can
//! trust a key that no longer exists: the peer never comes back and, with a peer in
//! trust, the UI could not pair again. When the peer has been without a link for
//! [`DEFAULT_STALE_AFTER`] and some other machine announces itself for pairing on the LAN,
//! the agent publishes [`PairingStatus::Offered`](tandem_proto::PairingStatus::Offered); the
//! UI then offers to forget the old pair and pair with the new one. Nothing is
//! forgotten here: that is still the user's decision ([`UiCommand::Unpair`]).
//!
//! [`UiCommand::Unpair`]: tandem_proto::UiCommand::Unpair

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tandem_proto::PeerStatus;
use tokio::sync::watch;

use crate::agent::Agent;

/// How long the peer must go without a link before another machine's offer is shown. Long
/// enough for a normal reconnection (a reboot, Wi-Fi roaming) not to show it.
pub const DEFAULT_STALE_AFTER: Duration = Duration::from_secs(20);

/// The paired peer exists but no link sees it.
fn silent(status: &PeerStatus) -> bool {
    matches!(status, PeerStatus::Paired { link: None, .. })
}

/// Keeps [`Agent::set_pairing_offer`] up to date until the peer is forgotten (the task is
/// aborted) or `offers` closes. `offers` carries the addresses of the machines that
/// announce themselves for pairing and are not the paired peer.
pub async fn watch(
    agent: Arc<Agent>,
    mut offers: watch::Receiver<Vec<SocketAddr>>,
    stale_after: Duration,
) {
    let mut peer = agent.subscribe_peer();
    loop {
        if peer.wait_for(silent).await.is_err() {
            return;
        }
        // A short drop is not a stale pair: the link must stay down for `stale_after`.
        let stayed = tokio::select! {
            () = tokio::time::sleep(stale_after) => true,
            back = peer.wait_for(|p| !silent(p)) => {
                if back.is_err() {
                    return;
                }
                false
            }
        };
        if !stayed {
            continue;
        }
        loop {
            let offered = !offers.borrow_and_update().is_empty();
            agent.set_pairing_offer(offered);
            tokio::select! {
                changed = offers.changed() => {
                    if changed.is_err() {
                        agent.set_pairing_offer(false);
                        return;
                    }
                }
                back = peer.wait_for(|p| !silent(p)) => {
                    if back.is_err() {
                        return;
                    }
                    break;
                }
            }
        }
        agent.set_pairing_offer(false);
    }
}

#[cfg(test)]
mod tests {
    use tandem_proto::{Link, PairingStatus};

    use super::*;
    use crate::config::fixtures;
    use crate::store::StateStore;

    const AFTER: Duration = Duration::from_secs(20);

    struct Rig {
        agent: Arc<Agent>,
        offers: watch::Sender<Vec<SocketAddr>>,
        _dir: tempfile::TempDir,
    }

    fn rig(peer: PeerStatus) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(Agent::new(
            fixtures::desk(),
            StateStore::new(dir.path().join("state.json")),
        ));
        agent.set_peer_status(peer);
        let (offers, rx) = watch::channel(Vec::new());
        tokio::spawn(watch(agent.clone(), rx, AFTER));
        Rig {
            agent,
            offers,
            _dir: dir,
        }
    }

    fn silent_peer() -> PeerStatus {
        PeerStatus::Paired {
            name: "Mac".into(),
            link: None,
        }
    }

    fn live_peer() -> PeerStatus {
        PeerStatus::Paired {
            name: "Mac".into(),
            link: Some(Link::Wifi),
        }
    }

    fn offer() -> Vec<SocketAddr> {
        vec!["192.168.1.30:47771".parse().unwrap()]
    }

    async fn settle() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_stale_peer_and_an_unknown_announcement_is_an_offer() {
        let r = rig(silent_peer());
        r.offers.send_replace(offer());
        // Not yet stale: the peer could be about to come back.
        tokio::time::sleep(AFTER / 2).await;
        assert_eq!(r.agent.pairing_status(), PairingStatus::Idle);
        tokio::time::sleep(AFTER).await;
        assert_eq!(r.agent.pairing_status(), PairingStatus::Offered);
        // The announcement goes away: so does the offer.
        r.offers.send_replace(Vec::new());
        settle().await;
        assert_eq!(r.agent.pairing_status(), PairingStatus::Idle);
        // And it comes back.
        r.offers.send_replace(offer());
        settle().await;
        assert_eq!(r.agent.pairing_status(), PairingStatus::Offered);
    }

    #[tokio::test(start_paused = true)]
    async fn a_stale_peer_without_announcements_offers_nothing() {
        let r = rig(silent_peer());
        tokio::time::sleep(AFTER * 3).await;
        assert_eq!(r.agent.pairing_status(), PairingStatus::Idle);
    }

    #[tokio::test(start_paused = true)]
    async fn a_peer_with_a_link_never_gets_an_offer() {
        let r = rig(live_peer());
        r.offers.send_replace(offer());
        tokio::time::sleep(AFTER * 3).await;
        assert_eq!(r.agent.pairing_status(), PairingStatus::Idle);
    }

    #[tokio::test(start_paused = true)]
    async fn a_peer_that_comes_back_withdraws_the_offer() {
        let r = rig(silent_peer());
        r.offers.send_replace(offer());
        tokio::time::sleep(AFTER * 2).await;
        assert_eq!(r.agent.pairing_status(), PairingStatus::Offered);
        r.agent.set_peer_status(live_peer());
        settle().await;
        assert_eq!(r.agent.pairing_status(), PairingStatus::Idle);
        // A short drop afterwards is not stale yet.
        r.agent.set_peer_status(silent_peer());
        tokio::time::sleep(AFTER / 2).await;
        assert_eq!(r.agent.pairing_status(), PairingStatus::Idle);
    }

    #[tokio::test(start_paused = true)]
    async fn a_short_drop_does_not_count_as_stale() {
        let r = rig(live_peer());
        r.offers.send_replace(offer());
        for _ in 0..5 {
            r.agent.set_peer_status(silent_peer());
            tokio::time::sleep(AFTER / 2).await;
            r.agent.set_peer_status(live_peer());
            tokio::time::sleep(AFTER / 4).await;
        }
        assert_eq!(r.agent.pairing_status(), PairingStatus::Idle);
    }

    /// A pairing in progress (or its result) is never overwritten by the offer.
    #[tokio::test(start_paused = true)]
    async fn other_pairing_statuses_are_left_alone() {
        let r = rig(silent_peer());
        r.agent.set_pairing_status(PairingStatus::Searching);
        r.offers.send_replace(offer());
        tokio::time::sleep(AFTER * 2).await;
        assert_eq!(r.agent.pairing_status(), PairingStatus::Searching);
        r.offers.send_replace(Vec::new());
        settle().await;
        assert_eq!(r.agent.pairing_status(), PairingStatus::Searching);
    }
}
