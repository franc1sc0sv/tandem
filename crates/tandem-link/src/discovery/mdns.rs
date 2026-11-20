use std::net::{SocketAddr, SocketAddrV6};

use mdns_sd::{ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::{
    Announcement, DiscoveryError, DiscoveryEvent, Fingerprint, PeerResolver, TXT_FINGERPRINT,
};
use crate::identity::PublicKey;

/// DNS-SD service type that each agent announces.
pub const SERVICE_TYPE: &str = "_tandem._udp.local.";

/// What an agent announces only while pairing, with the port of its pairing
/// endpoint. It is a different type so nobody confuses it with a peer's.
pub const PAIRING_SERVICE_TYPE: &str = "_tandem-pair._udp.local.";

/// Own announcement: instance and host `tandem-<short fingerprint>`, the agent's QUIC
/// port and the full fingerprint in the TXT key `fp`. The addresses are
/// set by mdns-sd according to the interfaces there are at any moment.
pub fn service_info(own: &Fingerprint, port: u16) -> Result<ServiceInfo, DiscoveryError> {
    service_info_of(SERVICE_TYPE, own, port)
}

fn service_info_of(
    service_type: &str,
    own: &Fingerprint,
    port: u16,
) -> Result<ServiceInfo, DiscoveryError> {
    let name = format!("tandem-{}", own.short());
    let host = format!("{name}.local.");
    let txt = [(TXT_FINGERPRINT, own.to_hex())];
    let info = ServiceInfo::new(service_type, &name, &host, "", port, &txt[..])?;
    Ok(info.enable_addr_auto())
}

/// Turns a service resolved by mdns-sd into an [`Announcement`].
pub fn announcement(service: &ResolvedService) -> Announcement {
    let port = service.get_port();
    let mut addrs: Vec<SocketAddr> = service
        .get_addresses()
        .iter()
        .map(|ip| match ip {
            // A link-local IPv6 is useless without the interface it arrived on.
            ScopedIp::V6(v6) if v6.addr().is_unicast_link_local() => {
                SocketAddrV6::new(*v6.addr(), port, 0, v6.scope_id().index).into()
            }
            other => SocketAddr::new(other.to_ip_addr(), port),
        })
        .collect();
    addrs.sort();
    Announcement {
        instance: service.get_fullname().to_owned(),
        fingerprint: service
            .get_property_val_str(TXT_FINGERPRINT)
            .map(str::to_owned),
        addrs,
    }
}

/// What matters of a browser event; the rest is ignored.
pub fn discovery_event(event: ServiceEvent) -> Option<DiscoveryEvent> {
    match event {
        ServiceEvent::ServiceResolved(s) => Some(DiscoveryEvent::Resolved(announcement(&s))),
        ServiceEvent::ServiceRemoved(_, instance) => Some(DiscoveryEvent::Removed { instance }),
        _ => None,
    }
}

/// Own announcement and search for the peer on the LAN over mDNS.
///
/// The peer's addresses are published in [`MdnsDiscovery::peer_addrs`],
/// from most to least preferred. Dropping it withdraws the announcement.
pub struct MdnsDiscovery {
    daemon: ServiceDaemon,
    peers: watch::Receiver<Vec<SocketAddr>>,
    task: JoinHandle<()>,
}

impl MdnsDiscovery {
    /// Announces `own` on `port` and looks for the peers in `trusted` (those of the
    /// trust store). It has to be called inside a tokio runtime.
    pub fn start(
        own: &PublicKey,
        port: u16,
        trusted: impl IntoIterator<Item = PublicKey>,
    ) -> Result<Self, DiscoveryError> {
        let own = Fingerprint::of(own);
        let resolver = PeerResolver::new(own, trusted.into_iter().map(|k| Fingerprint::of(&k)));
        Self::run(SERVICE_TYPE, own, Some(port), resolver)
    }

    /// For pairing: announces the pairing endpoint on `port` (if any) and
    /// looks for any other agent that is pairing ([`PAIRING_SERVICE_TYPE`]).
    /// It has to be called inside a tokio runtime.
    pub fn pairing(own: &PublicKey, port: Option<u16>) -> Result<Self, DiscoveryError> {
        let own = Fingerprint::of(own);
        Self::run(PAIRING_SERVICE_TYPE, own, port, PeerResolver::any(own))
    }

    /// Only looks (announces nothing) for agents that are pairing and whose key is not in
    /// `known`: a machine that is not the paired peer offering to pair (T-345).
    pub fn pairing_offers(
        own: &PublicKey,
        known: impl IntoIterator<Item = PublicKey>,
    ) -> Result<Self, DiscoveryError> {
        let own = Fingerprint::of(own);
        let known = known.into_iter().map(|k| Fingerprint::of(&k));
        Self::run(
            PAIRING_SERVICE_TYPE,
            own,
            None,
            PeerResolver::any_except(own, known),
        )
    }

    fn run(
        service_type: &str,
        own: Fingerprint,
        port: Option<u16>,
        mut resolver: PeerResolver,
    ) -> Result<Self, DiscoveryError> {
        let daemon = ServiceDaemon::new()?;
        if let Some(port) = port {
            daemon.register(service_info_of(service_type, &own, port)?)?;
        }
        let browse = daemon.browse(service_type)?;
        let (tx, peers) = watch::channel(Vec::new());
        let task = tokio::spawn(async move {
            // It ends when the daemon shuts down and closes the channel.
            while let Ok(event) = browse.recv_async().await {
                let Some(event) = discovery_event(event) else {
                    continue;
                };
                if resolver.apply(event) {
                    let addrs = resolver.addrs();
                    tracing::info!(?addrs, "peer addresses on the LAN");
                    tx.send_replace(addrs);
                }
            }
        });
        Ok(Self {
            daemon,
            peers,
            task,
        })
    }

    pub fn peer_addrs(&self) -> watch::Receiver<Vec<SocketAddr>> {
        self.peers.clone()
    }
}

impl Drop for MdnsDiscovery {
    fn drop(&mut self) {
        // `shutdown` sends the goodbye (TTL 0) for what was registered before stopping.
        if let Err(e) = self.daemon.shutdown() {
            tracing::debug!("could not shut down mDNS: {e}");
        }
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::identity::Identity;

    fn key(seed: u8) -> PublicKey {
        Identity::from_seed([seed; 32]).public_key()
    }

    #[test]
    fn the_announcement_carries_the_fingerprint_and_the_port() {
        let fp = Fingerprint::of(&key(1));
        let info = service_info(&fp, 47770).unwrap();
        assert_eq!(info.get_type(), SERVICE_TYPE);
        assert_eq!(info.get_port(), 47770);
        assert_eq!(
            info.get_fullname(),
            format!("tandem-{}.{SERVICE_TYPE}", fp.short())
        );
        assert_eq!(info.get_hostname(), format!("tandem-{}.local.", fp.short()));
        let fp_txt = fp.to_hex();
        assert_eq!(
            info.get_property_val_str(TXT_FINGERPRINT),
            Some(fp_txt.as_str())
        );
        assert!(info.is_addr_auto());
    }

    /// Round trip through the mdns-sd types: what one agent announces the
    /// other accepts, and a third does not.
    #[test]
    fn the_resolved_announcement_is_recognized_only_by_the_peer() {
        let (a, b, c) = (key(1), key(2), key(3));
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            "tandem-x",
            "tandem-x.local.",
            "192.168.1.20,10.77.1.2",
            47770,
            &[(TXT_FINGERPRINT, Fingerprint::of(&a).to_hex())][..],
        )
        .unwrap();
        let event = ServiceEvent::ServiceResolved(Box::new(info.as_resolved_service()));
        let Some(DiscoveryEvent::Resolved(ann)) = discovery_event(event) else {
            panic!("a resolved announcement was expected");
        };
        assert_eq!(ann.instance, format!("tandem-x.{SERVICE_TYPE}"));
        assert_eq!(
            ann.addrs,
            vec![
                "10.77.1.2:47770".parse().unwrap(),
                "192.168.1.20:47770".parse().unwrap()
            ]
        );

        let resolver = PeerResolver::new(Fingerprint::of(&b), [Fingerprint::of(&a)]);
        assert_eq!(resolver.accepts(&ann), Some(Fingerprint::of(&a)));
        let tercero = PeerResolver::new(Fingerprint::of(&c), [Fingerprint::of(&b)]);
        assert_eq!(tercero.accepts(&ann), None);
        let itself = PeerResolver::new(Fingerprint::of(&a), [Fingerprint::of(&b)]);
        assert_eq!(itself.accepts(&ann), None);
    }

    #[test]
    fn withdrawal_and_ignored_events() {
        assert_eq!(
            discovery_event(ServiceEvent::ServiceRemoved(
                SERVICE_TYPE.into(),
                "tandem-x._tandem._udp.local.".into()
            )),
            Some(DiscoveryEvent::Removed {
                instance: "tandem-x._tandem._udp.local.".into()
            })
        );
        assert_eq!(
            discovery_event(ServiceEvent::SearchStarted(SERVICE_TYPE.into())),
            None
        );
        assert_eq!(
            discovery_event(ServiceEvent::ServiceFound(
                SERVICE_TYPE.into(),
                "tandem-x._tandem._udp.local.".into()
            )),
            None
        );
    }

    /// Two agents on the same machine find each other over real multicast.
    /// It needs an interface with multicast (not guaranteed in CI and in a sandbox);
    /// the test between the Mac and the PC is the real one.
    #[tokio::test]
    #[ignore = "hardware"]
    async fn two_agents_find_each_other_on_the_lan() {
        let (a, b) = (key(1), key(2));
        let disc_a = MdnsDiscovery::start(&a, 47771, [b]).unwrap();
        let db = MdnsDiscovery::start(&b, 47772, [a]).unwrap();
        let mut wa = disc_a.peer_addrs();
        let found = tokio::time::timeout(
            Duration::from_secs(10),
            wa.wait_for(|addrs| !addrs.is_empty()),
        )
        .await
        .expect("the peer was not found")
        .unwrap()
        .clone();
        assert!(found.iter().all(|a| a.port() == 47772), "{found:?}");
        drop(db);
    }
}
