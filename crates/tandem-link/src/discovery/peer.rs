use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use super::Fingerprint;

/// TXT key with the fingerprint of the announcing agent.
pub const TXT_FINGERPRINT: &str = "fp";

/// A `_tandem._udp` service resolved on the LAN, already without mdns-sd types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    /// Full name of the instance (identifies the announcement when it is withdrawn).
    pub instance: String,
    /// Raw value of the TXT key `fp`, if it came.
    pub fingerprint: Option<String>,
    /// Addresses with the service's port.
    pub addrs: Vec<SocketAddr>,
}

/// What arrives from the mDNS browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryEvent {
    Resolved(Announcement),
    Removed { instance: String },
}

/// Filters the announcements on the LAN and keeps only those of the paired peer.
///
/// Announcements without a fingerprint or with a malformed fingerprint, those of
/// unpaired agents and its own are ignored (mDNS also returns what one
/// announces itself).
#[derive(Debug, Clone)]
pub struct PeerResolver {
    own: Fingerprint,
    /// `None`: any agent except its own (to pair with a new one).
    wanted: Option<BTreeSet<Fingerprint>>,
    /// Never accepted, even when `wanted` is `None`.
    excluded: BTreeSet<Fingerprint>,
    /// Accepted announcements, per instance.
    found: BTreeMap<String, (Fingerprint, Vec<SocketAddr>)>,
}

impl PeerResolver {
    pub fn new(own: Fingerprint, peers: impl IntoIterator<Item = Fingerprint>) -> Self {
        let mut wanted: BTreeSet<_> = peers.into_iter().collect();
        wanted.remove(&own);
        Self {
            own,
            wanted: Some(wanted),
            excluded: BTreeSet::new(),
            found: BTreeMap::new(),
        }
    }

    /// Accepts any agent except its own: what whoever is
    /// pairing announces, which is not yet trusted. Finding it does not give it trust:
    /// the code comparison does.
    pub fn any(own: Fingerprint) -> Self {
        Self {
            own,
            wanted: None,
            excluded: BTreeSet::new(),
            found: BTreeMap::new(),
        }
    }

    /// Accepts any agent except its own and the ones in `known`: whoever announces
    /// itself for pairing while this machine already trusts `known` (T-345).
    pub fn any_except(own: Fingerprint, known: impl IntoIterator<Item = Fingerprint>) -> Self {
        Self {
            own,
            wanted: None,
            excluded: known.into_iter().collect(),
            found: BTreeMap::new(),
        }
    }

    /// Fingerprint of the announcement if it is from a sought peer; `None` if it must be ignored.
    pub fn accepts(&self, ann: &Announcement) -> Option<Fingerprint> {
        let fp: Fingerprint = ann.fingerprint.as_deref()?.parse().ok()?;
        let wanted = self.wanted.as_ref().is_none_or(|w| w.contains(&fp));
        (fp != self.own && wanted && !self.excluded.contains(&fp)).then_some(fp)
    }

    /// Applies an event; returns whether the peer's addresses changed.
    pub fn apply(&mut self, event: DiscoveryEvent) -> bool {
        let before = self.addrs();
        match event {
            DiscoveryEvent::Resolved(ann) => match self.accepts(&ann) {
                Some(fp) => {
                    let addrs = usable(&ann.addrs);
                    if addrs.is_empty() {
                        self.found.remove(&ann.instance);
                    } else {
                        self.found.insert(ann.instance, (fp, addrs));
                    }
                }
                // If an instance that used to be the peer's now announces another
                // fingerprint, it stops counting.
                None => {
                    self.found.remove(&ann.instance);
                }
            },
            DiscoveryEvent::Removed { instance } => {
                self.found.remove(&instance);
            }
        }
        self.addrs() != before
    }

    /// The peer's addresses on the LAN, from most to least preferred.
    pub fn addrs(&self) -> Vec<SocketAddr> {
        self.collect(|_| true)
    }

    /// Addresses of a specific peer.
    pub fn addrs_of(&self, peer: &Fingerprint) -> Vec<SocketAddr> {
        self.collect(|fp| fp == peer)
    }

    fn collect(&self, keep: impl Fn(&Fingerprint) -> bool) -> Vec<SocketAddr> {
        let unique: BTreeSet<SocketAddr> = self
            .found
            .values()
            .filter(|(fp, _)| keep(fp))
            .flat_map(|(_, a)| a.iter().copied())
            .collect();
        let mut all: Vec<_> = unique.into_iter().collect();
        all.sort_by_key(|a| rank(a.ip()));
        all
    }
}

/// `fe80::1`: macOS gives it to its loopback (`lo0`) and mDNS announces it on every
/// interface, where it is never the other machine.
const MACOS_LOOPBACK_LINK_LOCAL: Ipv6Addr = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);

