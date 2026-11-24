use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use proptest::prelude::*;
use tandem_core::{Machine, Os};

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

const PORT: u16 = 47770;

#[test]
fn the_type_comes_from_the_name_and_the_doubtful_is_not_a_cable() {
    for (name, kind) in [
        ("USB4 P2P Network Adapter", InterfaceKind::Usb4),
        ("Thunderbolt Networking", InterfaceKind::Usb4),
        ("bridge0", InterfaceKind::Usb4),
        ("Wi-Fi", InterfaceKind::Wifi),
        ("wlp3s0", InterfaceKind::Wifi),
        ("Ethernet 2", InterfaceKind::Ethernet),
        ("enp5s0", InterfaceKind::Ethernet),
        // macOS: it can be Wi-Fi or Ethernet, it is not guessed.
        ("en0", InterfaceKind::Other),
        ("utun3", InterfaceKind::Other),
    ] {
        assert_eq!(classify(name), kind, "{name}");
    }
    // What is unknown counts as Wi-Fi for the link.
    assert_eq!(InterfaceKind::Other.link_kind(), LinkKind::Wifi);
}

#[test]
fn the_label_comes_from_the_interface_even_if_the_config_says_otherwise() {
    let ifs = [
        iface(5, "bridge0", &["10.77.1.1"]),
        iface(6, "en0", &["192.168.1.20"]),
    ];
    let cands = [
        // The config says Ethernet, but that network is the USB4 cable's.
        LinkCandidate {
            kind: LinkKind::Ethernet,
            addr: "10.77.1.2:47770".parse().unwrap(),
        },
        LinkCandidate {
            kind: LinkKind::Usb4,
            addr: "192.168.1.30:47770".parse().unwrap(),
        },
        // Without a known interface the config is respected.
        LinkCandidate {
            kind: LinkKind::Ethernet,
            addr: "172.16.0.2:47770".parse().unwrap(),
        },
    ];
    let kinds: Vec<_> = relabel(&cands, &ifs).iter().map(|c| c.kind).collect();
    assert_eq!(kinds, [LinkKind::Usb4, LinkKind::Wifi, LinkKind::Ethernet]);
}

#[test]
fn a_link_local_address_is_labeled_by_its_scope_id() {
    let ifs = [iface(7, "USB4 P2P Network Adapter", &["fe80::1"])];
    let on = |scope| -> SocketAddr { format!("[fe80::2%{scope}]:47770").parse().unwrap() };
    assert_eq!(kind_for_addr(&ifs, on(7)), Some(LinkKind::Usb4));
    assert_eq!(kind_for_addr(&ifs, on(8)), None);
}

#[test]
fn the_worst_label_wins() {
    use LinkKind::*;
    // PC Ethernet, Mac Wi-Fi: Wi-Fi, no cable.
    assert_eq!(worst(Ethernet, Wifi), Wifi);
    assert_eq!(worst(Usb4, Ethernet), Ethernet);
    assert_eq!(worst(Usb4, Usb4), Usb4);
    assert!(!worst(Ethernet, Wifi).is_wired());
}

proptest! {
    #[test]
    fn worst_is_commutative_idempotent_and_never_improves(a in 0usize..3, b in 0usize..3) {
        let (a, b) = (LinkKind::ALL[a], LinkKind::ALL[b]);
        prop_assert_eq!(worst(a, b), worst(b, a));
        prop_assert_eq!(worst(a, a), a);
        prop_assert!(worst(a, b) >= a && worst(a, b) >= b);
    }
}

#[test]
fn the_scope_id_goes_in_the_link_local_address_and_only_in_it() {
    let ll: IpAddr = "fe80::1c2:3ff:fe04:506".parse().unwrap();
    let SocketAddr::V6(v6) = scoped_addr(ll, 12, PORT) else {
        panic!("IPv6 was expected");
    };
    assert_eq!((v6.scope_id(), v6.port()), (12, PORT));
    assert_eq!(
        *v6.ip(),
        "fe80::1c2:3ff:fe04:506".parse::<Ipv6Addr>().unwrap()
    );
    // The limits of the fe80::/10 range.
    for (ip, scoped) in [
        ("febf::1", true),
        ("fec0::1", false),
        ("2001:db8::1", false),
    ] {
        let SocketAddr::V6(v6) = scoped_addr(ip.parse().unwrap(), 3, PORT) else {
            panic!()
        };
        assert_eq!(v6.scope_id() == 3, scoped, "{ip}");
    }
    // IPv4 carries no scope.
    assert_eq!(
        scoped_addr("10.77.1.2".parse().unwrap(), 3, PORT).to_string(),
        "10.77.1.2:47770"
    );
}

