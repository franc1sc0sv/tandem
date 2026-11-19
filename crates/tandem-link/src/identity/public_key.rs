use std::fmt;
use std::str::FromStr;

use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};

use super::IdentityError;

/// Ed25519 public key of an agent: its identity to the others.
///
/// It is always a valid curve point; it is serialized as hex.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Result<Self, IdentityError> {
        VerifyingKey::from_bytes(&bytes)
            .map_err(|_| IdentityError::InvalidPublicKey("not a valid Ed25519 point"))?;
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub(crate) fn from_verifying(key: &VerifyingKey) -> Self {
        Self(key.to_bytes())
    }
}

impl FromStr for PublicKey {
    type Err = IdentityError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.len() != 64 || !s.is_ascii() {
            return Err(IdentityError::InvalidPublicKey("64 hex digits expected"));
        }
        let mut bytes = [0u8; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)
                .map_err(|_| IdentityError::InvalidPublicKey("invalid hex"))?;
        }
        Self::from_bytes(bytes)
    }
}

impl TryFrom<String> for PublicKey {
    type Error = IdentityError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<PublicKey> for String {
    fn from(key: PublicKey) -> Self {
        key.to_hex()
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    #[test]
    fn hex_round_trip() {
        let key = Identity::from_seed([7; 32]).public_key();
        let parsed: PublicKey = key.to_hex().parse().unwrap();
        assert_eq!(parsed, key);
        assert_eq!(key.to_string().len(), 64);
    }

    #[test]
    fn rejects_malformed_hex() {
        assert!("abc".parse::<PublicKey>().is_err());
        assert!("zz".repeat(32).parse::<PublicKey>().is_err());
        assert!("é".repeat(32).parse::<PublicKey>().is_err());
    }

    #[test]
    fn rejects_an_invalid_point() {
        // y = 2 is not on the Edwards curve.
        let mut bytes = [0u8; 32];
        bytes[0] = 2;
        assert!(PublicKey::from_bytes(bytes).is_err());
    }
}