/// Discards what is no use for connecting to another machine.
fn usable(addrs: &[SocketAddr]) -> Vec<SocketAddr> {
    addrs
        .iter()
        .copied()
        .filter(|a| a.port() != 0)
        .filter(|a| {
            let ip = a.ip();
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip == IpAddr::V6(MACOS_LOOPBACK_LINK_LOCAL))
        })
        .collect()
}

/// IPv4 first (the home LAN), then global IPv6 and finally link-local
/// IPv6, which depends on the interface index.
fn rank(ip: IpAddr) -> u8 {
    match ip {
        IpAddr::V4(v4) if v4.is_link_local() => 1,
        IpAddr::V4(_) => 0,
        IpAddr::V6(v6) if v6.is_unicast_link_local() => 3,
        IpAddr::V6(_) => 2,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::identity::Identity;

    fn fp(seed: u8) -> Fingerprint {
        Fingerprint::of(&Identity::from_seed([seed; 32]).public_key())
    }

    fn ann(instance: &str, fp: Option<String>, addrs: &[&str]) -> Announcement {
        Announcement {
            instance: instance.into(),
            fingerprint: fp,
            addrs: addrs.iter().map(|a| a.parse().unwrap()).collect(),
        }
    }

    const ME: u8 = 1;
    const PEER: u8 = 2;
    const STRANGER: u8 = 3;

    fn resolver() -> PeerResolver {
        PeerResolver::new(fp(ME), [fp(PEER)])
    }

    #[test]
    fn accepts_only_the_peers_fingerprint() {
        let r = resolver();
        let peer = ann("pc", Some(fp(PEER).to_hex()), &["192.168.1.20:47770"]);
        assert_eq!(r.accepts(&peer), Some(fp(PEER)));
        // In uppercase it is also the same fingerprint.
        let upper = ann("pc", Some(fp(PEER).to_hex().to_uppercase()), &[]);
        assert_eq!(r.accepts(&upper), Some(fp(PEER)));
    }

    #[test]
    fn ignores_other_announcements() {
        let mut r = resolver();
        let otros = [
            // Another Tandem agent, unpaired.
            ann("x", Some(fp(STRANGER).to_hex()), &["192.168.1.30:47770"]),
            // Its own announcement.
            ann("me", Some(fp(ME).to_hex()), &["192.168.1.10:47770"]),
            // Without a fingerprint.
            ann("without", None, &["192.168.1.40:47770"]),
            // Malformed or truncated fingerprint.
            ann("bad", Some("hello".into()), &["192.168.1.50:47770"]),
            ann("short", Some(fp(PEER).short()), &["192.168.1.60:47770"]),
        ];
        for a in otros {
            assert_eq!(r.accepts(&a), None, "{a:?}");
            assert!(!r.apply(DiscoveryEvent::Resolved(a)));
        }
        assert!(r.addrs().is_empty());

        assert!(r.apply(DiscoveryEvent::Resolved(ann(
            "pc",
            Some(fp(PEER).to_hex()),
            &["192.168.1.20:47770"],
        ))));
        assert_eq!(r.addrs(), vec!["192.168.1.20:47770".parse().unwrap()]);
    }

    #[test]
    fn the_own_fingerprint_is_never_the_peer() {
        // Even if its own key is mistakenly passed as the peer.
        let r = PeerResolver::new(fp(ME), [fp(ME), fp(PEER)]);
        assert_eq!(r.accepts(&ann("me", Some(fp(ME).to_hex()), &[])), None);
    }

    #[test]
    fn without_a_list_it_accepts_anyone_but_itself() {
        let mut r = PeerResolver::any(fp(ME));
        let stranger = ann("x", Some(fp(STRANGER).to_hex()), &["192.168.1.30:47771"]);
        assert_eq!(r.accepts(&stranger), Some(fp(STRANGER)));
        assert_eq!(r.accepts(&ann("me", Some(fp(ME).to_hex()), &[])), None);
        assert_eq!(r.accepts(&ann("without", None, &[])), None);
        assert!(r.apply(DiscoveryEvent::Resolved(stranger)));
        assert_eq!(r.addrs(), vec!["192.168.1.30:47771".parse().unwrap()]);
    }

    /// T-345: whoever is pairing and is not the old peer is an offer; the old peer itself
    /// (or this machine) is not.
    #[test]
    fn any_except_skips_the_known_peer_and_itself() {
        let mut r = PeerResolver::any_except(fp(ME), [fp(PEER)]);
        let old = ann("old", Some(fp(PEER).to_hex()), &["192.168.1.20:47771"]);
        let me = ann("me", Some(fp(ME).to_hex()), &["192.168.1.10:47771"]);
        let new = ann("new", Some(fp(STRANGER).to_hex()), &["192.168.1.30:47771"]);
        assert_eq!(r.accepts(&old), None);
        assert_eq!(r.accepts(&me), None);
        assert_eq!(r.accepts(&new), Some(fp(STRANGER)));
        assert!(!r.apply(DiscoveryEvent::Resolved(old)));
        assert!(r.apply(DiscoveryEvent::Resolved(new)));
        assert_eq!(r.addrs(), vec!["192.168.1.30:47771".parse().unwrap()]);
    }

    #[test]
    fn forgotten_on_withdrawal_or_fingerprint_change() {
        let mut r = resolver();
        let peer = ann("pc", Some(fp(PEER).to_hex()), &["192.168.1.20:47770"]);
        assert!(r.apply(DiscoveryEvent::Resolved(peer.clone())));
        // Repeating the same announcement changes nothing.
        assert!(!r.apply(DiscoveryEvent::Resolved(peer)));
        assert!(r.apply(DiscoveryEvent::Removed {
            instance: "pc".into()
        }));
        assert!(r.addrs().is_empty());

        r.apply(DiscoveryEvent::Resolved(ann(
            "pc",
            Some(fp(PEER).to_hex()),
            &["192.168.1.20:47770"],
        )));
        assert!(r.apply(DiscoveryEvent::Resolved(ann(
            "pc",
            Some(fp(STRANGER).to_hex()),
            &["192.168.1.20:47770"],
        ))));
        assert!(r.addrs().is_empty());
    }

    #[test]
    fn sorts_and_cleans_addresses() {
        let mut r = resolver();
        r.apply(DiscoveryEvent::Resolved(ann(
            "pc",
            Some(fp(PEER).to_hex()),
            &[
                "[fe80::5%2]:47770",
                "[2001:db8::5]:47770",
                "127.0.0.1:47770",
                "0.0.0.0:47770",
                "192.168.1.20:0",
                "169.254.3.3:47770",
                "192.168.1.20:47770",
            ],
        )));
        let got: Vec<String> = r.addrs().iter().map(ToString::to_string).collect();
        assert_eq!(
            got,
            [
                "192.168.1.20:47770",
                "169.254.3.3:47770",
                "[2001:db8::5]:47770",
                "[fe80::5%2]:47770",
            ]
        );
        assert_eq!(r.addrs_of(&fp(PEER)), r.addrs());
        assert!(r.addrs_of(&fp(STRANGER)).is_empty());
    }

    /// The Mac announces `fe80::1`, the link-local of its loopback (`lo0`), on every interface.
    /// Taken as the peer on the cable it sorts first, so the PC dialled it over USB4 and never
    /// got an answer (2026-11-24): the cable only came up when the Mac dialled the PC.
    #[test]
    fn the_macos_loopback_link_local_is_not_the_peer() {
        let mut r = resolver();
        r.apply(DiscoveryEvent::Resolved(ann(
            "mac",
            Some(fp(PEER).to_hex()),
            &[
                "192.0.2.10:52000",
                "169.254.57.63:52000",
                "[fe80::1%21]:52000",
                "[fe80::c0c:1e4f:2370:bfc5%21]:52000",
                "[fe80::1%7]:52000",
            ],
        )));
        let got: Vec<String> = r.addrs().iter().map(ToString::to_string).collect();
        assert_eq!(
            got,
            [
                "192.0.2.10:52000",
                "169.254.57.63:52000",
                "[fe80::c0c:1e4f:2370:bfc5%21]:52000",
            ]
        );
    }

    proptest! {
        /// No announcement with a foreign fingerprint contributes addresses, in
        /// any order and mixed with the peer's.
        #[test]
        fn only_the_peer_contributes_addresses(
            events in prop::collection::vec((0u8..4, 0u8..5, any::<bool>(), 1u8..255), 0..40)
        ) {
            let mut r = resolver();
            let mut peer_ips = BTreeSet::new();
            for (who, inst, remove, last) in events {
                let instance = format!("i{inst}");
                let addr: SocketAddr = format!("10.0.{who}.{last}:47770").parse().unwrap();
                if remove {
                    r.apply(DiscoveryEvent::Removed { instance });
                } else {
                    let seed = [ME, PEER, STRANGER, 9][usize::from(who)];
                    r.apply(DiscoveryEvent::Resolved(ann(&instance, Some(fp(seed).to_hex()), &[])
                        .with(addr)));
                    if seed == PEER {
                        peer_ips.insert(addr);
                    }
                }
                for a in r.addrs() {
                    prop_assert!(peer_ips.contains(&a), "{a} does not come from the peer");
                }
            }
        }
    }

    impl Announcement {
        fn with(mut self, addr: SocketAddr) -> Self {
            self.addrs.push(addr);
            self
        }
    }
}
