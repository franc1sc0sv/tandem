//! Identities and config of the test agents, in a temporary folder.
//!
//! The two machines pair without ceremony: each one brings its seed and the other's public
//! key already in its trust. The seeds are test constants, unrelated to the user's real
//! identities, and the test agent never sees the
//! real installation's data directory.

use std::path::{Path, PathBuf};

use tandem_core::Machine;
use tandem_link::identity::{Identity, TrustStore, TrustedPeer};

/// UDP port of the test agents: different from the real installation's.
pub const TEST_PORT: u16 = 47871;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Pc,
    Mac,
}

impl Role {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pc" => Some(Role::Pc),
            "mac" => Some(Role::Mac),
            _ => None,
        }
    }

    fn seed(self) -> [u8; 32] {
        match self {
            Role::Pc => [0x51; 32],
            Role::Mac => [0x52; 32],
        }
    }

    fn other(self) -> Role {
        match self {
            Role::Pc => Role::Mac,
            Role::Mac => Role::Pc,
        }
    }
}

/// `A` is the one with the lower public key (DECISIONS A1).
pub fn machine_of(role: Role) -> Machine {
    let mine = Identity::from_seed(role.seed()).public_key();
    let theirs = Identity::from_seed(role.other().seed()).public_key();
    if mine.as_bytes() < theirs.as_bytes() {
        Machine::A
    } else {
        Machine::B
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provisioned {
    pub machine: Machine,
    pub config: PathBuf,
    pub data_dir: PathBuf,
}

/// Writes `<root>/data` (identity and trust) and `<root>/tandem.toml`.
/// `peer_addr` is `ip:port` of the other machine.
pub fn provision(
    root: &Path,
    role: Role,
    port: u16,
    peer_addr: &str,
) -> anyhow::Result<Provisioned> {
    let data_dir = root.join("data");
    std::fs::create_dir_all(&data_dir)?;
    Identity::from_seed(role.seed()).save(&data_dir.join("identity.key"))?;
    TrustStore::open(data_dir.join("trust.toml"))?.add_and_save(TrustedPeer {
        public_key: Identity::from_seed(role.other().seed()).public_key(),
        name: "hw-test-peer".into(),
    })?;
    let machine = machine_of(role);
    let config = root.join("tandem.toml");
    std::fs::write(
        &config,
        format!(
            "machine = \"{machine}\"\n[network]\nport = {port}\n\
             peers = [{{ kind = \"wifi\", addr = \"{peer_addr}\" }}]\n"
        ),
    )?;
    Ok(Provisioned {
        machine,
        config,
        data_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tandemd::config::AgentConfig;

    #[test]
    fn the_two_roles_get_different_machines() {
        assert_ne!(machine_of(Role::Pc), machine_of(Role::Mac));
    }

    #[test]
    fn each_side_trusts_exactly_the_other() {
        let dir = tempfile::tempdir().unwrap();
        for role in [Role::Pc, Role::Mac] {
            let p = provision(
                &dir.path().join(format!("{role:?}")),
                role,
                1234,
                "10.0.0.2:1234",
            )
            .unwrap();
            let trust = TrustStore::open(p.data_dir.join("trust.toml")).unwrap();
            let other = Identity::from_seed(role.other().seed()).public_key();
            assert!(trust.is_trusted(&other));
            let mine = Identity::from_seed(role.seed()).public_key();
            assert!(!trust.is_trusted(&mine));
        }
    }

    #[test]
    fn the_generated_config_is_valid_and_uses_the_test_port() {
        let dir = tempfile::tempdir().unwrap();
        let p = provision(dir.path(), Role::Pc, TEST_PORT, "192.0.2.7:47871").unwrap();
        let cfg = AgentConfig::load(&p.config).unwrap();
        assert_eq!(cfg.network.port, TEST_PORT);
        assert_eq!(cfg.network.peers.len(), 1);
        assert_ne!(TEST_PORT, tandemd::config::DEFAULT_PORT);
    }

    #[test]
    fn nothing_leaves_the_given_folder() {
        let dir = tempfile::tempdir().unwrap();
        let p = provision(dir.path(), Role::Mac, 1, "192.0.2.7:1").unwrap();
        assert!(p.data_dir.starts_with(dir.path()));
        assert!(p.config.starts_with(dir.path()));
    }
}
