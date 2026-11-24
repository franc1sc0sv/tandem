use std::io;
use std::path::PathBuf;

use super::PublicKey;

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the key at {path} is damaged: {reason}")]
    CorruptKey { path: PathBuf, reason: &'static str },
    #[error("insecure permissions on {path} ({mode:o}); 0600 expected")]
    InsecurePermissions { path: PathBuf, mode: u32 },
    #[error("could not obtain entropy from the system: {0}")]
    Entropy(getrandom::Error),
    #[error("could not encode the key as PKCS#8: {0}")]
    Pkcs8(String),
    #[error("could not generate the certificate: {0}")]
    Certificate(#[from] rcgen::Error),
    #[error("invalid public key: {0}")]
    InvalidPublicKey(&'static str),
    #[error("invalid trust store at {path}: {source}")]
    TrustStoreParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("unsupported trust store version at {path}: {version}")]
    TrustStoreVersion { path: PathBuf, version: u32 },
    #[error("could not serialize the trust store: {0}")]
    TrustStoreSerialize(#[from] toml::ser::Error),
    #[error("untrusted peer: {0}")]
    Untrusted(PublicKey),
    #[error("a machine cannot pair with itself")]
    SelfPairing,
    #[error("the peer confirmed code {remote}, but {local} is shown here")]
    CodeMismatch { local: String, remote: String },
    #[error("the revealed nonce does not match the peer's commitment")]
    CommitMismatch,
    #[error("the pairing is not confirmed on both sides")]
    NotConfirmed,
}

impl IdentityError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
