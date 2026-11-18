//! `tandemd links`: what the agent sees of the network, for spike S5 and to debug the wired
//! link (T-094). Lists the interfaces with the detected type, the address the link would use and,
//! if the agent is running, the state of each path.

use std::fmt::Write;

use tandem_core::{Machine, Os};
use tandem_link::iface::{InterfaceKind, NetInterface, plan};
use tandem_proto::{Link, NetworkStatus, PathState};

fn kind_name(k: InterfaceKind) -> &'static str {
    match k {
        InterfaceKind::Usb4 => "USB4",
        InterfaceKind::Ethernet => "Ethernet",
        InterfaceKind::Wifi => "Wi-Fi",
        InterfaceKind::Other => "other (counts as Wi-Fi)",
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
        PathState::Primary => "primary",
        PathState::Backup => "backup",
        PathState::Unplugged => "no cable",
        PathState::Down => "down",
    }
}

/// The text report. `status` is `None` if the agent is not running.
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
            if i.up { "up" } else { "down" },
            if addrs.is_empty() {
                "no addresses".to_owned()
            } else {
                addrs.join(", ")
            },
        );
        if matches!(i.kind, InterfaceKind::Usb4 | InterfaceKind::Ethernet) {
            let p = plan(i, side, os, None, port);
            let _ = writeln!(out, "      local: {}", p.local);
            let _ = match p.peer {
                Some(peer) => writeln!(out, "      other agent: {peer}"),
                None => writeln!(
                    out,
                    "      other agent: to be discovered (its link-local comes from mDNS on the interface)"
                ),
            };
            if let Some(order) = p.assign {
                let _ = writeln!(
                    out,
                    "      missing backup IP; assign it with: {}",
                    order.command
                );
            }
        }
    }
    match status {
        None => {
            let _ = writeln!(
                out,
                "Paths: the agent is not running (no state to show)."
            );
        }
        Some(s) => {
            let _ = writeln!(out, "Paths:");
            for p in &s.paths {
                let rtt = p.rtt_us.map_or_else(
                    || "no reply".to_owned(),
                    |us| format!("{:.1} ms", f64::from(us) / 1000.0),
                );
                let bw = p.bandwidth.map_or_else(String::new, |b| {
                    format!(", {:.1} MB/s measured", b.bytes_per_sec as f64 / 1e6)
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
    fn lists_the_type_the_address_and_the_missing_command() {
        let ifs = [
            iface(1, "Wi-Fi", InterfaceKind::Wifi, &["192.168.1.5"]),
            iface(7, "USB4 P2P Network Adapter", InterfaceKind::Usb4, &[]),
        ];
        let text = render(&ifs, Machine::B, Os::Windows, 52000, None);
        assert!(
            text.contains("[1] Wi-Fi - Wi-Fi - up - 192.168.1.5"),
            "{text}"
        );
        assert!(text.contains("USB4 P2P Network Adapter - USB4"), "{text}");
        assert!(text.contains("local: 10.77.1.2"), "{text}");
        assert!(text.contains("other agent: 10.77.1.1:52000"), "{text}");
        assert!(text.contains("netsh interface ip set address"), "{text}");
        assert!(text.contains("the agent is not running"), "{text}");
    }

    #[test]
    fn with_link_local_the_other_is_discovered() {
        let ifs = [iface(9, "bridge0", InterfaceKind::Usb4, &["fe80::1"])];
        let text = render(&ifs, Machine::A, Os::MacOs, 52000, None);
        assert!(text.contains("local: fe80::1"), "{text}");
        assert!(text.contains("to be discovered"), "{text}");
    }
}
