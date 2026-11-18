//! Migration of the v0.2 `tandem.toml` (`mac`/`pc` sides) to the A/B sides resolved by
//! identity (T-097), with the examples in `docs/examples/v0.2/`.
//!
//! What matters: after migrating, both machines have the same desk (each screen
//! on its machine's side, the arrangement the same) whichever one ended up as `A`.

use std::sync::Arc;

use tandem_core::{Machine, Side};
use tandem_link::identity::{Identity, TrustStore, TrustedPeer};
use tandemd::AgentConfig;
use tandemd::daemon::{self, Backends};
use tandemd::network::NetOptions;
use tandemd::orchestrator::Unimplemented;
use tandemd::paths::Paths;

const MAC: &str = include_str!("../../../docs/examples/v0.2/tandem.mac.toml");
const PC: &str = include_str!("../../../docs/examples/v0.2/tandem.pc.toml");

/// The two keys, in one order or the other according to `mac_low`.
fn keys(mac_low: bool) -> (Identity, Identity) {
    let (x, y) = (Identity::from_seed([7; 32]), Identity::from_seed([8; 32]));
    let (low, high) = if x.public_key().as_bytes() < y.public_key().as_bytes() {
        (x, y)
    } else {
        (y, x)
    };
    if mac_low { (low, high) } else { (high, low) }
}

/// What a machine's agent would do on start: read and resolve the side.
fn migrate(text: &str, own: &Identity, peer: &Identity) -> AgentConfig {
    let cfg = AgentConfig::from_toml(text).unwrap();
    assert!(
        cfg.rewrite_sides,
        "a v0.2 file has to be rewritten"
    );
    let side = Machine::of_keys(own.public_key().as_bytes(), peer.public_key().as_bytes()).unwrap();
    cfg.with_side(side)
}

#[test]
fn both_old_examples_end_up_with_the_same_desk_whatever_side_each_gets() {
    for mac_low in [true, false] {
        let (mac_id, pc_id) = keys(mac_low);
        let mac = migrate(MAC, &mac_id, &pc_id);
        let pc = migrate(PC, &pc_id, &mac_id);
        let (mac_side, pc_side) = if mac_low {
            (Machine::A, Machine::B)
        } else {
            (Machine::B, Machine::A)
        };
        assert_eq!((mac.machine, pc.machine), (mac_side, pc_side));
        // The same desk on both.
        assert_eq!(mac.desk, pc.desk, "mac_low = {mac_low}");
        // Each screen on its machine's side: the PC has the 34" and the 32" at 150%.
        let pc_screens: Vec<_> = mac.desk.screens_of(pc_side).map(|s| &s.id).collect();
        assert_eq!(pc_screens, ["34", "32"]);
        assert_eq!(mac.desk.screens_of(mac_side).count(), 1);
        // The Mac is still to the left of the PC.
        let a_side = if mac_side == Machine::A {
            Side::Left
        } else {
            Side::Right
        };
        let arr = mac.desk.arrangement.expect("v0.2 said where the Mac is");
        assert_eq!(arr.a_side, a_side, "mac_low = {mac_low}");
        // The keyboard started on the Mac, on both.
        assert_eq!(mac.input.keyboard, Some(mac_side));
        assert_eq!(pc.input.keyboard, Some(mac_side));
        // What is local does not change.
        assert!(mac.power.wol.is_some());
        assert_eq!(pc.network.peers.len(), 2);
    }
}

fn paths(dir: &std::path::Path) -> Paths {
    Paths {
        config: dir.join("tandem.toml"),
        data_dir: dir.to_owned(),
    }
}

/// End to end: an agent starts with the PC's v0.2 file and a peer whose key
/// is lower. It ends up as `B`, and the file ends up in the new format with everything translated.
#[tokio::test]
async fn an_agent_rewrites_an_old_file_with_the_sides_of_its_keys() {
    for mac_low in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        let (mac_id, pc_id) = keys(mac_low);
        pc_id.save(&paths.identity_file()).unwrap();
        TrustStore::open(paths.trust_file())
            .unwrap()
            .add_and_save(TrustedPeer {
                public_key: mac_id.public_key(),
                name: "MacBook".into(),
            })
            .unwrap();
        std::fs::write(&paths.config, PC).unwrap();
        let cfg = AgentConfig::load(&paths.config).unwrap();
        let net = NetOptions {
            bind: Some("127.0.0.1:0".parse().unwrap()),
            mdns: false,
            fallback_ports: Vec::new(),
            ..NetOptions::default()
        };
        let backends = Backends::Custom {
            local: Arc::new(Unimplemented),
            input: None,
            env: None,
        };
        let d = daemon::start(cfg, &paths, backends, net).unwrap();
        let pc_side = if mac_low { Machine::B } else { Machine::A };
        assert_eq!(d.agent.machine(), pc_side);

        let text = std::fs::read_to_string(&paths.config).unwrap();
        let saved = AgentConfig::from_toml(&text).unwrap();
        assert!(!saved.rewrite_sides, "already in the new format:\n{text}");
        assert_eq!(saved.machine, pc_side);
        assert_eq!(saved.input.keyboard, Some(pc_side.other()));
        let expected = migrate(PC, &pc_id, &mac_id);
        assert_eq!(saved.desk.screens, expected.desk.screens);
        assert_eq!(saved.desk.arrangement, expected.desk.arrangement);
        for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
            assert!(
                !line.contains("\"mac\"") && !line.contains("\"pc\"") && !line.contains("mac_side"),
                "a v0.2 name was left: {line}"
            );
        }
        // And on reading it again there is nothing left to translate.
        let again = AgentConfig::load(&paths.config).unwrap();
        assert!(!again.rewrite_sides);
        d.network.shutdown().await;
    }
}
