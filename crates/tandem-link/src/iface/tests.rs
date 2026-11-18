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
fn el_tipo_sale_del_nombre_y_lo_dudoso_no_es_cable() {
    for (name, kind) in [
        ("USB4 P2P Network Adapter", InterfaceKind::Usb4),
        ("Thunderbolt Networking", InterfaceKind::Usb4),
        ("bridge0", InterfaceKind::Usb4),
        ("Wi-Fi", InterfaceKind::Wifi),
        ("wlp3s0", InterfaceKind::Wifi),
        ("Ethernet 2", InterfaceKind::Ethernet),
        ("enp5s0", InterfaceKind::Ethernet),
        // macOS: puede ser Wi-Fi o Ethernet, no se adivina.
        ("en0", InterfaceKind::Other),
        ("utun3", InterfaceKind::Other),
    ] {
        assert_eq!(classify(name), kind, "{name}");
    }
    // Lo desconocido cuenta como Wi-Fi para el enlace.
    assert_eq!(InterfaceKind::Other.link_kind(), LinkKind::Wifi);
}

#[test]
fn la_etiqueta_sale_de_la_interfaz_aunque_la_config_diga_otra_cosa() {
    let ifs = [
        iface(5, "bridge0", &["10.77.1.1"]),
        iface(6, "en0", &["192.168.1.20"]),
    ];
    let cands = [
        // La config dice Ethernet, pero esa red es la del cable USB4.
        LinkCandidate {
            kind: LinkKind::Ethernet,
            addr: "10.77.1.2:47770".parse().unwrap(),
        },
        LinkCandidate {
            kind: LinkKind::Usb4,
            addr: "192.168.1.30:47770".parse().unwrap(),
        },
        // Sin interfaz conocida se respeta la config.
        LinkCandidate {
            kind: LinkKind::Ethernet,
            addr: "172.16.0.2:47770".parse().unwrap(),
        },
    ];
    let kinds: Vec<_> = relabel(&cands, &ifs).iter().map(|c| c.kind).collect();
    assert_eq!(kinds, [LinkKind::Usb4, LinkKind::Wifi, LinkKind::Ethernet]);
}

#[test]
fn una_direccion_link_local_se_etiqueta_por_su_scope_id() {
    let ifs = [iface(7, "USB4 P2P Network Adapter", &["fe80::1"])];
    let on = |scope| -> SocketAddr { format!("[fe80::2%{scope}]:47770").parse().unwrap() };
    assert_eq!(kind_for_addr(&ifs, on(7)), Some(LinkKind::Usb4));
    assert_eq!(kind_for_addr(&ifs, on(8)), None);
}

#[test]
fn la_peor_etiqueta_gana() {
    use LinkKind::*;
    // PC Ethernet, Mac Wi-Fi: Wi-Fi, sin cable.
    assert_eq!(worst(Ethernet, Wifi), Wifi);
    assert_eq!(worst(Usb4, Ethernet), Ethernet);
    assert_eq!(worst(Usb4, Usb4), Usb4);
    assert!(!worst(Ethernet, Wifi).is_wired());
}

proptest! {
    #[test]
    fn worst_es_conmutativa_idempotente_y_nunca_mejora(a in 0usize..3, b in 0usize..3) {
        let (a, b) = (LinkKind::ALL[a], LinkKind::ALL[b]);
        prop_assert_eq!(worst(a, b), worst(b, a));
        prop_assert_eq!(worst(a, a), a);
        prop_assert!(worst(a, b) >= a && worst(a, b) >= b);
    }
}

#[test]
fn el_scope_id_va_en_la_direccion_link_local_y_solo_en_ella() {
    let ll: IpAddr = "fe80::1c2:3ff:fe04:506".parse().unwrap();
    let SocketAddr::V6(v6) = scoped_addr(ll, 12, PORT) else {
        panic!("se esperaba IPv6");
    };
    assert_eq!((v6.scope_id(), v6.port()), (12, PORT));
    assert_eq!(
        *v6.ip(),
        "fe80::1c2:3ff:fe04:506".parse::<Ipv6Addr>().unwrap()
    );
    // Los límites del rango fe80::/10.
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
    // IPv4 no lleva scope.
    assert_eq!(
        scoped_addr("10.77.1.2".parse().unwrap(), 3, PORT).to_string(),
        "10.77.1.2:47770"
    );
}

#[test]
fn el_plan_usa_la_link_local_con_su_scope() {
    let i = iface(9, "bridge0", &["fe80::aa", "169.254.7.7"]);
    let peer = Some("[fe80::bb]:47999".parse().unwrap());
    let p = plan(&i, Machine::A, Os::MacOs, peer, PORT);
    assert_eq!(p.local, "fe80::aa".parse::<IpAddr>().unwrap());
    // El puerto es el que anuncia mDNS, no el configurado.
    assert_eq!(p.peer, Some("[fe80::bb%9]:47999".parse().unwrap()));
    assert_eq!(p.assign, None);
    // Sin saber dónde está el otro todavía (su link-local sale de mDNS), no hay dirección.
    assert_eq!(plan(&i, Machine::A, Os::MacOs, None, PORT).peer, None);
}

