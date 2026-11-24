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
            message: shadowed_text(kind, kept, &dropped),
            command: String::new(),
        })
        .collect();
    match failed {
        None | Some(ManagerError::NoCandidates) => {}
        Some(e) => out.push(NetworkNotice {
            interface: "Link manager".into(),
            message: format!(
                "The link manager is not running, so there is no link with the other machine: {e}"
            ),
            command: String::new(),
        }),
    }
    out
}

/// What Tandem did with the extra address(es) of a path. It says nothing about where the
/// address came from or how to remove it: a declared peer and a discovered one read the same,
/// and there is nothing for the user to edit (D4).
fn shadowed_text(kind: LinkKind, kept: std::net::SocketAddr, dropped: &[String]) -> String {
    let lead = if dropped.len() == 1 {
        "Two addresses reach"
    } else {
        "Several addresses reach"
    };
    format!(
        "{lead} the other machine over the {kind} path. Tandem uses {kept} and ignores {}.",
        dropped.join(", ")
    )
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
        assert_eq!(
            n[0].message,
            "Two addresses reach the other machine over the Ethernet path. Tandem uses \
             [fe80::2%3]:47770 and ignores 169.254.252.182:47770."
        );
    }

    #[test]
    fn no_text_asks_the_user_to_edit_the_config_file() {
        let n = link_notices(
            &[
                shadowed(LinkKind::Ethernet, "[fe80::2%3]:1", "169.254.1.2:1"),
                shadowed(LinkKind::Wifi, "192.168.1.2:1", "192.168.1.3:1"),
            ],
            Some(&ManagerError::ZeroDuration),
        );
        for notice in &n {
            for text in [&notice.interface, &notice.message, &notice.command] {
                let lower = text.to_lowercase();
                assert!(
                    !lower.contains("tandem.toml") && !lower.contains("[network]"),
                    "{text}"
                );
            }
        }
    }

    #[test]
    fn the_manager_notice_starts_with_a_capital_and_keeps_the_reason() {
        let n = link_notices(&[], Some(&ManagerError::ZeroDuration));
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].interface, "Link manager");
        assert_eq!(
            n[0].message,
            "The link manager is not running, so there is no link with the other machine: \
             the health check timings cannot be 0"
        );
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
        assert_eq!(
            n[1].message,
            "Several addresses reach the other machine over the Wi-Fi path. Tandem uses \
             192.168.1.2:1 and ignores 192.168.1.3:1, 192.168.1.4:1."
        );
        assert!(n[2].message.contains("cannot be 0"));
    }
}
