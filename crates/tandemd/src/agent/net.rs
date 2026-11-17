//! Métricas de red en el agente (T-102): lo que ve la pantalla Red y el resumen del Puesto.
//!
//! Las métricas viven en el agente y no en el gestor de enlaces: la red rearma el gestor
//! cuando cambian los caminos descubiertos, y el historial y los cambios de camino tienen
//! que sobrevivir a eso. La red ([`crate::network`]) las alimenta con
//! [`tandem_link::metrics::follow`] y deja aquí la conexión de cada camino sano, para medir
//! el ancho de banda de cualquiera, no solo del activo.

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
    /// Las métricas de red, para que la red las alimente.
    pub fn net_metrics(&self) -> &Metrics {
        &self.net.metrics
    }

    /// Lo que ve la pantalla Red ([`tandem_proto::AgentToUi::Network`]).
    pub fn network_status(&self) -> NetworkStatus {
        self.net.metrics.status()
    }

    /// La conexión de cada camino que responde ahora (la red la actualiza con cada foto
    /// del gestor de enlaces).
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

    /// [`tandem_proto::UiCommand::MeasureLink`]: la prueba de ancho de banda por `path`.
    /// Lo medido queda en las métricas y en el tope de los datos de ese camino (T-101).
    pub async fn measure_link(&self, path: Link) -> Reply {
        let kind = LinkKind::from(path);
        let Some(conn) = self.path_conn(kind) else {
            return Reply::Rejected(format!("Sin conexión con la otra máquina por {kind}"));
        };
        let Some(_measuring) = self.net.metrics.begin_measure(kind) else {
            return Reply::Rejected("Ya se está midiendo un camino".into());
        };
        tracing::info!(link = %kind, "midiendo el ancho de banda");
        match peer::measure(&conn, self.machine()).await {
            Ok(bytes_per_sec) => {
                tracing::info!(link = %kind, bytes_per_sec, "ancho de banda medido");
                self.net
                    .metrics
                    .record_bandwidth(kind, bytes_per_sec, self.link_qos());
                Reply::Bandwidth {
                    link: path,
                    bytes_per_sec,
                }
            }
            Err(e) => {
                tracing::warn!(link = %kind, "no se pudo medir el ancho de banda: {e}");
                Reply::Rejected(format!("No se pudo medir {kind}: {e}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::Arc;

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
    async fn sin_conexión_por_ese_camino_no_se_mide() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path(), Machine::A);
        let Reply::Rejected(why) = a.measure_link(Link::Ethernet).await else {
            panic!("se esperaba un rechazo");
        };
        assert!(why.contains("Ethernet"), "{why}");
        assert_eq!(a.network_status().measuring, None);
        // Todos los caminos aparecen, aunque no haya red.
        let s = a.network_status();
        assert_eq!(s.paths.len(), 3);
        assert!(s.paths.iter().all(|p| p.state == PathState::Unplugged));
    }

    /// Dos agentes por QUIC real en loopback (puertos efímeros): la medición va por la
    /// conexión del camino pedido, la contesta el otro agente y queda en las métricas y en
    /// el `Qos` de ese camino.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn la_medición_va_por_el_camino_pedido_y_queda_guardada() {
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
        a.set_path_conns(BTreeMap::from([(LinkKind::Wifi, conn_a)]));

        let reply = tokio::time::timeout(peer::MEASURE_TIMEOUT, a.measure_link(Link::Wifi))
            .await
            .unwrap();
        let Reply::Bandwidth {
            link: Link::Wifi,
            bytes_per_sec,
        } = reply
        else {
            panic!("{reply:?}");
        };
        assert!(bytes_per_sec > 1_000_000, "{bytes_per_sec}");
        let s = a.network_status();
        assert_eq!(s.measuring, None);
        assert_eq!(
            s.path(Link::Wifi)
                .and_then(|p| p.bandwidth)
                .map(|b| b.bytes_per_sec),
            Some(bytes_per_sec)
        );
        assert_eq!(
            a.link_qos().get(LinkKind::Wifi).measured(),
            Some(bytes_per_sec)
        );
    }
}
