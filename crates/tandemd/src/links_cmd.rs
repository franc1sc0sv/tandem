//! `tandemd links`: qué ve el agente de la red, para el spike S5 y para depurar el enlace
//! por cable (T-094). Lista las interfaces con el tipo detectado, la dirección que usaría el
//! enlace y, si el agente está corriendo, el estado de cada camino.

use std::fmt::Write;

use tandem_core::{Machine, Os};
use tandem_link::iface::{InterfaceKind, NetInterface, plan};
use tandem_proto::{Link, NetworkStatus, PathState};

fn kind_name(k: InterfaceKind) -> &'static str {
    match k {
        InterfaceKind::Usb4 => "USB4",
        InterfaceKind::Ethernet => "Ethernet",
        InterfaceKind::Wifi => "Wi-Fi",
        InterfaceKind::Other => "otro (cuenta como Wi-Fi)",
    }
}

fn link_name(l: Link) -> &'static str {
    match l {
        Link::Usb4 => "USB4",
        Link::Ethernet => "Ethernet",
        Link::Wifi => "Wi-Fi",
    }
}

fn state_name(s: PathState) -> &'static str {
    match s {
        PathState::Primary => "principal",
        PathState::Backup => "de respaldo",
        PathState::Unplugged => "sin cable",
        PathState::Down => "caído",
    }
}

/// El informe en texto. `status` es `None` si el agente no está corriendo.
pub fn render(
    interfaces: &[NetInterface],
    side: Machine,
    os: Os,
    port: u16,
    status: Option<&NetworkStatus>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Interfaces ({}):", interfaces.len());
    for i in interfaces {
        let addrs: Vec<String> = i.addrs.iter().map(ToString::to_string).collect();
        let _ = writeln!(
            out,
            "  [{}] {} - {} - {} - {}",
            i.index,
            i.name,
            kind_name(i.kind),
            if i.up { "arriba" } else { "abajo" },
            if addrs.is_empty() {
                "sin direcciones".to_owned()
            } else {
                addrs.join(", ")
            },
        );
        if matches!(i.kind, InterfaceKind::Usb4 | InterfaceKind::Ethernet) {
            let p = plan(i, side, os, None, port);
            let _ = writeln!(out, "      local: {}", p.local);
            let _ = match p.peer {
                Some(peer) => writeln!(out, "      otro agente: {peer}"),
                None => writeln!(
                    out,
                    "      otro agente: por descubrir (su link-local sale de mDNS en la interfaz)"
                ),
            };
            if let Some(order) = p.assign {
                let _ = writeln!(
                    out,
                    "      falta la IP de respaldo; asígnala con: {}",
                    order.command
                );
            }
        }
    }
    match status {
        None => {
            let _ = writeln!(
                out,
                "Caminos: el agente no está corriendo (no hay estado que mostrar)."
            );
        }
        Some(s) => {
            let _ = writeln!(out, "Caminos:");
            for p in &s.paths {
                let rtt = p.rtt_us.map_or_else(
                    || "sin respuesta".to_owned(),
                    |us| format!("{:.1} ms", f64::from(us) / 1000.0),
                );
                let bw = p.bandwidth.map_or_else(String::new, |b| {
                    format!(", {:.1} MB/s medidos", b.bytes_per_sec as f64 / 1e6)
                });
                let _ = writeln!(
                    out,
                    "  {} - {} - {rtt}{bw}",
                    link_name(p.link),
                    state_name(p.state)
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(index: u32, name: &str, kind: InterfaceKind, addrs: &[&str]) -> NetInterface {
        NetInterface {
            index,
            name: name.into(),
            kind,
            addrs: addrs.iter().map(|a| a.parse().unwrap()).collect(),
            mac: None,
            speed_mbps: None,
            up: true,
        }
    }

    #[test]
    fn lista_el_tipo_la_direccion_y_la_orden_que_falta() {
        let ifs = [
            iface(1, "Wi-Fi", InterfaceKind::Wifi, &["192.168.1.5"]),
            iface(7, "USB4 P2P Network Adapter", InterfaceKind::Usb4, &[]),
        ];
        let text = render(&ifs, Machine::B, Os::Windows, 52000, None);
        assert!(
            text.contains("[1] Wi-Fi - Wi-Fi - arriba - 192.168.1.5"),
            "{text}"
        );
        assert!(text.contains("USB4 P2P Network Adapter - USB4"), "{text}");
        assert!(text.contains("local: 10.77.1.2"), "{text}");
        assert!(text.contains("otro agente: 10.77.1.1:52000"), "{text}");
        assert!(text.contains("netsh interface ip set address"), "{text}");
        assert!(text.contains("el agente no está corriendo"), "{text}");
    }

    #[test]
    fn con_link_local_el_otro_se_descubre() {
        let ifs = [iface(9, "bridge0", InterfaceKind::Usb4, &["fe80::1"])];
        let text = render(&ifs, Machine::A, Os::MacOs, 52000, None);
        assert!(text.contains("local: fe80::1"), "{text}");
        assert!(text.contains("por descubrir"), "{text}");
    }
}
