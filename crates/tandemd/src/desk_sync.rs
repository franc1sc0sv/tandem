//! The desk shared between the two machines (T-084, T-099).
//!
//! The desk ([`tandem_core::Desk`]: machines, arrangement, scenes, shortcuts, preferences and
//! modules) belongs to both, not to one machine: a change made on either holds for
//! both. Each agent keeps its copy in `tandem.toml` with its stamp (`desk_rev`, `desk_at`,
//! `desk_origin`) and sends it to the other on connecting and with each change
//! ([`tandem_proto::ControlMsg::Desk`]). The last one wins ([`tandem_core::DeskVersion`]).
//!
//! A hand-edited `tandem.toml` does not match the fingerprint the agent wrote
//! ([`fingerprint`]): on start it counts as a change by this machine.
//!
//! What is local-only (machine, identity, network, ports, paths, power) never travels.

use tandem_core::Desk;

/// Stable fingerprint of the desk (64-bit FNV-1a over its JSON), in hexadecimal: it does not depend
/// on the Rust version or the agent's, so it does not change on its own between starts.
pub fn fingerprint(desk: &Desk) -> String {
    // Serializing these types to JSON does not fail: the maps have text keys.
    let mut json = serde_json::to_vec(desk).unwrap_or_default();
    // A section added later counts only once it is used: a file written by an earlier version keeps
    // its fingerprint and is not taken for a hand edit after an update. `builtin_display` is the
    // last field of the desk (T-351).
    let unused = br#","builtin_display":{"a":null,"b":null}}"#;
    if json.ends_with(unused) {
        json.truncate(json.len() - unused.len());
        json.push(b'}');
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in json {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Whether the `tandem.toml` desk was changed by hand since the agent wrote it.
/// Without a fingerprint (a generated file or one from an earlier version), only if it is not the
/// factory one.
pub fn edited_by_hand(desk: &Desk, sum: Option<&str>) -> bool {
    match sum {
        Some(sum) => fingerprint(desk) != sum,
        None => *desk != Desk::default(),
    }
}

#[cfg(test)]
mod tests {
    use tandem_core::{Machine, Scale, ScreenSpec};

    use super::*;

    fn desk(ids: &[&str]) -> Desk {
        Desk {
            screens: ids
                .iter()
                .map(|id| ScreenSpec {
                    machine: Machine::B,
                    id: (*id).to_owned(),
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                    scale: Scale::ONE,
                })
                .collect(),
            ..Desk::default()
        }
    }

    /// The fingerprint of a desk that does not use `[builtin_display]` is the one earlier versions
    /// wrote (computed here as they did, without the field).
    #[test]
    fn an_unused_builtin_display_keeps_the_old_fingerprint() {
        let d = desk(&["34"]);
        let mut json = serde_json::to_vec(&d).unwrap();
        let cut = json.len() - br#","builtin_display":{"a":null,"b":null}}"#.len();
        json.truncate(cut);
        json.push(b'}');
        let back: Desk = serde_json::from_slice(&json).unwrap();
        assert_eq!(back, d, "what is hashed is the desk without the field");
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in json {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        assert_eq!(fingerprint(&d), format!("{h:016x}"));
        let mut used = d.clone();
        used.builtin_display
            .set(Machine::B, tandem_core::BuiltinMode::Extended);
        assert_ne!(fingerprint(&used), fingerprint(&d));
    }

    #[test]
    fn a_hand_edit_is_noticed_by_the_fingerprint() {
        let d = desk(&["a"]);
        let sum = fingerprint(&d);
        assert_eq!(sum.len(), 16);
        assert!(!edited_by_hand(&d, Some(&sum)));
        assert!(edited_by_hand(&desk(&["b"]), Some(&sum)));
        let mut lock = d.clone();
        lock.shortcuts.lock.clear();
        assert!(edited_by_hand(&lock, Some(&sum)));
        // Without a fingerprint: the factory one is not an edit; anything else is.
        assert!(!edited_by_hand(&Desk::default(), None));
        assert!(edited_by_hand(&d, None));
        let mut prefs = Desk::default();
        prefs.prefs.drag_files = false;
        assert!(edited_by_hand(&prefs, None));
        // The fingerprint is stable: the same input gives the same.
        assert_eq!(fingerprint(&d), sum);
    }
}
