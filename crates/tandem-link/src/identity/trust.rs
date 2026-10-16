use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{IdentityError, PublicKey, fsutil};

/// Par emparejado: su llave y el nombre que se muestra en la UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedPeer {
    pub public_key: PublicKey,
    pub name: String,
}

/// Llaves de los pares en los que esta máquina confía, persistidas en TOML.
///
/// Es la única fuente de confianza del transporte: un par que no está aquí
/// se rechaza.
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
    /// Abre el trust store de `path`; si no existe, empieza vacío (se crea al
    /// guardar).
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

    /// Devuelve el par si es de confianza; si no, [`IdentityError::Untrusted`].
    pub fn verify(&self, key: &PublicKey) -> Result<&TrustedPeer, IdentityError> {
        self.peers.get(key).ok_or(IdentityError::Untrusted(*key))
    }

    pub fn peers(&self) -> impl Iterator<Item = &TrustedPeer> {
        self.peers.values()
    }

    /// Agrega o reemplaza un par en memoria. Llamar a [`save`](Self::save)
    /// para persistir.
    pub fn add(&mut self, peer: TrustedPeer) {
        self.peers.insert(peer.public_key, peer);
    }

    /// Quita un par en memoria (olvidar emparejamiento). Devuelve si estaba.
    pub fn remove(&mut self, key: &PublicKey) -> bool {
        self.peers.remove(key).is_some()
    }

    /// Agrega y persiste; si no se puede guardar, la memoria queda como antes
    /// para que nunca se confíe en un par que no sobreviviría a un reinicio.
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

    /// Quita y persiste (desemparejar); si no se puede guardar, el par sigue como
    /// estaba, para que la memoria nunca diga algo distinto del disco. Devuelve si
    /// estaba.
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

    /// Escribe el trust store (atómico, 0600 en Unix).
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
    fn persiste_y_recarga() {
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
    fn quitar_persiste_y_si_no_se_puede_guardar_no_cambia_nada() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.toml");
        let mut store = TrustStore::open(&path).unwrap();
        store.add_and_save(peer(1, "PC")).unwrap();
        let key = peer(1, "").public_key;
        assert!(!store.remove_and_save(&peer(2, "").public_key).unwrap());
        assert!(store.remove_and_save(&key).unwrap());
        assert!(!TrustStore::open(&path).unwrap().is_trusted(&key));

        // Si donde iba su directorio aparece un archivo, no se puede guardar.
        let mut broken = TrustStore::open(dir.path().join("sub/trust.toml")).unwrap();
        std::fs::write(dir.path().join("sub"), b"x").unwrap();
        broken.add(peer(3, "Mac"));
        let key = peer(3, "").public_key;
        assert!(broken.remove_and_save(&key).is_err());
        assert!(broken.is_trusted(&key));
    }

    #[test]
    fn par_no_confiable_se_rechaza() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.toml");
        let mut store = TrustStore::open(&path).unwrap();
        store.add_and_save(peer(1, "PC")).unwrap();

        let reloaded = TrustStore::open(&path).unwrap();
        let stranger = peer(9, "intruso").public_key;
        assert!(!reloaded.is_trusted(&stranger));
        assert!(matches!(
            reloaded.verify(&stranger),
            Err(IdentityError::Untrusted(k)) if k == stranger
        ));
    }

    #[test]
    fn olvidar_un_par_persiste() {
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
    fn inexistente_empieza_vacio_sin_crear_archivo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.toml");
        let store = TrustStore::open(&path).unwrap();
        assert_eq!(store.peers().count(), 0);
        assert!(!path.exists());
    }

    #[test]
    fn archivo_invalido_es_error() {
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
    fn si_no_se_puede_guardar_no_queda_confiado() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        let mut store = TrustStore::open(sub.join("trust.toml")).unwrap();
        // "sub" pasa a ser un archivo regular: ya no se puede escribir dentro.
        fs::write(&sub, "").unwrap();
        let p = peer(1, "PC");
        assert!(store.add_and_save(p.clone()).is_err());
        assert!(!store.is_trusted(&p.public_key));
    }

    // QA ronda 1: un archivo editado a mano puede repetir un par; gana la última entrada
    // y al guardar queda una sola.
    #[test]
    fn par_repetido_en_el_archivo_queda_una_vez() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.toml");
        let key = peer(1, "").public_key;
        fs::write(
            &path,
            format!(
                "version = 1\n[[peer]]\npublic_key = \"{key}\"\nname = \"vieja\"\n\
                 [[peer]]\npublic_key = \"{key}\"\nname = \"nueva\"\n"
            ),
        )
        .unwrap();
        let store = TrustStore::open(&path).unwrap();
        assert_eq!(store.peers().count(), 1);
        assert_eq!(store.verify(&key).unwrap().name, "nueva");
        store.save().unwrap();
        assert_eq!(TrustStore::open(&path).unwrap().peers().count(), 1);
    }

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

        /// Cualquier nombre (comillas, saltos de línea, Unicode, controles) sobrevive
        /// a guardar y abrir: el nombre del par lo elige el otro lado.
        #[test]
        fn cualquier_nombre_sobrevive_a_guardar_y_abrir(
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
    fn se_guarda_con_permisos_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.toml");
        let mut store = TrustStore::open(&path).unwrap();
        store.add_and_save(peer(1, "PC")).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
