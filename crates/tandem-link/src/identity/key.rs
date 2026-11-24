use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use ed25519_dalek::SigningKey;
use zeroize::Zeroizing;

use super::{IdentityError, PublicKey, fsutil};

/// Ed25519 identity of this machine (private key included).
///
/// On disk it is the 32-byte seed, with no further format: the migration to the
/// system keychain will replace the file, so it is not worth
/// versioning it.
#[derive(Clone)]
pub struct Identity {
    signing: SigningKey,
}

const SEED_LEN: usize = 32;

impl Identity {
    /// Creates a new identity with the operating system's entropy.
    pub fn generate() -> Result<Self, IdentityError> {
        let mut seed = Zeroizing::new([0u8; SEED_LEN]);
        getrandom::fill(seed.as_mut()).map_err(IdentityError::Entropy)?;
        Ok(Self::from_seed(*seed))
    }

    /// Deterministic identity from a seed (tests and migrations).
    pub fn from_seed(seed: [u8; SEED_LEN]) -> Self {
        Self {
            signing: SigningKey::from_bytes(&seed),
        }
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey::from_verifying(&self.signing.verifying_key())
    }

    pub(crate) fn signing_key(&self) -> &SigningKey {
        &self.signing
    }

    /// Reads the identity from `path`. On Unix it is rejected if other users
    /// can read the file.
    pub fn load(path: &Path) -> Result<Self, IdentityError> {
        let mut file = File::open(path).map_err(|e| IdentityError::io(path, e))?;
        fsutil::check_private(path, &file)?;
        let mut buf = Zeroizing::new(Vec::with_capacity(SEED_LEN + 1));
        // One extra byte is read to detect files longer than they should be.
        file.by_ref()
            .take(SEED_LEN as u64 + 1)
            .read_to_end(&mut buf)
            .map_err(|e| IdentityError::io(path, e))?;
        let seed: [u8; SEED_LEN] =
            buf.as_slice()
                .try_into()
                .map_err(|_| IdentityError::CorruptKey {
                    path: path.to_path_buf(),
                    reason: "length other than 32 bytes",
                })?;
        let seed = Zeroizing::new(seed);
        Ok(Self::from_seed(*seed))
    }

    /// Saves the identity to `path` (0600 on Unix, atomic write).
    pub fn save(&self, path: &Path) -> Result<(), IdentityError> {
        let seed = Zeroizing::new(self.signing.to_bytes());
        fsutil::write_private(path, seed.as_ref())
    }

    /// Loads the identity or, if the file does not exist, creates and saves a new one.
    /// A damaged file or one with insecure permissions is an error: an identity
    /// that peers already know is never silently replaced.
    pub fn load_or_create(path: &Path) -> Result<Self, IdentityError> {
        match Self::load(path) {
            Err(IdentityError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                let identity = Self::generate()?;
                identity.save(path)?;
                Ok(identity)
            }
            other => other,
        }
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The private part is never printed.
        f.debug_struct("Identity")
            .field("public_key", &self.public_key())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_gives_different_keys() {
        let a = Identity::generate().unwrap();
        let b = Identity::generate().unwrap();
        assert_ne!(a.public_key(), b.public_key());
    }

    #[test]
    fn saving_and_loading_keeps_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/identity.key");
        let id = Identity::generate().unwrap();
        id.save(&path).unwrap();
        let loaded = Identity::load(&path).unwrap();
        assert_eq!(loaded.public_key(), id.public_key());
    }

    #[test]
    fn load_or_create_creates_once_and_then_reuses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.key");
        let first = Identity::load_or_create(&path).unwrap();
        let second = Identity::load_or_create(&path).unwrap();
        assert_eq!(first.public_key(), second.public_key());
    }

    #[test]
    fn a_damaged_file_is_an_error_and_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.key");
        fsutil::write_private(&path, &[1, 2, 3]).unwrap();
        assert!(matches!(
            Identity::load_or_create(&path),
            Err(IdentityError::CorruptKey { .. })
        ));
        fsutil::write_private(&path, &[0; 33]).unwrap();
        assert!(matches!(
            Identity::load(&path),
            Err(IdentityError::CorruptKey { .. })
        ));
        assert_eq!(std::fs::read(&path).unwrap(), vec![0; 33]);
    }

    #[test]
    fn debug_does_not_expose_the_private_key() {
        let id = Identity::from_seed([0xab; 32]);
        let text = format!("{id:?}");
        assert!(!text.contains(&"ab".repeat(32)));
        assert!(text.contains(&id.public_key().to_hex()));
    }

    #[cfg(unix)]
    #[test]
    fn is_saved_with_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.key");
        Identity::generate().unwrap().save(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_key_readable_by_others() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.key");
        Identity::generate().unwrap().save(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            Identity::load_or_create(&path),
            Err(IdentityError::InsecurePermissions { mode: 0o644, .. })
        ));
    }
}
