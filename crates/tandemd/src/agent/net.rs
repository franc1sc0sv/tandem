//! Network metrics in the agent (T-102): what the Network screen and the Desk summary show.
//!
//! The metrics live in the agent and not in the link manager: the network rebuilds the manager
//! when the discovered paths change, and the history and path changes have
//! to survive that. The network ([`crate::network`]) feeds them with
//! [`tandem_link::metrics::follow`] and leaves here the connection of each healthy path, to measure
//! the bandwidth of any, not only the active one.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use tandem_link::manager::LinkKind;
use tandem_link::metrics::Metrics;
use tandem_link::transport::Connection;
use tandem_proto::{Link, NetworkStatus, Reply};

use super::Agent;
use crate::peer;

#[derive(Default)]
pub(super) struct NetState {
    metrics: Metrics,
    conns: Mutex<BTreeMap<LinkKind, Connection>>,
}

impl std::fmt::Debug for NetState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetState")
            .field("metrics", &self.metrics)
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// The network metrics, for the network to feed.
    pub fn net_metrics(&self) -> &Metrics {
        &self.net.metrics
    }

    /// What the Network screen sees ([`tandem_proto::AgentToUi::Network`]).
    pub fn network_status(&self) -> NetworkStatus {
        self.net.metrics.status()
    }

    /// The connection of each path that answers now (the network updates it with each snapshot
    /// of the link manager).
    pub fn set_path_conns(&self, conns: BTreeMap<LinkKind, Connection>) {
        *self
            .net
            .conns
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = conns;
    }

    fn path_conn(&self, kind: LinkKind) -> Option<Connection> {
        self.net
            .conns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&kind)
            .cloned()
    }

    /// [`tandem_proto::UiCommand::MeasureLink`]: the bandwidth test over `path`.
    /// What is measured stays in the metrics and in that path's data cap (T-101).
    pub async fn measure_link(&self, path: Link) -> Reply {
        let kind = LinkKind::from(path);
        // The test sends uncapped data for 3 s: over Wi-Fi it would add latency to the
        // input. The path's label counts, which is the worse of the two ends (T-094).
        let label = self.link_qos().get(kind).link().unwrap_or(kind);
        if !label.is_wired() {
            return Reply::Rejected(format!(
                "Bandwidth is not measured over {label}: the test saturates the link and is only done over a cable"
            ));
        }
        let Some(conn) = self.path_conn(kind) else {
            return Reply::Rejected(format!("No connection to the other machine over {kind}"));
        };
        let Some(_measuring) = self.net.metrics.begin_measure(kind) else {
            return Reply::Rejected("A path is already being measured".into());
        };
        tracing::info!(link = %kind, "measuring the bandwidth");
        match peer::measure(&conn, self.machine()).await {
            Ok(bytes_per_sec) => {
                tracing::info!(link = %kind, bytes_per_sec, "bandwidth measured");
                self.net
                    .metrics
                    .record_bandwidth(kind, bytes_per_sec, self.link_qos());
                Reply::Bandwidth {
                    link: path,
                    bytes_per_sec,
                }
            }
            Err(e) => {
                tracing::warn!(link = %kind, "could not measure the bandwidth: {e}");
                Reply::Rejected(format!("Could not measure {kind}: {e}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use std::time::Duration;

    use tandem_core::Machine;
    use tandem_link::identity::{Identity, PublicKey};
    use tandem_link::transport::{Endpoint, PeerTrust};
    use tandem_proto::PathState;

    use super::*;
    use crate::config::fixtures;
    use crate::store::StateStore;

    struct Anyone;

    impl PeerTrust for Anyone {
        fn is_trusted(&self, _: &PublicKey) -> bool {
            true
        }
    }

    fn agent(dir: &std::path::Path, machine: Machine) -> Arc<Agent> {
        let mut cfg = fixtures::desk();
        cfg.machine = machine;
        Arc::new(Agent::new(
            cfg,
            StateStore::new(dir.join(format!("{machine}.json"))),
        ))
    }

    #[tokio::test]
    async fn without_a_connection_over_that_path_it_is_not_measured() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path(), Machine::A);
        let Reply::Rejected(why) = a.measure_link(Link::Ethernet).await else {
            panic!("a rejection was expected");
        };
        assert!(why.contains("Ethernet"), "{why}");
        assert_eq!(a.network_status().measuring, None);
        // All paths appear, even without a network.
        let s = a.network_status();
        assert_eq!(s.paths.len(), 3);
        assert!(s.paths.iter().all(|p| p.state == PathState::Unplugged));
    }

    #[tokio::test]
    async fn over_wifi_it_is_not_measured_even_with_a_connection() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path(), Machine::A);
        let Reply::Rejected(why) = a.measure_link(Link::Wifi).await else {
            panic!("a rejection was expected");
        };
        assert!(why.contains("Wi-Fi") && why.contains("cable"), "{why}");
        assert_eq!(a.network_status().measuring, None);
        assert_eq!(a.link_qos().get(LinkKind::Wifi).measured(), None);
    }

    /// A label worse than its own (the other end sees Wi-Fi) is not measured either.
    #[tokio::test]
    async fn if_the_other_end_sees_it_over_wifi_the_cable_is_not_measured_either() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path(), Machine::A);
        a.link_qos()
            .get(LinkKind::Ethernet)
            .set_link(Some(tandem_link::iface::worst(
                LinkKind::Ethernet,
                LinkKind::Wifi,
            )));
        let Reply::Rejected(why) = a.measure_link(Link::Ethernet).await else {
            panic!("a rejection was expected");
        };
        assert!(why.contains("Wi-Fi"), "{why}");
    }

    /// PC with Ethernet, Mac with Wi-Fi: each end announces its classification when opening the
    /// connection and both labels are the worse, Wi-Fi: no data plane over the cable.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn different_classifications_give_the_worse() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, pc) = (agent(dir.path(), Machine::A), agent(dir.path(), Machine::B));
        let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let ep_a =
            Endpoint::bind(loopback, &Identity::from_seed([1; 32]), Arc::new(Anyone)).unwrap();
        let ep_b =
            Endpoint::bind(loopback, &Identity::from_seed([2; 32]), Arc::new(Anyone)).unwrap();
        let to_b = ep_b.local_addr().unwrap();
        let (conn_a, conn_b) = tokio::join!(ep_a.connect(to_b), ep_b.accept());
        // The Mac (A) sees the path as Wi-Fi and the PC (B) as Ethernet.
        let conn_a = conn_a.unwrap().with_qos(mac.link_qos().get(LinkKind::Wifi));
        let conn_b = conn_b
            .unwrap()
            .unwrap()
            .with_qos(pc.link_qos().get(LinkKind::Ethernet));
        tokio::spawn(peer::accept_streams(conn_b, pc.clone()));

        let client = crate::PeerClient::open(&conn_a, Machine::A).await.unwrap();
        client.send(tandem_proto::ControlMsg::LinkClass { kind: Link::Wifi });
        let pc_qos = pc.link_qos().get(LinkKind::Ethernet);
        tokio::time::timeout(Duration::from_secs(5), async {
            while pc_qos.link() != Some(LinkKind::Wifi) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the PC did not adopt the worse label");

        // Without a data plane over the cable: it is neither measured nor carries a cable's cap.
        let Reply::Rejected(why) = pc.measure_link(Link::Ethernet).await else {
            panic!("a rejection was expected");
        };
        assert!(why.contains("Wi-Fi"), "{why}");
        // If the Mac announces Ethernet again, the label recovers: the other's class
        // is stored apart and is not sticky.
        let again = crate::PeerClient::open(&conn_a, Machine::A).await.unwrap();
        again.send(tandem_proto::ControlMsg::LinkClass {
            kind: Link::Ethernet,
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while pc_qos.link() != Some(LinkKind::Ethernet) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the label did not recover");
        // The Mac already sees it so by its own classification.
        assert_eq!(
            mac.link_qos().get(LinkKind::Wifi).link(),
            Some(LinkKind::Wifi)
        );
    }

    /// Two agents over real QUIC on loopback (ephemeral ports): the measurement goes over the
    /// requested path's connection, the other agent answers it and it stays in the metrics and in
    /// that path's `Qos`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_measurement_goes_over_the_requested_path_and_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (agent(dir.path(), Machine::A), agent(dir.path(), Machine::B));
        let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let ep_a =
            Endpoint::bind(loopback, &Identity::from_seed([1; 32]), Arc::new(Anyone)).unwrap();
        let ep_b =
            Endpoint::bind(loopback, &Identity::from_seed([2; 32]), Arc::new(Anyone)).unwrap();
        let to_b = ep_b.local_addr().unwrap();
        let (conn_a, conn_b) = tokio::join!(ep_a.connect(to_b), ep_b.accept());
        let (conn_a, conn_b) = (conn_a.unwrap(), conn_b.unwrap().unwrap());
        tokio::spawn(peer::accept_streams(conn_b, b));
        a.set_path_conns(BTreeMap::from([(LinkKind::Ethernet, conn_a)]));

        let reply = tokio::time::timeout(peer::MEASURE_TIMEOUT, a.measure_link(Link::Ethernet))
            .await
            .unwrap();
        let Reply::Bandwidth {
            link: Link::Ethernet,
            bytes_per_sec,
        } = reply
        else {
            panic!("{reply:?}");
        };
        assert!(bytes_per_sec > 1_000_000, "{bytes_per_sec}");
        let s = a.network_status();
        assert_eq!(s.measuring, None);
        assert_eq!(
            s.path(Link::Ethernet)
                .and_then(|p| p.bandwidth)
                .map(|b| b.bytes_per_sec),
            Some(bytes_per_sec)
        );
        assert_eq!(
            a.link_qos().get(LinkKind::Ethernet).measured(),
            Some(bytes_per_sec)
        );
    }
}
