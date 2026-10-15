use std::fmt;

use sha2::{Digest, Sha256};

use super::{IdentityError, PublicKey, TrustStore, TrustedPeer};

/// Separa este uso de SHA-256 de cualquier otro hash sobre las mismas llaves.
const DOMAIN: &[u8] = b"tandem-pairing-v1";

/// Código de 6 dígitos que el usuario compara en las dos pantallas.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PairingCode(u32);

impl PairingCode {
    pub fn value(self) -> u32 {
        self.0
    }
}

impl fmt::Display for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:06}", self.0)
    }
}

impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PairingCode({self})")
    }
}

/// Deriva el código de comparación de las dos llaves públicas.
///
/// Las llaves se ordenan antes de hashear para que ambos lados obtengan el
/// mismo código sin acordar quién es "A". Un atacante en medio tendría que
/// sustituir una llave por otra con el mismo código: 1 en un millón por
/// intento, y cada intento exige que el usuario confirme.
pub fn pairing_code(a: &PublicKey, b: &PublicKey) -> PairingCode {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let digest = Sha256::new()
        .chain_update(DOMAIN)
        .chain_update(lo.as_bytes())
        .chain_update(hi.as_bytes())
        .finalize();
    let mut head = [0u8; 8];
    head.copy_from_slice(&digest[..8]);
    // Con 64 bits el sesgo del módulo es despreciable (< 2^-44).
    let code = u64::from_be_bytes(head) % 1_000_000;
    PairingCode(code as u32)
}

/// Un emparejamiento en curso con un par concreto.
///
/// Solo se confía en el par cuando el usuario confirmó el código aquí y el
/// par avisó que su usuario confirmó el mismo código allá.
#[derive(Debug)]
pub struct Pairing {
    remote: TrustedPeer,
    code: PairingCode,
    local_confirmed: bool,
    remote_confirmed: bool,
}

impl Pairing {
    pub fn new(
        local: &PublicKey,
        remote: PublicKey,
        remote_name: impl Into<String>,
    ) -> Result<Self, IdentityError> {
        if *local == remote {
            return Err(IdentityError::SelfPairing);
        }
        Ok(Self {
            code: pairing_code(local, &remote),
            remote: TrustedPeer {
                public_key: remote,
                name: remote_name.into(),
            },
            local_confirmed: false,
            remote_confirmed: false,
        })
    }

    /// Código que esta máquina debe mostrar.
    pub fn code(&self) -> PairingCode {
        self.code
    }

    pub fn remote(&self) -> &TrustedPeer {
        &self.remote
    }

    /// El usuario de esta máquina confirmó que los códigos coinciden.
    pub fn confirm_local(&mut self) {
        self.local_confirmed = true;
    }

    /// El par informa que su usuario confirmó `code`. Si no es el nuestro,
    /// alguien cambió una de las llaves en el camino.
    pub fn confirm_remote(&mut self, code: PairingCode) -> Result<(), IdentityError> {
        if code != self.code {
            return Err(IdentityError::CodeMismatch {
                local: self.code.to_string(),
                remote: code.to_string(),
            });
        }
        self.remote_confirmed = true;
        Ok(())
    }

    pub fn is_confirmed(&self) -> bool {
        self.local_confirmed && self.remote_confirmed
    }

