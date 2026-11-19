//! T-196: the Thunderbolt / USB4 cable as the hardware run saw it, with simulated OS
//! interface tables for Windows and macOS.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use proptest::prelude::*;
use tandem_core::{DataPrefs, Machine, Os};

use super::*;
use crate::manager::{LinkCandidate, LinkKind};
use crate::qos::{LinkQos, Qos};

const PORT: u16 = 47871;

fn row(name: &str, index: u32, description: Option<&str>, ip: &str) -> AddrRow {
    AddrRow {
        name: name.into(),
        index,
        description: description.map(Into::into),
        ip: ip.parse().unwrap(),
        up: true,
    }
}

/// The PC of the run: Wi-Fi to the LAN and the USB4 P2P adapter, whose friendly name is
/// the default "Ethernet 3".
fn windows_table() -> Vec<NetInterface> {
    let wifi = Some("Intel(R) Wi-Fi 6E AX211 160MHz");
    let usb4 = Some("USB4(TM) P2P Network Adapter");
    interfaces_from_rows([
        row("Wi-Fi", 5, wifi, "192.168.1.40"),
        row("Wi-Fi", 5, wifi, "fe80::a1"),
        row("Ethernet 3", 13, usb4, "169.254.252.182"),
        row("Ethernet 3", 13, usb4, "fe80::b2"),
    ])
}

/// The Mac of the run: `en0` (Wi-Fi, but the name does not say so) and the Thunderbolt
/// Bridge `bridge0`.
fn macos_table() -> Vec<NetInterface> {
    interfaces_from_rows([
        row("en0", 11, None, "192.168.1.41"),
        row("en0", 11, None, "fe80::c3"),
        row("bridge0", 4, None, "169.254.93.252"),
        row("bridge0", 4, None, "fe80::d4"),
    ])
}

fn wifi_candidate(addr: &str) -> LinkCandidate {
    LinkCandidate {
        kind: LinkKind::Wifi,
        addr: addr.parse().unwrap(),
    }
}

fn kind_of(table: &[NetInterface], name: &str) -> InterfaceKind {
    table.iter().find(|i| i.name == name).unwrap().kind
}

#[test]
fn the_windows_usb4_adapter_is_usb4_by_its_description_not_its_name() {
    let t = windows_table();
    assert_eq!(kind_of(&t, "Ethernet 3"), InterfaceKind::Usb4);
    assert_eq!(kind_of(&t, "Wi-Fi"), InterfaceKind::Wifi);
    // One entry per adapter, with all its addresses.
    assert_eq!(t.len(), 2);
    assert_eq!(t.iter().map(|i| i.addrs.len()).sum::<usize>(), 4);
    // Without a description (macOS, Linux) the name decides, as before.
    assert_eq!(classify_adapter("bridge0", None), InterfaceKind::Usb4);
    assert_eq!(
        classify_adapter("Ethernet 3", None),
        InterfaceKind::Ethernet
    );
    // A description that says nothing useful leaves the name in charge.
    assert_eq!(
        classify_adapter("Ethernet 2", Some("Generic adapter")),
        InterfaceKind::Ethernet
    );
    assert_eq!(
        classify_adapter(
            "Ethernet 2",
            Some("Realtek PCIe GbE Ethernet Family Controller")
        ),
        InterfaceKind::Ethernet
    );
}

#[test]
fn a_configured_link_local_peer_on_the_cable_is_usb4_on_windows() {
    // The PC's peer is the Mac's cable address: a different /24 than its own.
    let fixed = relabel(&[wifi_candidate("169.254.93.252:47871")], &windows_table());
    assert_eq!(fixed[0].kind, LinkKind::Usb4);
}

#[test]
fn a_configured_link_local_peer_on_the_cable_is_usb4_on_macos() {
    let fixed = relabel(&[wifi_candidate("169.254.252.182:47871")], &macos_table());
    assert_eq!(fixed[0].kind, LinkKind::Usb4);
}

#[test]
fn the_lan_peer_stays_wifi_and_an_unknown_network_keeps_its_label() {
    let cands = [
        wifi_candidate("192.168.1.41:47871"),
        LinkCandidate {
            kind: LinkKind::Ethernet,
            addr: "172.16.0.2:47871".parse().unwrap(),
        },
    ];
    let kinds: Vec<_> = relabel(&cands, &windows_table())
        .iter()
        .map(|c| c.kind)
        .collect();
    assert_eq!(kinds, [LinkKind::Wifi, LinkKind::Ethernet]);
}

#[test]
fn an_incoming_ipv4_peer_on_a_dual_stack_socket_is_classified_too() {
    // A dual-stack socket reports the Mac as `::ffff:169.254.93.252`.
    let remote: SocketAddr = "[::ffff:169.254.93.252]:52000".parse().unwrap();
    assert_eq!(
        kind_for_addr(&windows_table(), remote),
        Some(LinkKind::Usb4)
    );
}

