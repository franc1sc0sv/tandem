//! Identidad de la máquina y emparejamiento (DESIGN §9).
//!
//! Cada agente tiene una llave Ed25519 propia. Dos agentes se emparejan una
//! sola vez comparando un código de 6 dígitos que ambos derivan de sus llaves
//! públicas; al confirmarlo en los dos lados, cada uno guarda la llave del otro
//! en su [`TrustStore`]. El transporte (T-004) fija esa llave al verificar el
//! certificado autofirmado que genera [`Identity::tls_certificate`].
//!
//! Por ahora la llave vive en un archivo con permisos 0600; el llavero del
//! sistema (Keychain / DPAPI) llega en una tarea posterior.

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
pub use pairing::{Pairing, PairingCode, pairing_code};
pub use public_key::PublicKey;
pub use trust::{TrustStore, TrustedPeer};
