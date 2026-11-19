use ed25519_dalek::pkcs8::EncodePrivateKey;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use zeroize::Zeroizing;

use super::{Identity, IdentityError};

/// Self-signed X.509 certificate with the identity's key, for TLS/QUIC.
///
/// There is no CA: the peer verifies by pinning the public key against its
/// [`TrustStore`](super::TrustStore), so name and validity are not used
/// to decide trust.
pub struct TlsIdentity {
    /// Certificate in DER.
    pub cert_der: Vec<u8>,
    /// Private key in PKCS#8 DER; wiped from memory on drop.
    pub key_pkcs8_der: Zeroizing<Vec<u8>>,
}

/// Fixed subject name and SAN; with pinning it adds no security, but rustls
/// requires a valid server name on connect.
pub const TLS_SERVER_NAME: &str = "tandem";

impl Identity {
    /// Generates the self-signed certificate derived from this identity.
    pub fn tls_certificate(&self) -> Result<TlsIdentity, IdentityError> {
        let pkcs8 = self
            .signing_key()
            .to_pkcs8_der()
            .map_err(|e| IdentityError::Pkcs8(e.to_string()))?;
        let key_pkcs8_der = Zeroizing::new(pkcs8.as_bytes().to_vec());
        let key_pair = KeyPair::try_from(key_pkcs8_der.as_slice())?;

        let mut params = CertificateParams::new(vec![TLS_SERVER_NAME.to_owned()])?;
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, TLS_SERVER_NAME);
        params.distinguished_name = dn;
        let cert = params.self_signed(&key_pair)?;

        Ok(TlsIdentity {
            cert_der: cert.der().to_vec(),
            key_pkcs8_der,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::pkcs8::EncodePublicKey;

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn the_certificate_carries_the_identitys_public_key() {
        let id = Identity::from_seed([3; 32]);
        let tls = id.tls_certificate().unwrap();
        let spki = id
            .signing_key()
            .verifying_key()
            .to_public_key_der()
            .unwrap();
        assert!(contains(&tls.cert_der, spki.as_bytes()));

        let other = Identity::from_seed([4; 32]);
        let other_spki = other
            .signing_key()
            .verifying_key()
            .to_public_key_der()
            .unwrap();
        assert!(!contains(&tls.cert_der, other_spki.as_bytes()));
    }

    #[test]
    fn the_private_key_is_usable_by_rcgen() {
        let id = Identity::from_seed([5; 32]);
        let tls = id.tls_certificate().unwrap();
        let kp = KeyPair::try_from(tls.key_pkcs8_der.as_slice()).unwrap();
        assert_eq!(kp.public_key_raw(), id.public_key().as_bytes());
    }
}
