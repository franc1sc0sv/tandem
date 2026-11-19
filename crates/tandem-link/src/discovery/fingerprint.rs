use std::fmt;
use std::str::FromStr;

use sha2::{Digest, Sha256};

use super::DiscoveryError;
use crate::identity::PublicKey;

/// Fingerprint of an agent's public key: SHA-256 of its 32 bytes.
///
/// It is what is announced over mDNS. It is not a secret and does not replace the
/// transport's pinning: it only lets each agent recognize, among the
/// announcements on the LAN, the one from its paired peer.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fingerprint([u8; 32]);

const DOMAIN: &[u8] = b"tandem/fingerprint/v1";

impl Fingerprint {
    pub fn of(key: &PublicKey) -> Self {
        let digest = Sha256::new()
            .chain_update(DOMAIN)
            .chain_update(key.as_bytes())
            .finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&digest);
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Short prefix for instance and host names (DNS labels
    /// have a maximum of 63 bytes).
    pub fn short(&self) -> String {
        self.0[..6].iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl From<&PublicKey> for Fingerprint {
    fn from(key: &PublicKey) -> Self {
        Self::of(key)
    }
}

impl FromStr for Fingerprint {
    type Err = DiscoveryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.len() != 64 || !s.is_ascii() {
            return Err(DiscoveryError::InvalidFingerprint);
        }
        let mut bytes = [0u8; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)
                .map_err(|_| DiscoveryError::InvalidFingerprint)?;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({})", self.short())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::identity::Identity;

    fn key(seed: u8) -> PublicKey {
        Identity::from_seed([seed; 32]).public_key()
    }

    #[test]
    fn is_stable_and_tells_keys_apart() {
        assert_eq!(Fingerprint::of(&key(1)), Fingerprint::of(&key(1)));
        assert_ne!(Fingerprint::of(&key(1)), Fingerprint::of(&key(2)));
        // It is not the key in the clear.
        assert_ne!(Fingerprint::of(&key(1)).as_bytes(), key(1).as_bytes());
    }

    #[test]
    fn the_short_prefix_fits_in_a_dns_label() {
        let fp = Fingerprint::of(&key(3));
        assert_eq!(fp.short().len(), 12);
        assert!(fp.to_hex().starts_with(&fp.short()));
    }

    #[test]
    fn rejects_malformed_hex() {
        assert!("".parse::<Fingerprint>().is_err());
        assert!("ab".repeat(31).parse::<Fingerprint>().is_err());
        assert!("zz".repeat(32).parse::<Fingerprint>().is_err());
        assert!("é".repeat(32).parse::<Fingerprint>().is_err());
    }

    proptest! {
        #[test]
        fn hex_round_trip(bytes in any::<[u8; 32]>()) {
            let fp = Fingerprint(bytes);
            prop_assert_eq!(fp.to_hex().parse::<Fingerprint>().unwrap(), fp);
            prop_assert_eq!(fp.to_hex().to_uppercase().parse::<Fingerprint>().unwrap(), fp);
        }
    }
}
