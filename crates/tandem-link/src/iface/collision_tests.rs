//! T-223: after relabelling by interface, two candidates can land on the same kind. The
//! link manager keeps one path per kind, so the set has to be made unique before it starts,
//! never refused.

use std::net::SocketAddr;
use std::time::Duration;

use proptest::prelude::*;

use super::*;
use crate::manager::{HealthConfig, LinkCandidate, LinkKind, LinkManager, Prober};

fn iface(index: u32, name: &str, addrs: &[&str]) -> NetInterface {
    NetInterface {
        index,
        name: name.into(),
        kind: classify(name),
        addrs: addrs.iter().map(|a| a.parse().unwrap()).collect(),
        mac: None,
        speed_mbps: None,
        up: true,
    }
}

fn cand(kind: LinkKind, addr: &str) -> LinkCandidate {
    LinkCandidate {
        kind,
        addr: addr.parse().unwrap(),
    }
}

/// The acceptance box: `eth1` with an IPv4 link-local and an `fe80::` address, plus Wi-Fi.
fn acceptance_table() -> Vec<NetInterface> {
    vec![
        iface(2, "wlan0", &["192.168.1.20"]),
        iface(3, "eth1", &["169.254.252.1", "fe80::1"]),
    ]
}

/// The Mac of the T-196 run: `en0` (Wi-Fi without saying so) and the Thunderbolt Bridge.
fn mac_table() -> Vec<NetInterface> {
    vec![
        iface(11, "en0", &["192.168.1.41", "fe80::c3"]),
        iface(4, "bridge0", &["169.254.93.252", "fe80::d4"]),
    ]
}

fn kinds(cands: &[LinkCandidate]) -> Vec<LinkKind> {
    cands.iter().map(|c| c.kind).collect()
}

#[derive(Debug, thiserror::Error)]
#[error("no answer")]
struct Silent;

/// Every link answers.
struct AlwaysUp;

impl Prober for AlwaysUp {
    type Session = SocketAddr;
    type Handle = SocketAddr;
    type Error = Silent;

    async fn connect(&self, addr: SocketAddr) -> Result<SocketAddr, Silent> {
        Ok(addr)
    }

    async fn ping(&self, _: &mut SocketAddr) -> Result<(), Silent> {
        Ok(())
    }

    fn handle(s: &SocketAddr) -> SocketAddr {
        *s
    }
}

#[tokio::test(start_paused = true)]
async fn two_configured_peers_on_one_interface_still_start_the_manager() {
    // The reproduction of the report: both are classified Ethernet.
    let cands = [
        cand(LinkKind::Ethernet, "[fe80::2%3]:47770"),
        cand(LinkKind::Wifi, "169.254.252.182:47770"),
    ];
    let (fixed, shadowed) = relabel_with_shadowed(&cands, &acceptance_table());
    assert_eq!(kinds(&fixed), [LinkKind::Ethernet]);
    assert_eq!(fixed[0].addr, cands[0].addr);
    assert_eq!(
        shadowed,
        [Shadowed {
            kind: LinkKind::Ethernet,
            kept: cands[0].addr,
            dropped: cands[1].addr,
        }]
    );
    let (manager, _events) = LinkManager::spawn(&fixed, AlwaysUp, HealthConfig::default())
        .expect("the manager starts with a colliding config");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(manager.active().map(|a| a.kind), Some(LinkKind::Ethernet));
}

#[test]
fn the_plain_relabel_is_unique_too() {
    let cands = [
        cand(LinkKind::Ethernet, "[fe80::2%3]:47770"),
        cand(LinkKind::Wifi, "169.254.252.182:47770"),
    ];
    assert_eq!(
        kinds(&relabel(&cands, &acceptance_table())),
        [LinkKind::Ethernet]
    );
}