#[test]
fn the_plan_uses_the_link_local_with_its_scope() {
    let i = iface(9, "bridge0", &["fe80::aa", "169.254.7.7"]);
    let peer = Some("[fe80::bb]:47999".parse().unwrap());
    let p = plan(&i, Machine::A, Os::MacOs, peer, PORT);
    assert_eq!(p.local, "fe80::aa".parse::<IpAddr>().unwrap());
    // The port is the one mDNS announces, not the configured one.
    assert_eq!(p.peer, Some("[fe80::bb%9]:47999".parse().unwrap()));
    assert_eq!(p.assign, None);
    // Without knowing where the other is yet (its link-local comes from mDNS), there is no address.
    assert_eq!(plan(&i, Machine::A, Os::MacOs, None, PORT).peer, None);
}

#[test]
fn without_link_local_each_side_assigns_its_fallback_ip() {
    let i = iface(4, "USB4 P2P Network Adapter", &[]);
    let a = plan(&i, Machine::A, Os::MacOs, None, PORT);
    assert_eq!(a.local, IpAddr::V4(Ipv4Addr::new(10, 77, 1, 1)));
    assert_eq!(a.peer, Some("10.77.1.2:47770".parse().unwrap()));
    assert_eq!(
        a.assign.unwrap().command,
        "sudo ifconfig USB4 P2P Network Adapter inet 10.77.1.1 netmask 255.255.255.252"
    );

    let b = plan(&i, Machine::B, Os::Windows, None, PORT);
    assert_eq!(b.local, IpAddr::V4(Ipv4Addr::new(10, 77, 1, 2)));
    assert_eq!(b.peer, Some("10.77.1.1:47770".parse().unwrap()));
    assert_eq!(
        b.assign.unwrap().command,
        "netsh interface ip set address name=\"USB4 P2P Network Adapter\" static 10.77.1.2 255.255.255.252"
    );

    // With the IP already assigned there is nothing to ask for.
    let done = iface(4, "bridge0", &["10.77.1.2"]);
    assert_eq!(plan(&done, Machine::B, Os::MacOs, None, PORT).assign, None);
}

#[test]
fn diff_gives_additions_and_removals_and_a_down_interface_counts_as_a_removal() {
    let wifi = iface(1, "wlan0", &["192.168.1.5"]);
    let usb = iface(2, "bridge0", &["10.77.1.1"]);
    let all = [wifi.clone(), usb.clone()];
    assert_eq!(
        diff(std::slice::from_ref(&wifi), &all),
        [InterfaceEvent::Added(usb.clone())]
    );
    assert_eq!(
        diff(&all, std::slice::from_ref(&wifi)),
        [InterfaceEvent::Removed(usb.clone())]
    );
    let mut down = usb.clone();
    down.up = false;
    assert_eq!(
        diff(&all, &[wifi.clone(), down.clone()]),
        [InterfaceEvent::Removed(usb)]
    );
    assert_eq!(diff(&[wifi.clone(), down], &[wifi]), []);
}

fn resolve(i: &NetInterface) -> Option<SocketAddr> {
    let port = PORT + u16::from(i.kind.link_kind().priority());
    Some(SocketAddr::from(([10, 77, 1, 2], port)))
}

#[test]
fn the_cable_changes_are_the_difference_from_what_was_applied() {
    let mut cur = Default::default();
    let wifi = iface(1, "wlan0", &["192.168.1.5"]);
    let usb = iface(2, "bridge0", &["10.77.1.1"]);
    let eth = iface(3, "Ethernet", &["10.77.2.1"]);

    // Wi-Fi is not a cable.
    assert_eq!(
        wired_changes(&mut cur, std::slice::from_ref(&wifi), &resolve),
        []
    );
    let all = [wifi.clone(), usb, eth.clone()];
    let kinds: Vec<_> = wired_changes(&mut cur, &all, &resolve)
        .iter()
        .map(|c| match c {
            WiredChange::Add(c) => (true, c.kind),
            WiredChange::Remove(k) => (false, *k),
        })
        .collect();
    assert_eq!(kinds, [(true, LinkKind::Usb4), (true, LinkKind::Ethernet)]);
    // No changes, nothing.
    assert_eq!(wired_changes(&mut cur, &all, &resolve), []);
    // USB4 goes down.
    assert_eq!(
        wired_changes(&mut cur, &[wifi, eth], &resolve),
        [WiredChange::Remove(LinkKind::Usb4)]
    );
}

#[test]
fn without_the_others_address_there_is_no_link() {
    let mut cur = Default::default();
    let usb = iface(2, "bridge0", &["fe80::1"]);
    assert_eq!(wired_changes(&mut cur, &[usb], &|_| None), []);
}

