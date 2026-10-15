use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use ed25519_dalek::SigningKey;
use zeroize::Zeroizing;

use super::{IdentityError, PublicKey, fsutil};

/// Identidad Ed25519 de esta máquina (llave privada incluida).
///
/// En disco es la semilla de 32 bytes, sin más formato: la migración al
/// llavero del sistema reemplazará el archivo, así que no vale la pena
/// versionarlo.
pub struct Identity {
    signing: SigningKey,
}

const SEED_LEN: usize = 32;

impl Identity {
    /// Crea una identidad nueva con entropía del sistema operativo.
    pub fn generate() -> Result<Self, IdentityError> {
        let mut seed = Zeroizing::new([0u8; SEED_LEN]);
        getrandom::fill(seed.as_mut()).map_err(IdentityError::Entropy)?;
        Ok(Self::from_seed(*seed))
    }

    /// Identidad determinista a partir de una semilla (pruebas y migraciones).
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

    /// Lee la identidad de `path`. En Unix se rechaza si otros usuarios
    /// pueden leer el archivo.
    pub fn load(path: &Path) -> Result<Self, IdentityError> {
        let mut file = File::open(path).map_err(|e| IdentityError::io(path, e))?;
        fsutil::check_private(path, &file)?;
        let mut buf = Zeroizing::new(Vec::with_capacity(SEED_LEN + 1));
        // Se lee un byte de más para detectar archivos más largos de lo debido.
        file.by_ref()
            .take(SEED_LEN as u64 + 1)
            .read_to_end(&mut buf)
            .map_err(|e| IdentityError::io(path, e))?;
        let seed: [u8; SEED_LEN] =
            buf.as_slice()
                .try_into()
                .map_err(|_| IdentityError::CorruptKey {
                    path: path.to_path_buf(),
                    reason: "longitud distinta de 32 bytes",
                })?;
        let seed = Zeroizing::new(seed);
        Ok(Self::from_seed(*seed))
    }

    /// Guarda la identidad en `path` (0600 en Unix, escritura atómica).
    pub fn save(&self, path: &Path) -> Result<(), IdentityError> {
        let seed = Zeroizing::new(self.signing.to_bytes());
        fsutil::write_private(path, seed.as_ref())
    }

    /// Carga la identidad o, si el archivo no existe, crea y guarda una nueva.
    /// Un archivo dañado o con permisos inseguros es un error: nunca se
    /// reemplaza en silencio una identidad que los pares ya conocen.
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
        // Nunca se imprime la parte privada.
        f.debug_struct("Identity")
            .field("public_key", &self.public_key())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generar_da_llaves_distintas() {
        let a = Identity::generate().unwrap();
        let b = Identity::generate().unwrap();
        assert_ne!(a.public_key(), b.public_key());
    }

    #[test]
    fn guardar_y_cargar_conserva_la_llave() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/identity.key");
        let id = Identity::generate().unwrap();
        id.save(&path).unwrap();
        let loaded = Identity::load(&path).unwrap();
        assert_eq!(loaded.public_key(), id.public_key());
    }

    #[test]
    fn load_or_create_crea_una_vez_y_luego_reutiliza() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.key");
        let first = Identity::load_or_create(&path).unwrap();
        let second = Identity::load_or_create(&path).unwrap();
        assert_eq!(first.public_key(), second.public_key());
    }

    #[test]
    fn archivo_danado_es_error_y_no_se_reemplaza() {
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
    fn debug_no_expone_la_llave_privada() {
        let id = Identity::from_seed([0xab; 32]);
        let text = format!("{id:?}");
        assert!(!text.contains(&"ab".repeat(32)));
        assert!(text.contains(&id.public_key().to_hex()));
    }

    #[cfg(unix)]
    #[test]
    fn se_guarda_con_permisos_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.key");
        Identity::generate().unwrap().save(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn rechaza_llave_legible_por_otros() {
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