#[test]
fn two_peers_in_one_lan_subnet_with_different_declared_kinds_collapse_to_one() {
    let cands = [
        cand(LinkKind::Ethernet, "192.168.1.30:47770"),
        cand(LinkKind::Wifi, "192.168.1.31:47770"),
    ];
    let (fixed, shadowed) = relabel_with_shadowed(&cands, &acceptance_table());
    assert_eq!(kinds(&fixed), [LinkKind::Wifi]);
    // The first in priority order (the declared one is first) wins.
    assert_eq!(fixed[0].addr, cands[0].addr);
    assert_eq!(shadowed.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn one_peer_on_two_interfaces_keeps_both_paths_and_the_cable_is_primary() {
    // The PC seen from the Mac: over the cable and over the LAN.
    let cands = [
        cand(LinkKind::Wifi, "169.254.252.182:47871"),
        cand(LinkKind::Wifi, "192.168.1.40:47871"),
    ];
    let (fixed, shadowed) = relabel_with_shadowed(&cands, &mac_table());
    assert_eq!(kinds(&fixed), [LinkKind::Usb4, LinkKind::Wifi]);
    assert!(shadowed.is_empty());
    let (manager, _events) = LinkManager::spawn(&fixed, AlwaysUp, HealthConfig::default()).unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(manager.active().map(|a| a.kind), Some(LinkKind::Usb4));
    assert_eq!(manager.links().len(), 2);
}

#[test]
fn the_cable_survives_a_peer_that_shares_its_interface() {
    // The T-196 cable declared `wifi`, plus the peer's scoped IPv6 on the same bridge: one
    // USB4 path, and the cable is not lost.
    let cands = [
        cand(LinkKind::Wifi, "169.254.252.182:47871"),
        cand(LinkKind::Wifi, "[fe80::b2%4]:47871"),
    ];
    let (fixed, shadowed) = relabel_with_shadowed(&cands, &mac_table());
    assert_eq!(kinds(&fixed), [LinkKind::Usb4]);
    assert_eq!(fixed[0].addr, cands[0].addr);
    assert_eq!(shadowed.len(), 1);
}

#[test]
fn the_same_address_twice_is_one_candidate() {
    let cands = [
        cand(LinkKind::Ethernet, "10.0.0.2:47770"),
        cand(LinkKind::Usb4, "10.0.0.2:47770"),
    ];
    // No interface knows 10.0.0.0/24, so the labels stay; the address is still one path.
    let (fixed, shadowed) = relabel_with_shadowed(&cands, &[]);
    assert_eq!(fixed, [cands[0]]);
    assert_eq!(shadowed[0].dropped, cands[1].addr);
}

fn any_kind() -> impl Strategy<Value = LinkKind> {
    prop_oneof![
        Just(LinkKind::Usb4),
        Just(LinkKind::Ethernet),
        Just(LinkKind::Wifi)
    ]
}

fn any_addr() -> impl Strategy<Value = SocketAddr> {
    prop_oneof![
        (0u8..4).prop_map(|h| format!("169.254.252.{h}:47770").parse().unwrap()),
        (0u8..4).prop_map(|h| format!("192.168.1.{h}:47770").parse().unwrap()),
        (0u8..4).prop_map(|h| format!("10.0.0.{h}:47770").parse().unwrap()),
        (0u8..4, 2u32..5).prop_map(|(h, s)| format!("[fe80::{h}%{s}]:47770").parse().unwrap()),
    ]
}

proptest! {
    /// Whatever the config: unique kinds and addresses, every kind that relabelling
    /// produced is still there, and every dropped candidate is accounted for.
    #[test]
    fn relabelling_never_leaves_a_set_the_manager_refuses(
        raw in proptest::collection::vec((any_kind(), any_addr()), 1..8),
    ) {
        let cands: Vec<_> = raw.into_iter().map(|(kind, addr)| LinkCandidate { kind, addr }).collect();
        let table = acceptance_table();
        let (fixed, shadowed) = relabel_with_shadowed(&cands, &table);
        let mut seen = kinds(&fixed);
        seen.sort();
        seen.dedup();
        prop_assert_eq!(seen.len(), fixed.len());
        for c in &cands {
            let kind = kind_for_addr(&table, c.addr).unwrap_or(c.kind);
            prop_assert!(fixed.iter().any(|f| f.kind == kind || f.addr == c.addr));
        }
        prop_assert_eq!(fixed.len() + shadowed.len(), cands.len());
        for s in &shadowed {
            prop_assert!(fixed.iter().any(|f| f.addr == s.kept));
        }
    }
}
