//! What the user sees about the link set (T-223): candidates left out after relabelling,
//! and a link manager that is not running. Before, both only reached the log.

use std::collections::BTreeMap;

use tandem_proto::NetworkNotice;

use crate::iface::Shadowed;
use crate::manager::{LinkKind, ManagerError};

/// One notice per kind with left-out candidates (the UI keys them by `interface`), plus one
/// if the manager could not start. Having no candidates yet is not trouble: the agent
/// waits to discover the peer.
pub fn link_notices(shadowed: &[Shadowed], failed: Option<&ManagerError>) -> Vec<NetworkNotice> {
    let mut by_kind: BTreeMap<LinkKind, (std::net::SocketAddr, Vec<String>)> = BTreeMap::new();
    for s in shadowed {
        by_kind
            .entry(s.kind)
            .or_insert_with(|| (s.kept, Vec::new()))
            .1
            .push(s.dropped.to_string());
    }
    let mut out: Vec<NetworkNotice> = by_kind
        .into_iter()
        .map(|(kind, (kept, dropped))| NetworkNotice {
            interface: format!("{kind} path"),
            message: format!(
                "{} also {} the {kind} path; only {kept} is used. Remove the extra peer from \
                 [network] in tandem.toml to silence this.",
                dropped.join(", "),
                if dropped.len() == 1 {
                    "reaches"
                } else {
                    "reach"
                },
            ),
            command: String::new(),
        })
        .collect();
    match failed {
        None | Some(ManagerError::NoCandidates) => {}
        Some(e) => out.push(NetworkNotice {
            interface: "Link manager".into(),
            message: format!("not running, so there is no link with the other machine: {e}"),
            command: String::new(),
        }),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shadowed(kind: LinkKind, kept: &str, dropped: &str) -> Shadowed {
        Shadowed {
            kind,
            kept: kept.parse().unwrap(),
            dropped: dropped.parse().unwrap(),
        }
    }

    #[test]
    fn nothing_to_say_when_all_is_well_or_the_peer_is_not_found_yet() {
        assert!(link_notices(&[], None).is_empty());
        assert!(link_notices(&[], Some(&ManagerError::NoCandidates)).is_empty());
    }

    #[test]
    fn a_shadowed_peer_names_both_addresses() {
        let n = link_notices(
            &[shadowed(
                LinkKind::Ethernet,
                "[fe80::2%3]:47770",
                "169.254.252.182:47770",
            )],
            None,
        );
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].interface, "Ethernet path");
        assert!(n[0].message.contains("[fe80::2%3]:47770"));
        assert!(n[0].message.contains("169.254.252.182:47770"));
    }

    #[test]
    fn one_notice_per_kind_so_the_ui_keys_stay_unique() {
        let n = link_notices(
            &[
                shadowed(LinkKind::Wifi, "192.168.1.2:1", "192.168.1.3:1"),
                shadowed(LinkKind::Wifi, "192.168.1.2:1", "192.168.1.4:1"),
                shadowed(LinkKind::Usb4, "169.254.1.1:1", "[fe80::1%4]:1"),
            ],
            Some(&ManagerError::ZeroDuration),
        );
        let keys: Vec<_> = n.iter().map(|n| n.interface.as_str()).collect();
        assert_eq!(keys, ["USB4 path", "Wi-Fi path", "Link manager"]);
        assert!(
            n[1].message
                .contains("192.168.1.3:1, 192.168.1.4:1 also reach")
        );
        assert!(n[2].message.contains("cannot be 0"));
    }
}
