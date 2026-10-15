use std::io;
use std::path::PathBuf;

use super::PublicKey;

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("error de E/S en {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("la llave en {path} está dañada: {reason}")]
    CorruptKey { path: PathBuf, reason: &'static str },
    #[error("permisos inseguros en {path} ({mode:o}); se esperan 0600")]
    InsecurePermissions { path: PathBuf, mode: u32 },
    #[error("no se pudo obtener entropía del sistema: {0}")]
    Entropy(getrandom::Error),
    #[error("no se pudo codificar la llave en PKCS#8: {0}")]
    Pkcs8(String),
    #[error("no se pudo generar el certificado: {0}")]
    Certificate(#[from] rcgen::Error),
    #[error("llave pública inválida: {0}")]
    InvalidPublicKey(&'static str),
    #[error("trust store inválido en {path}: {source}")]
    TrustStoreParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("versión de trust store no soportada en {path}: {version}")]
    TrustStoreVersion { path: PathBuf, version: u32 },
    #[error("no se pudo serializar el trust store: {0}")]
    TrustStoreSerialize(#[from] toml::ser::Error),
    #[error("par no confiable: {0}")]
    Untrusted(PublicKey),
    #[error("una máquina no puede emparejarse consigo misma")]
    SelfPairing,
    #[error("el par confirmó el código {remote}, pero aquí se muestra {local}")]
    CodeMismatch { local: String, remote: String },
    #[error("el emparejamiento no está confirmado en ambos lados")]
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