/// The PC's log of 2026-11-24: mDNS on the PC alternates between the Mac's full answer (with
/// its link-locals on the cable's interface) and one without them, and the cable's address
/// flipped with it. While the cable's interface stays the same, that is never a removal: the
/// link keeps its connection, and a new address only replaces the one to dial next.
#[test]
fn an_address_that_flickers_on_the_same_interface_never_removes_the_cable() {
    let mut cur = Default::default();
    let usb = iface(
        21,
        "USB4 P2P Network Adapter",
        &["fe80::677:f28f:7726:2c3b"],
    );
    let a: SocketAddr = "[fe80::c0c:1e4f:2370:bfc5%21]:52000".parse().unwrap();
    let b: SocketAddr = "[fe80::1c46:56e:2a49:fb65%21]:52000".parse().unwrap();
    let add = |addr| {
        vec![WiredChange::Add(LinkCandidate {
            kind: LinkKind::Usb4,
            addr,
        })]
    };
    let list = std::slice::from_ref(&usb);
    assert_eq!(wired_changes(&mut cur, list, &|_| Some(a)), add(a));
    for _ in 0..3 {
        assert_eq!(wired_changes(&mut cur, list, &|_| None), []);
        assert_eq!(wired_changes(&mut cur, list, &|_| Some(a)), []);
    }
    assert_eq!(wired_changes(&mut cur, list, &|_| Some(b)), add(b));
    assert_eq!(wired_changes(&mut cur, list, &|_| None), []);
}

/// The adapter came back with another index (20, then 38 on the PC): the old scope is gone, so
/// that is a real removal, then the new link.
#[test]
fn a_cable_that_comes_back_on_another_interface_is_replaced() {
    let mut cur = Default::default();
    let old = iface(
        20,
        "USB4 P2P Network Adapter",
        &["fe80::677:f28f:7726:2c3b"],
    );
    let new = iface(
        38,
        "USB4 P2P Network Adapter #2",
        &["fe80::d1eb:d9f3:caf4:3ff1"],
    );
    let peer = |i: &NetInterface| {
        Some(SocketAddr::V6(std::net::SocketAddrV6::new(
            "fe80::c0c:1e4f:2370:bfc5".parse().unwrap(),
            52000,
            0,
            i.index,
        )))
    };
    wired_changes(&mut cur, std::slice::from_ref(&old), &peer);
    let changes = wired_changes(&mut cur, std::slice::from_ref(&new), &peer);
    assert_eq!(
        changes,
        [
            WiredChange::Remove(LinkKind::Usb4),
            WiredChange::Add(LinkCandidate {
                kind: LinkKind::Usb4,
                addr: peer(&new).unwrap(),
            }),
        ]
    );
}

// --- With the link manager and a transport double ---

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
async fn a_usb4_interface_appearing_creates_the_link_and_its_removal_drops_it_at_once() {
    let wifi = LinkCandidate {
        kind: LinkKind::Wifi,
        addr: "192.168.1.30:47770".parse().unwrap(),
    };
    let (manager, _events) =
        LinkManager::spawn(&[wifi], AlwaysUp, HealthConfig::default()).unwrap();
    let ifaces = FakeInterfaces::new(vec![iface(1, "wlan0", &["192.168.1.5"])]);
    let _follow = follow_wired(&ifaces, manager.control(), resolve);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(manager.active().map(|a| a.kind), Some(LinkKind::Wifi));

    ifaces.plug(iface(2, "USB4 P2P Network Adapter", &["10.77.1.1"]));
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(manager.active().map(|a| a.kind), Some(LinkKind::Usb4));

    let before = tokio::time::Instant::now();
    ifaces.unplug(2);
    let mut rx = manager.watch();
    rx.wait_for(|s| s.active.as_ref().is_some_and(|a| a.kind == LinkKind::Wifi))
        .await
        .unwrap();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(manager.links().len(), 1);
}

#[tokio::test]
async fn the_double_publishes_every_change() {
    let f = FakeInterfaces::new(vec![]);
    let mut rx = f.watch();
    f.plug(iface(1, "bridge0", &[]));
    rx.changed().await.unwrap();
    assert_eq!(rx.borrow().len(), 1);
    f.set_up(1, false);
    rx.changed().await.unwrap();
    assert!(!f.list()[0].up);
    f.unplug(1);
    rx.changed().await.unwrap();
    assert!(f.list().is_empty());
    let _: Arc<dyn Interfaces> = Arc::new(f);
}

// --- Per-interface discovery and fallback IP assignment ---

fn resolver(
    side: Machine,
    discovery: &Arc<FakeDiscovery>,
    assigner: &Arc<FakeAssigner>,
) -> WiredResolver {
    WiredResolver {
        side,
        os: Os::Windows,
        port: PORT,
        discovery: discovery.clone(),
        assigner: assigner.clone(),
        notices: AssignNotices::default(),
    }
}

