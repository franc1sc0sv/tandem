//! rustls verifiers by pinning the peer's Ed25519 key.
//!
//! There is no CA or chain: the certificate only carries the public key. It is
//! accepted if that key is in the local trust and if the peer signed the
//! handshake with it (that is checked by `verify_tls13_signature`, which proves
//! it has the private key). Name, validity and extensions are ignored.

use std::fmt;
use std::sync::{Arc, RwLock};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::ParsedCertificate;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, DigitallySignedStruct, DistinguishedName, Error, PeerIncompatible,
    SignatureScheme,
};

use crate::identity::{PublicKey, TrustStore};

/// Decides which keys are trusted. The normal source is the [`TrustStore`];
/// with `RwLock` the peers paired after the endpoint was created count
/// for new connections.
pub trait PeerTrust: Send + Sync + 'static {
    fn is_trusted(&self, key: &PublicKey) -> bool;
}

impl PeerTrust for TrustStore {
    fn is_trusted(&self, key: &PublicKey) -> bool {
        TrustStore::is_trusted(self, key)
    }
}

impl<T: PeerTrust> PeerTrust for RwLock<T> {
    fn is_trusted(&self, key: &PublicKey) -> bool {
        // A reader leaves no data half done; if another thread panicked
        // while writing, the store is still a valid map.
        let guard = self.read().unwrap_or_else(|e| e.into_inner());
        guard.is_trusted(key)
    }
}

/// Accepts any key: only for the pairing endpoint, where the peer
/// is not yet trusted. The handshake signature is still checked.
pub(crate) struct AnyKey;

impl PeerTrust for AnyKey {
    fn is_trusted(&self, _key: &PublicKey) -> bool {
        true
    }
}

/// Fixed DER prefix of an Ed25519 SubjectPublicKeyInfo (RFC 8410): the
/// 32 bytes of the key follow.
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// The certificate's Ed25519 key, or `None` if it is not an X.509 certificate
/// with an Ed25519 key.
pub fn peer_public_key(cert: &CertificateDer<'_>) -> Option<PublicKey> {
    let parsed = ParsedCertificate::try_from(cert).ok()?;
    let spki = parsed.subject_public_key_info();
    let raw = spki.as_ref().strip_prefix(&ED25519_SPKI_PREFIX)?;
    let bytes: [u8; 32] = raw.try_into().ok()?;
    PublicKey::from_bytes(bytes).ok()
}

