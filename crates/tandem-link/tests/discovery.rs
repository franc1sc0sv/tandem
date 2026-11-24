//! The config's fixed addresses reach the link manager as
//! candidates, together with what is discovered on the LAN.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use tandem_link::discovery::{
    Announcement, DiscoveryEvent, Fingerprint, PeerResolver, link_candidates, validate_static_links,
};
use tandem_link::identity::Identity;
use tandem_link::manager::{HealthConfig, LinkCandidate, LinkKind, LinkManager, Prober};

/// Transport double: everything answers and it records which addresses
/// the manager connected to.
#[derive(Default)]
struct Recorder(Mutex<BTreeSet<SocketAddr>>);

struct FakeProber(Arc<Recorder>);

#[derive(Debug, thiserror::Error)]
#[error("never fails")]
struct Never;

impl Prober for FakeProber {
    type Session = SocketAddr;
    type Handle = SocketAddr;
    type Error = Never;

    async fn connect(&self, addr: SocketAddr) -> Result<SocketAddr, Never> {
        self.0.0.lock().unwrap().insert(addr);
        Ok(addr)
    }

    async fn ping(&self, _: &mut SocketAddr) -> Result<(), Never> {
        Ok(())
    }

    fn handle(s: &SocketAddr) -> SocketAddr {
        *s
    }
}

#[derive(Deserialize)]
struct Network {
    peers: Vec<LinkCandidate>,
}

const CONFIG: &str = r#"
peers = [
    { kind = "ethernet", addr = "10.77.2.2:47770" },
    { kind = "usb4", addr = "10.77.1.2:47770" },
]
"#;

#[tokio::test(start_paused = true)]
async fn the_fixed_and_the_discovered_reach_the_manager() {
    let statics = toml::from_str::<Network>(CONFIG).unwrap().peers;
    validate_static_links(&statics).unwrap();

    // The peer announces over mDNS its LAN IP and also the USB4 one.
    let me = Identity::from_seed([1; 32]).public_key();
    let peer = Identity::from_seed([2; 32]).public_key();
    let mut resolver = PeerResolver::new(Fingerprint::of(&me), [Fingerprint::of(&peer)]);
    resolver.apply(DiscoveryEvent::Resolved(Announcement {
        instance: "tandem-par._tandem._udp.local.".into(),
        fingerprint: Some(Fingerprint::of(&peer).to_hex()),
        addrs: vec![
            "10.77.1.2:47770".parse().unwrap(),
            "192.168.1.20:47770".parse().unwrap(),
        ],
    }));
    // And a neighbor with another Tandem, which does not count.
    resolver.apply(DiscoveryEvent::Resolved(Announcement {
        instance: "tandem-other._tandem._udp.local.".into(),
        fingerprint: Some(Fingerprint::of(&Identity::from_seed([3; 32]).public_key()).to_hex()),
        addrs: vec!["192.168.1.99:47770".parse().unwrap()],
    }));

    let candidates = link_candidates(&statics, &resolver.addrs());
    let rec = Arc::new(Recorder::default());
    let (manager, _events) = LinkManager::spawn(
        &candidates,
        FakeProber(rec.clone()),
        HealthConfig::default(),
    )
    .unwrap();

    let mut watch = manager.watch();
    tokio::time::timeout(
        Duration::from_secs(5),
        watch.wait_for(|s| s.links.iter().all(|l| l.rtt.is_some())),
    )
    .await
    .unwrap()
    .unwrap();

    let links: Vec<(LinkKind, String)> = manager
        .links()
        .iter()
        .map(|l| (l.kind, l.addr.to_string()))
        .collect();
    assert_eq!(
        links,
        [
            (LinkKind::Usb4, "10.77.1.2:47770".into()),
            (LinkKind::Ethernet, "10.77.2.2:47770".into()),
            (LinkKind::Wifi, "192.168.1.20:47770".into()),
        ]
    );
    let connected: Vec<String> = rec
        .0
        .lock()
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        connected,
        ["10.77.1.2:47770", "10.77.2.2:47770", "192.168.1.20:47770"]
    );
    assert_eq!(manager.active().map(|a| a.kind), Some(LinkKind::Usb4));
}