#[test]
fn with_link_local_the_cable_comes_in_when_the_other_answers_on_that_interface() {
    let (discovery, assigner) = (Arc::new(FakeDiscovery::default()), Arc::default());
    let r = resolver(Machine::A, &discovery, &assigner);
    let usb = iface(7, "bridge0", &["fe80::1"]);
    // Without an answer from the other, the cable does not come in.
    assert_eq!(r.resolve(&usb), None);
    // It announces on another interface: neither.
    discovery.announce(9, "[fe80::99]:47770".parse().unwrap());
    assert_eq!(r.resolve(&usb), None);
    // It answers on this one: its link-local is learned with the interface's scope.
    discovery.announce(7, "[fe80::2]:47770".parse().unwrap());
    let mut cur = Default::default();
    let changes = wired_changes(&mut cur, std::slice::from_ref(&usb), &|i| r.resolve(i));
    let want = scoped_addr("fe80::2".parse().unwrap(), 7, PORT);
    assert_eq!(
        changes,
        [WiredChange::Add(LinkCandidate {
            kind: LinkKind::Usb4,
            addr: want
        })]
    );
    assert!(assigner.calls().is_empty(), "with fe80 nothing is assigned");
}

#[test]
fn real_discovery_filters_by_scope_id() {
    let announced: Vec<SocketAddr> = vec![
        "[fe80::5%3]:47770".parse().unwrap(),
        "[fe80::6%7]:47770".parse().unwrap(),
        "192.168.1.5:47770".parse().unwrap(),
    ];
    assert_eq!(
        scoped_peer(&announced, 7),
        Some("[fe80::6]:47770".parse().unwrap())
    );
    assert_eq!(scoped_peer(&announced, 8), None);
    let (_tx, rx) = tokio::sync::watch::channel(announced);
    let d = ScopedDiscovery::new(rx);
    assert_eq!(
        d.peer_addr(&iface(3, "en5", &["fe80::1"])),
        Some("[fe80::5]:47770".parse().unwrap())
    );
}

#[test]
fn without_link_local_the_fallback_is_assigned_and_there_is_no_notice() {
    let (discovery, assigner) = (Arc::new(FakeDiscovery::default()), Arc::default());
    let r = resolver(Machine::B, &discovery, &assigner);
    let usb = iface(4, "USB4 P2P Network Adapter", &[]);
    let peer = r.resolve(&usb).unwrap();
    assert_eq!(peer, "10.77.1.1:47770".parse().unwrap());
    assert_eq!(
        assigner.calls(),
        [(
            "USB4 P2P Network Adapter".to_owned(),
            Ipv4Addr::new(10, 77, 1, 2)
        )]
    );
    assert!(r.notices.list().is_empty());
}

#[test]
fn without_permissions_the_notice_with_the_exact_command_remains_and_is_cleared_on_assigning() {
    let discovery = Arc::new(FakeDiscovery::default());
    let assigner = Arc::new(FakeAssigner::failing(AssignError::PermissionDenied));
    let r = resolver(Machine::A, &discovery, &assigner);
    let usb = iface(4, "Ethernet 3", &[]);
    // The link comes in anyway through the fallback: the user runs the command.
    assert_eq!(r.resolve(&usb), Some("10.77.1.2:47770".parse().unwrap()));
    let notices = r.notices.list();
    assert_eq!(notices.len(), 1);
    assert_eq!(
        notices[0].command,
        "netsh interface ip set address name=\"Ethernet 3\" static 10.77.1.1 255.255.255.252"
    );
    assert!(notices[0].message.contains("permission"));
    // The user ran it: the interface already has the IP and the notice is withdrawn.
    let done = iface(4, "Ethernet 3", &["10.77.1.1"]);
    r.resolve(&done);
    assert!(r.notices.list().is_empty());
}

#[test]
fn another_failure_also_notifies() {
    let discovery = Arc::new(FakeDiscovery::default());
    let assigner = Arc::new(FakeAssigner::failing(AssignError::Failed("boom".into())));
    let r = resolver(Machine::A, &discovery, &assigner);
    r.resolve(&iface(4, "Ethernet 3", &[]));
    assert!(r.notices.list()[0].message.contains("boom"));
}

#[test]
fn the_system_commands_and_the_permission_failures() {
    let ip = Ipv4Addr::new(10, 77, 1, 1);
    let (prog, args) = assign_command(Os::MacOs, "bridge0", ip);
    assert_eq!(prog, "ifconfig");
    assert_eq!(
        args.join(" "),
        "bridge0 inet 10.77.1.1 netmask 255.255.255.252"
    );
    let (prog, args) = assign_command(Os::Windows, "Ethernet 3", ip);
    assert_eq!(prog, "netsh");
    assert!(args.contains(&"name=Ethernet 3".to_owned()));
    assert!(is_permission_failure(
        "The requested operation requires elevation."
    ));
    assert!(is_permission_failure(
        "ifconfig: ioctl: Operation not permitted"
    ));
    assert!(!is_permission_failure("The interface is not found"));
}
