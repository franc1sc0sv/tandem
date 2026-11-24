//! Machine identity and pairing (DESIGN §9).
//!
//! Each agent has its own Ed25519 key. Two agents pair only once by comparing a
//! 6-digit code that both derive from their public keys and a nonce per side, with a prior
//! commitment from the initiator so
//! that a man in the middle cannot force the same code; once confirmed on both sides, each one stores the other's key
//! in its [`TrustStore`]. The transport (T-004) pins that key when verifying the
//! self-signed certificate that [`Identity::tls_certificate`] generates.
//!
//! For now the key lives in a file with 0600 permissions; the system keychain
//! (Keychain / DPAPI) arrives in a later task.

mod cert;
mod error;
mod fsutil;
mod key;
mod pairing;
mod public_key;
mod trust;

pub use cert::{TLS_SERVER_NAME, TlsIdentity};
pub use error::IdentityError;
pub use key::Identity;
pub use pairing::{
    NONCE_LEN, Pairing, PairingAnswer, PairingCode, PairingCommitment, PairingInitiator,
    PairingNonce, PairingOffer, PairingResponder, PairingReveal, pairing_code,
};
pub use public_key::PublicKey;
pub use trust::{TrustStore, TrustedPeer};
