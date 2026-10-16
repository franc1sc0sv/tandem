use std::fmt;
use std::str::FromStr;

use sha2::{Digest, Sha256};

use super::DiscoveryError;
use crate::identity::PublicKey;

/// Huella de la llave pública de un agente: SHA-256 de sus 32 bytes.
///
/// Es lo que se anuncia por mDNS. No es un secreto ni sustituye al pinning
/// del transporte: solo sirve para que cada agente reconozca, entre los
/// anuncios de la LAN, el de su par emparejado.
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

    /// Prefijo corto para nombres de instancia y de host (las etiquetas DNS
    /// tienen un máximo de 63 bytes).
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
    fn es_estable_y_distingue_llaves() {
        assert_eq!(Fingerprint::of(&key(1)), Fingerprint::of(&key(1)));
        assert_ne!(Fingerprint::of(&key(1)), Fingerprint::of(&key(2)));
        // No es la llave en claro.
        assert_ne!(Fingerprint::of(&key(1)).as_bytes(), key(1).as_bytes());
    }

    #[test]
    fn el_prefijo_corto_cabe_en_una_etiqueta_dns() {
        let fp = Fingerprint::of(&key(3));
        assert_eq!(fp.short().len(), 12);
        assert!(fp.to_hex().starts_with(&fp.short()));
    }

    #[test]
    fn rechaza_hex_mal_formado() {
        assert!("".parse::<Fingerprint>().is_err());
        assert!("ab".repeat(31).parse::<Fingerprint>().is_err());
        assert!("zz".repeat(32).parse::<Fingerprint>().is_err());
        assert!("é".repeat(32).parse::<Fingerprint>().is_err());
    }

    proptest! {
        #[test]
        fn hex_ida_y_vuelta(bytes in any::<[u8; 32]>()) {
            let fp = Fingerprint(bytes);
            prop_assert_eq!(fp.to_hex().parse::<Fingerprint>().unwrap(), fp);
            prop_assert_eq!(fp.to_hex().to_uppercase().parse::<Fingerprint>().unwrap(), fp);
        }
    }
}