#[test]
fn sin_link_local_cada_lado_se_asigna_su_ip_de_respaldo() {
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

    // Con la IP ya asignada no hay nada que pedir.
    let done = iface(4, "bridge0", &["10.77.1.2"]);
    assert_eq!(plan(&done, Machine::B, Os::MacOs, None, PORT).assign, None);
}

#[test]
fn diff_da_altas_y_bajas_y_una_interfaz_caida_cuenta_como_baja() {
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
fn los_cambios_de_cable_son_la_diferencia_con_lo_aplicado() {
    let mut cur = Default::default();
    let wifi = iface(1, "wlan0", &["192.168.1.5"]);
    let usb = iface(2, "bridge0", &["10.77.1.1"]);
    let eth = iface(3, "Ethernet", &["10.77.2.1"]);

    // El Wi-Fi no es un cable.
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
    // Sin cambios, nada.
    assert_eq!(wired_changes(&mut cur, &all, &resolve), []);
    // Baja el USB4.
    assert_eq!(
        wired_changes(&mut cur, &[wifi, eth], &resolve),
        [WiredChange::Remove(LinkKind::Usb4)]
    );
}

#[test]
fn sin_direccion_del_otro_no_hay_enlace() {
    let mut cur = Default::default();
    let usb = iface(2, "bridge0", &["fe80::1"]);
    assert_eq!(wired_changes(&mut cur, &[usb], &|_| None), []);
}

// --- Con el gestor de enlaces y un doble del transporte ---

#[derive(Debug, thiserror::Error)]
#[error("sin respuesta")]
struct Silent;

/// Todo enlace responde.
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
async fn la_aparicion_de_una_interfaz_usb4_crea_el_enlace_y_su_baja_lo_quita_en_el_acto() {
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
async fn el_doble_publica_cada_cambio() {
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

// --- Descubrimiento por interfaz y asignación de la IP de respaldo ---

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
fn con_link_local_el_cable_entra_cuando_el_otro_contesta_por_esa_interfaz() {
    let (discovery, assigner) = (Arc::new(FakeDiscovery::default()), Arc::default());
    let r = resolver(Machine::A, &discovery, &assigner);
    let usb = iface(7, "bridge0", &["fe80::1"]);
    // Sin respuesta del otro, el cable no entra.
    assert_eq!(r.resolve(&usb), None);
    // Anuncia en otra interfaz: tampoco.
    discovery.announce(9, "[fe80::99]:47770".parse().unwrap());
    assert_eq!(r.resolve(&usb), None);
    // Contesta por esta: se aprende su link-local con el scope de la interfaz.
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
    assert!(assigner.calls().is_empty(), "con fe80 no se asigna nada");
}

#[test]
fn el_descubrimiento_real_filtra_por_scope_id() {
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
fn sin_link_local_se_asigna_el_respaldo_y_no_hay_aviso() {
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
fn sin_permisos_queda_el_aviso_con_la_orden_exacta_y_se_borra_al_asignarse() {
    let discovery = Arc::new(FakeDiscovery::default());
    let assigner = Arc::new(FakeAssigner::failing(AssignError::PermissionDenied));
    let r = resolver(Machine::A, &discovery, &assigner);
    let usb = iface(4, "Ethernet 3", &[]);
    // El enlace entra igual por el respaldo: la orden la ejecuta el usuario.
    assert_eq!(r.resolve(&usb), Some("10.77.1.2:47770".parse().unwrap()));
    let notices = r.notices.list();
    assert_eq!(notices.len(), 1);
    assert_eq!(
        notices[0].command,
        "netsh interface ip set address name=\"Ethernet 3\" static 10.77.1.1 255.255.255.252"
    );
    assert!(notices[0].message.contains("permisos"));
    // El usuario la ejecutó: la interfaz ya tiene la IP y el aviso se retira.
    let done = iface(4, "Ethernet 3", &["10.77.1.1"]);
    r.resolve(&done);
    assert!(r.notices.list().is_empty());
}

#[test]
fn otro_fallo_tambien_avisa() {
    let discovery = Arc::new(FakeDiscovery::default());
    let assigner = Arc::new(FakeAssigner::failing(AssignError::Failed("boom".into())));
    let r = resolver(Machine::A, &discovery, &assigner);
    r.resolve(&iface(4, "Ethernet 3", &[]));
    assert!(r.notices.list()[0].message.contains("boom"));
}

#[test]
fn las_ordenes_del_sistema_y_los_fallos_de_permisos() {
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