#[test]
fn a_cable_that_is_down_does_not_classify_anything() {
    let mut t = macos_table();
    for i in t.iter_mut().filter(|i| i.name == "bridge0") {
        i.up = false;
    }
    assert_eq!(
        kind_for_addr(&t, "169.254.252.182:47871".parse().unwrap()),
        None
    );
}

#[test]
fn with_several_link_local_interfaces_the_cable_wins() {
    // A Wi-Fi without DHCP also gets 169.254/16; the cable network is link-local by design.
    let mut t = windows_table();
    t.iter_mut()
        .find(|i| i.name == "Wi-Fi")
        .unwrap()
        .addrs
        .push("169.254.1.9".parse().unwrap());
    t.reverse();
    assert_eq!(
        kind_for_addr(&t, "169.254.93.252:47871".parse().unwrap()),
        Some(LinkKind::Usb4)
    );
}

#[test]
fn an_ipv6_link_local_is_dialled_with_the_local_cable_scope_id() {
    let t = windows_table();
    let cable = t.iter().find(|i| i.name == "Ethernet 3").unwrap();
    // mDNS tags each answer with the interface it arrived on; the Mac also answers over
    // Wi-Fi (index 5), where its cable address is not reachable.
    let announced: Vec<SocketAddr> = vec![
        "[fe80::d4%5]:47871".parse().unwrap(),
        "[fe80::d4%13]:47871".parse().unwrap(),
    ];
    let (_tx, rx) = tokio::sync::watch::channel(announced);
    let discovery = ScopedDiscovery::new(rx);
    let p = plan(
        cable,
        Machine::B,
        Os::Windows,
        discovery.peer_addr(cable),
        PORT,
    );
    let Some(SocketAddr::V6(dial)) = p.peer else {
        panic!("expected an IPv6 peer, got {:?}", p.peer);
    };
    assert_eq!(dial.scope_id(), cable.index);
    assert_eq!(dial.ip().to_string(), "fe80::d4");
    // And that address is classified as the cable.
    assert_eq!(
        kind_for_addr(&t, SocketAddr::V6(dial)),
        Some(LinkKind::Usb4)
    );
}

#[test]
fn an_ipv6_link_local_learned_on_another_interface_is_not_used_for_the_cable() {
    let t = macos_table();
    let cable = t.iter().find(|i| i.name == "bridge0").unwrap();
    let only_wifi: Vec<SocketAddr> = vec!["[fe80::b2%11]:47871".parse().unwrap()];
    let (_tx, rx) = tokio::sync::watch::channel(only_wifi);
    let discovery = ScopedDiscovery::new(rx);
    assert_eq!(discovery.peer_addr(cable), None);
}

#[test]
fn the_usb4_qos_profile_applies_to_the_cable_link() {
    let wifi_profile = Qos::default();
    wifi_profile.set_link(Some(LinkKind::Wifi));
    assert!(wifi_profile.cap().is_some(), "Wi-Fi is capped");

    // Each side labels the link with its own table; the link takes the worst of both.
    let pc = relabel(&[wifi_candidate("169.254.93.252:47871")], &windows_table())[0].kind;
    let mac = relabel(&[wifi_candidate("169.254.252.182:47871")], &macos_table())[0].kind;
    let qos = LinkQos::default();
    qos.set_prefs(DataPrefs::default());
    let link = qos.get(pc);
    link.set_peer_link(Some(mac));
    assert_eq!(link.link(), Some(LinkKind::Usb4));
    assert_eq!(link.cap(), None, "no data cap on the cable");
    assert_eq!(
        link.input_target(),
        crate::qos::input_p99_target(Some(LinkKind::Usb4))
    );
    assert!(link.input_target() < wifi_profile.input_target());
}

#[test]
fn the_system_backend_is_still_an_interfaces_impl() {
    fn assert_impl<T: Interfaces>() {}
    assert_impl::<SystemInterfaces>();
    let _: Arc<dyn Interfaces> = Arc::new(FakeInterfaces::new(macos_table()));
}

proptest! {
    #[test]
    fn a_v4_mapped_address_is_classified_like_its_ipv4(a in any::<u8>(), b in any::<u8>(), port in 1u16..) {
        let t = windows_table();
        let v4 = Ipv4Addr::new(169, 254, a, b);
        let plain = SocketAddr::new(IpAddr::V4(v4), port);
        let mapped = SocketAddr::new(IpAddr::V6(v4.to_ipv6_mapped()), port);
        prop_assert_eq!(kind_for_addr(&t, plain), kind_for_addr(&t, mapped));
        prop_assert_eq!(kind_for_addr(&t, plain), Some(LinkKind::Usb4));
    }
}
