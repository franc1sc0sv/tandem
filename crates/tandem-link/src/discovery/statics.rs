use std::collections::BTreeSet;
use std::net::SocketAddr;

use super::DiscoveryError;
use crate::manager::{LinkCandidate, LinkKind};

/// Checks the other agent's fixed addresses that come from the config
/// (the direct links, for example `10.77.1.2` over USB4 and `10.77.2.2` over
/// Ethernet; DESIGN §6).
pub fn validate_static_links(links: &[LinkCandidate]) -> Result<(), DiscoveryError> {
    let mut kinds = BTreeSet::new();
    for l in links {
        if l.addr.port() == 0 {
            return Err(DiscoveryError::StaticWithoutPort(*l));
        }
        if l.addr.ip().is_unspecified() || l.addr.ip().is_multicast() {
            return Err(DiscoveryError::StaticBadAddress(*l));
        }
        if !kinds.insert(l.kind) {
            return Err(DiscoveryError::DuplicateStatic(l.kind));
        }
    }
    Ok(())
}

/// Candidates for the [`LinkManager`](crate::manager::LinkManager): the
/// config's fixed addresses as they are and, if the config does not set Wi-Fi,
/// the best address the peer announced on the LAN as a Wi-Fi link.
///
/// A discovered address that is already that of a fixed link is discarded: the
/// peer also announces over mDNS on the cable interfaces and that address is not
/// the LAN's.
pub fn link_candidates(statics: &[LinkCandidate], discovered: &[SocketAddr]) -> Vec<LinkCandidate> {
    let mut out: Vec<LinkCandidate> = statics.to_vec();
    if !statics.iter().any(|l| l.kind == LinkKind::Wifi) {
        let lan = discovered
            .iter()
            .find(|d| !statics.iter().any(|s| s.addr.ip() == d.ip()));
        if let Some(&addr) = lan {
            out.push(LinkCandidate {
                kind: LinkKind::Wifi,
                addr,
            });
        }
    }
    out.sort_by_key(|l| l.kind);
    out
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn link(kind: LinkKind, addr: &str) -> LinkCandidate {
        LinkCandidate {
            kind,
            addr: addr.parse().unwrap(),
        }
    }

    fn statics() -> Vec<LinkCandidate> {
        vec![
            link(LinkKind::Ethernet, "10.77.2.2:47770"),
            link(LinkKind::Usb4, "10.77.1.2:47770"),
        ]
    }

    #[test]
    fn validates_the_fixed_ones() {
        assert!(validate_static_links(&statics()).is_ok());
        assert!(validate_static_links(&[]).is_ok());
        assert!(matches!(
            validate_static_links(&[link(LinkKind::Usb4, "10.77.1.2:0")]),
            Err(DiscoveryError::StaticWithoutPort(_))
        ));
        assert!(matches!(
            validate_static_links(&[link(LinkKind::Usb4, "0.0.0.0:47770")]),
            Err(DiscoveryError::StaticBadAddress(_))
        ));
        assert!(matches!(
            validate_static_links(&[
                link(LinkKind::Usb4, "10.77.1.2:47770"),
                link(LinkKind::Usb4, "10.77.1.6:47770"),
            ]),
            Err(DiscoveryError::DuplicateStatic(LinkKind::Usb4))
        ));
    }

    #[test]
    fn the_fixed_ones_pass_as_they_are_and_in_priority_order() {
        assert_eq!(
            link_candidates(&statics(), &[]),
            vec![
                link(LinkKind::Usb4, "10.77.1.2:47770"),
                link(LinkKind::Ethernet, "10.77.2.2:47770"),
            ]
        );
    }

    #[test]
    fn the_discovered_is_added_as_wifi() {
        let lan: SocketAddr = "192.168.1.20:47770".parse().unwrap();
        let got = link_candidates(&statics(), &["10.77.1.2:47770".parse().unwrap(), lan]);
        assert_eq!(
            got.last(),
            Some(&link(LinkKind::Wifi, "192.168.1.20:47770"))
        );
        assert_eq!(got.len(), 3);
    }

    #[test]
    fn a_fixed_wifi_overrides_the_discovered() {
        let mut s = statics();
        s.push(link(LinkKind::Wifi, "192.168.1.99:47770"));
        let got = link_candidates(&s, &["192.168.1.20:47770".parse().unwrap()]);
        assert_eq!(
            got.last(),
            Some(&link(LinkKind::Wifi, "192.168.1.99:47770"))
        );
        assert_eq!(got.len(), 3);
    }

    #[test]
    fn without_fixed_ones_only_the_discovered_remains() {
        let got = link_candidates(&[], &["192.168.1.20:47770".parse().unwrap()]);
        assert_eq!(got, vec![link(LinkKind::Wifi, "192.168.1.20:47770")]);
        assert!(link_candidates(&[], &[]).is_empty());
    }

    proptest! {
        /// The result is always acceptable to the manager: no repeated
        /// types, with all the fixed ones and in priority order.
        #[test]
        fn valid_candidates(
            kinds in prop::sample::subsequence(LinkKind::ALL.to_vec(), 0..=3),
            found in prop::collection::vec((0u8..3, 1u8..4), 0..6),
        ) {
            let statics: Vec<_> = kinds
                .iter()
                .enumerate()
                .map(|(i, &kind)| LinkCandidate {
                    kind,
                    addr: SocketAddr::from(([10, 77, i as u8 + 1, 2], 47770)),
                })
                .collect();
            let found: Vec<SocketAddr> = found
                .into_iter()
                .map(|(net, host)| SocketAddr::from(([10, 77, net + 1, host], 47770)))
                .collect();
            let got = link_candidates(&statics, &found);
            let mut seen = BTreeSet::new();
            prop_assert!(got.iter().all(|l| seen.insert(l.kind)));
            prop_assert!(got.windows(2).all(|w| w[0].kind < w[1].kind));
            prop_assert!(statics.iter().all(|s| got.contains(s)));
            for l in got.iter().filter(|l| !statics.contains(l)) {
                prop_assert_eq!(l.kind, LinkKind::Wifi);
                prop_assert!(!statics.iter().any(|s| s.addr.ip() == l.addr.ip()));
            }
        }
    }
}
