use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{IdentityError, PublicKey, fsutil};

/// Paired peer: its key and the name shown in the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedPeer {
    pub public_key: PublicKey,
    pub name: String,
}

/// Keys of the peers this machine trusts, persisted in TOML.
///
/// It is the transport's only source of trust: a peer that is not here
/// is rejected.
#[derive(Debug)]
pub struct TrustStore {
    path: PathBuf,
    peers: BTreeMap<PublicKey, TrustedPeer>,
}

const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct TrustFile {
    version: u32,
    #[serde(default, rename = "peer")]
    peers: Vec<TrustedPeer>,
}

impl TrustStore {
    /// Opens the trust store at `path`; if it does not exist, it starts empty (it is created
    /// on save).
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, IdentityError> {
        let path = path.into();
        let peers = match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&path, &text)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(IdentityError::io(path, e)),
        };
        Ok(Self { path, peers })
    }

    fn parse(path: &Path, text: &str) -> Result<BTreeMap<PublicKey, TrustedPeer>, IdentityError> {
        let file: TrustFile =
            toml::from_str(text).map_err(|source| IdentityError::TrustStoreParse {
                path: path.to_path_buf(),
                source,
            })?;
        if file.version != FORMAT_VERSION {
            return Err(IdentityError::TrustStoreVersion {
                path: path.to_path_buf(),
                version: file.version,
            });
        }
        Ok(file.peers.into_iter().map(|p| (p.public_key, p)).collect())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_trusted(&self, key: &PublicKey) -> bool {
        self.peers.contains_key(key)
    }

    /// Returns the peer if trusted; otherwise, [`IdentityError::Untrusted`].
    pub fn verify(&self, key: &PublicKey) -> Result<&TrustedPeer, IdentityError> {
        self.peers.get(key).ok_or(IdentityError::Untrusted(*key))
    }

    pub fn peers(&self) -> impl Iterator<Item = &TrustedPeer> {
        self.peers.values()
    }

    /// Adds or replaces a peer in memory. Call [`save`](Self::save)
    /// to persist.
    pub fn add(&mut self, peer: TrustedPeer) {
        self.peers.insert(peer.public_key, peer);
    }

    /// Removes a peer in memory (forget the pairing). Returns whether it was there.
    pub fn remove(&mut self, key: &PublicKey) -> bool {
        self.peers.remove(key).is_some()
    }

    /// Adds and persists; if it cannot be saved, memory is left as it was
    /// so a peer that would not survive a restart is never trusted.
    pub fn add_and_save(&mut self, peer: TrustedPeer) -> Result<(), IdentityError> {
        let key = peer.public_key;
        let previous = self.peers.insert(key, peer);
        if let Err(e) = self.save() {
            match previous {
                Some(p) => self.peers.insert(key, p),
                None => self.peers.remove(&key),
            };
            return Err(e);
        }
        Ok(())
    }

    /// Removes and persists (unpair); if it cannot be saved, the peer stays as
    /// it was, so memory never says anything different from disk. Returns whether
    /// it was there.
    pub fn remove_and_save(&mut self, key: &PublicKey) -> Result<bool, IdentityError> {
        let Some(previous) = self.peers.remove(key) else {
            return Ok(false);
        };
        if let Err(e) = self.save() {
            self.peers.insert(*key, previous);
            return Err(e);
        }
        Ok(true)
    }

    /// Writes the trust store (atomic, 0600 on Unix).
    pub fn save(&self) -> Result<(), IdentityError> {
        let file = TrustFile {
            version: FORMAT_VERSION,
            peers: self.peers.values().cloned().collect(),
        };
        let text = toml::to_string(&file)?;
        fsutil::write_private(&self.path, text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    fn peer(seed: u8, name: &str) -> TrustedPeer {
        TrustedPeer {
            public_key: Identity::from_seed([seed; 32]).public_key(),
            name: name.to_owned(),
        }
    }

    #[test]
    fn persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.toml");
        let mut store = TrustStore::open(&path).unwrap();
        store.add(peer(1, "PC"));
        store.add(peer(2, "Mac"));
        store.save().unwrap();

        let reloaded = TrustStore::open(&path).unwrap();
        let names: Vec<_> = reloaded.peers().map(|p| p.name.clone()).collect();
        assert_eq!(names.len(), 2);
        assert_eq!(reloaded.verify(&peer(1, "").public_key).unwrap().name, "PC");
        assert_eq!(
            reloaded.verify(&peer(2, "").public_key).unwrap().name,
            "Mac"
        );
    }

    #[test]
    fn removing_persists_and_if_it_cannot_be_saved_nothing_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.toml");
        let mut store = TrustStore::open(&path).unwrap();
        store.add_and_save(peer(1, "PC")).unwrap();
        let key = peer(1, "").public_key;
        assert!(!store.remove_and_save(&peer(2, "").public_key).unwrap());
        assert!(store.remove_and_save(&key).unwrap());
        assert!(!TrustStore::open(&path).unwrap().is_trusted(&key));

        // If a file appears where its directory should go, it cannot be saved.
        let mut broken = TrustStore::open(dir.path().join("sub/trust.toml")).unwrap();
        std::fs::write(dir.path().join("sub"), b"x").unwrap();
        broken.add(peer(3, "Mac"));
        let key = peer(3, "").public_key;
        assert!(broken.remove_and_save(&key).is_err());
        assert!(broken.is_trusted(&key));
    }

    #[test]
    fn an_untrusted_peer_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.toml");
        let mut store = TrustStore::open(&path).unwrap();
        store.add_and_save(peer(1, "PC")).unwrap();

        let reloaded = TrustStore::open(&path).unwrap();
        let stranger = peer(9, "intruder").public_key;
        assert!(!reloaded.is_trusted(&stranger));
        assert!(matches!(
            reloaded.verify(&stranger),
            Err(IdentityError::Untrusted(k)) if k == stranger
        ));
    }

    #[test]
    fn forgetting_a_peer_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.toml");
        let mut store = TrustStore::open(&path).unwrap();
        store.add_and_save(peer(1, "PC")).unwrap();
        assert!(store.remove(&peer(1, "").public_key));
        assert!(!store.remove(&peer(1, "").public_key));
        store.save().unwrap();
        assert_eq!(TrustStore::open(&path).unwrap().peers().count(), 0);
    }

    #[test]
    fn a_nonexistent_one_starts_empty_without_creating_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.toml");
        let store = TrustStore::open(&path).unwrap();
        assert_eq!(store.peers().count(), 0);
        assert!(!path.exists());
    }

    #[test]
    fn an_invalid_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.toml");
        fs::write(
            &path,
            "version = 1\n[[peer]]\npublic_key = \"abc\"\nname = \"x\"\n",
        )
        .unwrap();
        assert!(matches!(
            TrustStore::open(&path),
            Err(IdentityError::TrustStoreParse { .. })
        ));
        fs::write(&path, "version = 99\n").unwrap();
        assert!(matches!(
            TrustStore::open(&path),
            Err(IdentityError::TrustStoreVersion { version: 99, .. })
        ));
    }

    #[test]
    fn if_it_cannot_be_saved_it_is_not_left_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        let mut store = TrustStore::open(sub.join("trust.toml")).unwrap();
        // "sub" becomes a regular file: it can no longer be written into.
        fs::write(&sub, "").unwrap();
        let p = peer(1, "PC");
        assert!(store.add_and_save(p.clone()).is_err());
        assert!(!store.is_trusted(&p.public_key));
    }

    // QA round 1: a hand-edited file may repeat a peer; the last entry wins
    // and on saving only one remains.
    #[test]
    fn a_peer_repeated_in_the_file_remains_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.toml");
        let key = peer(1, "").public_key;
        fs::write(
            &path,
            format!(
                "version = 1\n[[peer]]\npublic_key = \"{key}\"\nname = \"old\"\n\
                 [[peer]]\npublic_key = \"{key}\"\nname = \"new\"\n"
            ),
        )
        .unwrap();
        let store = TrustStore::open(&path).unwrap();
        assert_eq!(store.peers().count(), 1);
        assert_eq!(store.verify(&key).unwrap().name, "new");
        store.save().unwrap();
        assert_eq!(TrustStore::open(&path).unwrap().peers().count(), 1);
    }

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

        /// Any name (quotes, line breaks, Unicode, controls) survives
        /// saving and opening: the peer's name is chosen by the other side.
        #[test]
        fn any_name_survives_saving_and_opening(
            peers in prop::collection::btree_map(any::<u8>(), any::<String>(), 0..6),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("trust.toml");
            let mut store = TrustStore::open(&path).unwrap();
            for (seed, name) in &peers {
                store.add(peer(*seed, name));
            }
            store.save().unwrap();
            let reloaded = TrustStore::open(&path).unwrap();
            prop_assert_eq!(reloaded.peers().count(), peers.len());
            for (seed, name) in &peers {
                let key = peer(*seed, "").public_key;
                prop_assert_eq!(&reloaded.verify(&key).unwrap().name, name);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn is_saved_with_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.toml");
        let mut store = TrustStore::open(&path).unwrap();
        store.add_and_save(peer(1, "PC")).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