    /// Guarda la llave del par en el trust store y lo persiste.
    pub fn complete(self, store: &mut TrustStore) -> Result<TrustedPeer, IdentityError> {
        if !self.is_confirmed() {
            return Err(IdentityError::NotConfirmed);
        }
        store.add_and_save(self.remote.clone())?;
        Ok(self.remote)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;
    use proptest::prelude::*;

    fn key(seed: u8) -> PublicKey {
        Identity::from_seed([seed; 32]).public_key()
    }

    #[test]
    fn el_codigo_tiene_seis_digitos() {
        let code = pairing_code(&key(1), &key(2));
        assert!(code.value() < 1_000_000);
        assert_eq!(code.to_string().len(), 6);
        assert_eq!(PairingCode(42).to_string(), "000042");
    }

    #[test]
    fn el_codigo_es_estable() {
        // Fija el algoritmo: cambiarlo rompe el emparejamiento entre versiones.
        assert_eq!(pairing_code(&key(1), &key(2)).to_string(), "508434");
    }

    proptest! {
        #[test]
        fn mismo_codigo_en_ambos_lados(a in any::<[u8; 32]>(), b in any::<[u8; 32]>()) {
            let a = Identity::from_seed(a).public_key();
            let b = Identity::from_seed(b).public_key();
            prop_assert_eq!(pairing_code(&a, &b), pairing_code(&b, &a));
        }

        #[test]
        fn cambia_si_cambia_cualquier_llave(
            a in any::<[u8; 32]>(),
            b in any::<[u8; 32]>(),
            c in any::<[u8; 32]>(),
        ) {
            let (a, b, c) = (
                Identity::from_seed(a).public_key(),
                Identity::from_seed(b).public_key(),
                Identity::from_seed(c).public_key(),
            );
            prop_assume!(c != a && c != b);
            // Una colisión real tiene probabilidad 1e-6 por caso; con las
            // semillas de proptest es determinista y no ocurre.
            prop_assert_ne!(pairing_code(&a, &b), pairing_code(&c, &b));
            prop_assert_ne!(pairing_code(&a, &b), pairing_code(&a, &c));
        }
    }

    #[test]
    fn no_se_empareja_consigo_mismo() {
        assert!(matches!(
            Pairing::new(&key(1), key(1), "yo"),
            Err(IdentityError::SelfPairing)
        ));
    }

    #[test]
    fn flujo_completo_entre_dos_maquinas() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, pc) = (key(10), key(20));
        let mut store_mac = TrustStore::open(dir.path().join("mac.toml")).unwrap();
        let mut store_pc = TrustStore::open(dir.path().join("pc.toml")).unwrap();

        let mut en_mac = Pairing::new(&mac, pc, "PC").unwrap();
        let mut en_pc = Pairing::new(&pc, mac, "Mac").unwrap();
        assert_eq!(en_mac.code(), en_pc.code());

        en_mac.confirm_local();
        en_pc.confirm_local();
        en_mac.confirm_remote(en_pc.code()).unwrap();
        en_pc.confirm_remote(en_mac.code()).unwrap();

        en_mac.complete(&mut store_mac).unwrap();
        en_pc.complete(&mut store_pc).unwrap();
        assert!(store_mac.is_trusted(&pc));
        assert!(store_pc.is_trusted(&mac));
        assert!(!store_mac.is_trusted(&key(30)));
    }

    #[test]
    fn sin_ambas_confirmaciones_no_se_guarda() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = TrustStore::open(dir.path().join("t.toml")).unwrap();

        let mut solo_local = Pairing::new(&key(1), key(2), "PC").unwrap();
        solo_local.confirm_local();
        assert!(matches!(
            solo_local.complete(&mut store),
            Err(IdentityError::NotConfirmed)
        ));

        let mut solo_remoto = Pairing::new(&key(1), key(2), "PC").unwrap();
        let code = solo_remoto.code();
        solo_remoto.confirm_remote(code).unwrap();
        assert!(matches!(
            solo_remoto.complete(&mut store),
            Err(IdentityError::NotConfirmed)
        ));

        assert!(!store.is_trusted(&key(2)));
        assert!(!dir.path().join("t.toml").exists());
    }

    #[test]
    fn codigo_distinto_del_par_se_rechaza() {
        // Un intermediario sustituye la llave de la PC: la Mac ve otro código.
        let (mac, pc, intruso) = (key(1), key(2), key(3));
        let mut en_mac = Pairing::new(&mac, intruso, "PC").unwrap();
        let en_pc = Pairing::new(&pc, mac, "Mac").unwrap();
        en_mac.confirm_local();
        assert!(matches!(
            en_mac.confirm_remote(en_pc.code()),
            Err(IdentityError::CodeMismatch { .. })
        ));
        assert!(!en_mac.is_confirmed());
    }
}