/// Logic common to both sides.
#[derive(Clone)]
struct Pinning {
    trust: Arc<dyn PeerTrust>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl Pinning {
    fn new(trust: Arc<dyn PeerTrust>, provider: &CryptoProvider) -> Self {
        Self {
            trust,
            algorithms: provider.signature_verification_algorithms,
        }
    }

    fn check(&self, end_entity: &CertificateDer<'_>) -> Result<(), Error> {
        let key = peer_public_key(end_entity)
            .ok_or(Error::InvalidCertificate(CertificateError::BadEncoding))?;
        if self.trust.is_trusted(&key) {
            Ok(())
        } else {
            tracing::warn!(peer = %key, "unpaired peer rejected");
            Err(Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        // Ed25519 only: it is the only key type an identity can have.
        if dss.scheme != SignatureScheme::ED25519 {
            return Err(Error::PeerIncompatible(
                PeerIncompatible::NoSignatureSchemesInCommon,
            ));
        }
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }
}

impl fmt::Debug for Pinning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Pinning")
    }
}

/// Client side: verifies the server's certificate.
#[derive(Debug)]
pub(crate) struct PinnedServerVerifier(Pinning);

impl PinnedServerVerifier {
    pub(crate) fn new(trust: Arc<dyn PeerTrust>, provider: &CryptoProvider) -> Self {
        Self(Pinning::new(trust, provider))
    }
}

impl ServerCertVerifier for PinnedServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        self.0.check(end_entity)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        // QUIC requires TLS 1.3; this should never be called.
        Err(Error::PeerIncompatible(
            PeerIncompatible::Tls13RequiredForQuic,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.0.verify_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

/// Server side: requires and verifies the client's certificate.
#[derive(Debug)]
pub(crate) struct PinnedClientVerifier(Pinning);

impl PinnedClientVerifier {
    pub(crate) fn new(trust: Arc<dyn PeerTrust>, provider: &CryptoProvider) -> Self {
        Self(Pinning::new(trust, provider))
    }
}

impl ClientCertVerifier for PinnedClientVerifier {
    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        self.0.check(end_entity)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::PeerIncompatible(
            PeerIncompatible::Tls13RequiredForQuic,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.0.verify_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::identity::Identity;

    struct Keys(BTreeSet<PublicKey>);

    impl PeerTrust for Keys {
        fn is_trusted(&self, key: &PublicKey) -> bool {
            self.0.contains(key)
        }
    }

    fn cert_of(seed: u8) -> CertificateDer<'static> {
        let tls = Identity::from_seed([seed; 32]).tls_certificate().unwrap();
        CertificateDer::from(tls.cert_der)
    }

    fn trusting(seeds: &[u8]) -> Arc<dyn PeerTrust> {
        Arc::new(Keys(
            seeds
                .iter()
                .map(|s| Identity::from_seed([*s; 32]).public_key())
                .collect(),
        ))
    }

    fn provider() -> CryptoProvider {
        rustls::crypto::ring::default_provider()
    }

    #[test]
    fn extracts_the_key_from_the_certificate() {
        let id = Identity::from_seed([1; 32]);
        assert_eq!(peer_public_key(&cert_of(1)), Some(id.public_key()));
    }

    #[test]
    fn a_garbage_certificate_has_no_key() {
        assert_eq!(peer_public_key(&CertificateDer::from(vec![0x30, 0])), None);
        assert_eq!(peer_public_key(&CertificateDer::from(Vec::new())), None);
    }

    #[test]
    fn a_certificate_with_another_curve_has_no_key() {
        let kp = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let cert = rcgen::CertificateParams::new(vec!["tandem".to_owned()])
            .unwrap()
            .self_signed(&kp)
            .unwrap();
        assert_eq!(peer_public_key(cert.der()), None);
    }

    #[test]
    fn a_paired_server_is_accepted_and_another_rejected() {
        let v = PinnedServerVerifier::new(trusting(&[1]), &provider());
        let name = ServerName::try_from("tandem").unwrap();
        let now = UnixTime::now();
        assert!(
            v.verify_server_cert(&cert_of(1), &[], &name, &[], now)
                .is_ok()
        );
        assert_eq!(
            v.verify_server_cert(&cert_of(2), &[], &name, &[], now)
                .unwrap_err(),
            Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure)
        );
        assert_eq!(
            v.verify_server_cert(&CertificateDer::from(vec![1, 2, 3]), &[], &name, &[], now)
                .unwrap_err(),
            Error::InvalidCertificate(CertificateError::BadEncoding)
        );
    }

    #[test]
    fn a_paired_client_is_accepted_and_another_rejected() {
        let v = PinnedClientVerifier::new(trusting(&[1, 3]), &provider());
        let now = UnixTime::now();
        assert!(v.client_auth_mandatory());
        assert!(v.verify_client_cert(&cert_of(1), &[], now).is_ok());
        assert!(v.verify_client_cert(&cert_of(3), &[], now).is_ok());
        assert!(v.verify_client_cert(&cert_of(2), &[], now).is_err());
    }

    #[test]
    fn only_ed25519_is_offered() {
        let p = provider();
        let s = PinnedServerVerifier::new(trusting(&[]), &p);
        let c = PinnedClientVerifier::new(trusting(&[]), &p);
        assert_eq!(s.supported_verify_schemes(), [SignatureScheme::ED25519]);
        assert_eq!(c.supported_verify_schemes(), [SignatureScheme::ED25519]);
    }

    #[test]
    fn the_rwlock_sees_peers_added_later() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(RwLock::new(
            TrustStore::open(dir.path().join("trust.toml")).unwrap(),
        ));
        let key = Identity::from_seed([4; 32]).public_key();
        let v = PinnedClientVerifier::new(store.clone(), &provider());
        assert!(
            v.verify_client_cert(&cert_of(4), &[], UnixTime::now())
                .is_err()
        );
        store.write().unwrap().add(crate::identity::TrustedPeer {
            public_key: key,
            name: "PC".to_owned(),
        });
        assert!(
            v.verify_client_cert(&cert_of(4), &[], UnixTime::now())
                .is_ok()
        );
    }

    // QA round 1: forgetting a pairing has to hold for new connections
    // of an endpoint that is already open, on both sides.
    #[test]
    fn the_rwlock_sees_peers_forgotten_later() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(RwLock::new(
            TrustStore::open(dir.path().join("trust.toml")).unwrap(),
        ));
        let key = Identity::from_seed([5; 32]).public_key();
        store.write().unwrap().add(crate::identity::TrustedPeer {
            public_key: key,
            name: "PC".to_owned(),
        });
        let p = provider();
        let client = PinnedClientVerifier::new(store.clone(), &p);
        let server = PinnedServerVerifier::new(store.clone(), &p);
        let name = ServerName::try_from("tandem").unwrap();
        let now = UnixTime::now();
        assert!(client.verify_client_cert(&cert_of(5), &[], now).is_ok());
        assert!(
            server
                .verify_server_cert(&cert_of(5), &[], &name, &[], now)
                .is_ok()
        );

        assert!(store.write().unwrap().remove(&key));
        assert!(client.verify_client_cert(&cert_of(5), &[], now).is_err());
        assert!(
            server
                .verify_server_cert(&cert_of(5), &[], &name, &[], now)
                .is_err()
        );
    }

    #[test]
    fn a_poisoned_rwlock_keeps_verifying() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(RwLock::new(
            TrustStore::open(dir.path().join("trust.toml")).unwrap(),
        ));
        let poisoner = store.clone();
        let _ = std::thread::spawn(move || {
            let mut g = poisoner.write().unwrap();
            g.add(crate::identity::TrustedPeer {
                public_key: Identity::from_seed([6; 32]).public_key(),
                name: "PC".to_owned(),
            });
            panic!("panic with the write lock held");
        })
        .join();
        assert!(store.is_poisoned());
        let v = PinnedClientVerifier::new(store, &provider());
        assert!(
            v.verify_client_cert(&cert_of(6), &[], UnixTime::now())
                .is_ok()
        );
        assert!(
            v.verify_client_cert(&cert_of(7), &[], UnixTime::now())
                .is_err()
        );
    }
}
