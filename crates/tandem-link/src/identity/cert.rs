use ed25519_dalek::pkcs8::EncodePrivateKey;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use zeroize::Zeroizing;

use super::{Identity, IdentityError};

/// Certificado X.509 autofirmado con la llave de la identidad, para TLS/QUIC.
///
/// No hay CA: el par verifica fijando la llave pública (pinning) contra su
/// [`TrustStore`](super::TrustStore), así que nombre y vigencia no se usan
/// para decidir confianza.
pub struct TlsIdentity {
    /// Certificado en DER.
    pub cert_der: Vec<u8>,
    /// Llave privada en PKCS#8 DER; se borra de memoria al soltarla.
    pub key_pkcs8_der: Zeroizing<Vec<u8>>,
}

/// Nombre fijo del sujeto y SAN; con pinning no aporta seguridad, pero rustls
/// exige un nombre de servidor válido al conectar.
pub const TLS_SERVER_NAME: &str = "tandem";

impl Identity {
    /// Genera el certificado autofirmado derivado de esta identidad.
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
    fn el_certificado_lleva_la_llave_publica_de_la_identidad() {
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
    fn la_llave_privada_es_utilizable_por_rcgen() {
        let id = Identity::from_seed([5; 32]);
        let tls = id.tls_certificate().unwrap();
        let kp = KeyPair::try_from(tls.key_pkcs8_der.as_slice()).unwrap();
        assert_eq!(kp.public_key_raw(), id.public_key().as_bytes());
    }
}
