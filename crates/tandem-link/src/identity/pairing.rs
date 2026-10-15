//! Emparejamiento por comparación numérica con compromiso previo.
//!
//! Un código derivado solo de las dos llaves públicas no sirve: un intermediario
//! puede buscar por fuerza bruta una llave propia que dé el mismo código en las
//! dos pantallas (un millón de intentos tarda segundos). Por eso, como en
//! Bluetooth SSP o la SAS de ZRTP, cada lado aporta un nonce aleatorio y quien
//! inicia se compromete con el suyo antes de ver el del otro:
//!
//! 1. Quien inicia envía su llave y `commit = H(llave ‖ nonce_i)` ([`PairingOffer`]).
//! 2. Quien responde envía su llave y `nonce_r` ([`PairingAnswer`]).
//! 3. Quien inicia revela `nonce_i` ([`PairingReveal`]); el otro comprueba el
//!    compromiso.
//!
//! El código depende de las dos llaves y los dos nonces. Cuando un atacante
//! tiene que fijar su llave y su nonce, todavía no conoce el nonce que decide
//! el código del otro lado, así que acierta con probabilidad 1 en un millón por
//! intento, y cada intento fallido lo ven los usuarios.

use std::fmt;

use sha2::{Digest, Sha256};

use super::{IdentityError, PublicKey, TrustStore, TrustedPeer};

/// Separa el hash del código de cualquier otro hash sobre las mismas llaves.
const CODE_DOMAIN: &[u8] = b"tandem-pairing-v2";
/// Separa el hash del compromiso del hash del código.
const COMMIT_DOMAIN: &[u8] = b"tandem-pairing-commit-v1";

pub const NONCE_LEN: usize = 32;

/// Nonce de un solo uso que cada lado aporta a un emparejamiento.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairingNonce([u8; NONCE_LEN]);

impl PairingNonce {
    /// Nonce nuevo con la entropía del sistema. Debe ser distinto en cada
    /// intento: reutilizarlo devuelve al atacante la ventaja de la fuerza bruta.
    pub fn generate() -> Result<Self, IdentityError> {
        let mut bytes = [0u8; NONCE_LEN];
        getrandom::fill(&mut bytes).map_err(IdentityError::Entropy)?;
        Ok(Self(bytes))
    }

    pub fn from_bytes(bytes: [u8; NONCE_LEN]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; NONCE_LEN] {
        &self.0
    }
}

/// Compromiso de quien inicia con su llave y su nonce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairingCommitment([u8; 32]);

impl PairingCommitment {
    pub fn new(public_key: &PublicKey, nonce: &PairingNonce) -> Self {
        let digest = Sha256::new()
            .chain_update(COMMIT_DOMAIN)
            .chain_update(public_key.as_bytes())
            .chain_update(nonce.as_bytes())
            .finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&digest);
        Self(bytes)
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

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

/// Deriva el código de comparación de las dos llaves y los dos nonces.
///
/// Las llaves se ordenan para que ambos lados obtengan el mismo código sin
/// acordar quién es "A"; los nonces van en orden de rol (quien inicia primero).
pub fn pairing_code(
    a: &PublicKey,
    b: &PublicKey,
    nonce_initiator: &PairingNonce,
    nonce_responder: &PairingNonce,
) -> PairingCode {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let digest = Sha256::new()
        .chain_update(CODE_DOMAIN)
        .chain_update(lo.as_bytes())
        .chain_update(hi.as_bytes())
        .chain_update(nonce_initiator.as_bytes())
        .chain_update(nonce_responder.as_bytes())
        .finalize();
    let mut head = [0u8; 8];
    head.copy_from_slice(&digest[..8]);
    // Con 64 bits el sesgo del módulo es despreciable (< 2^-44).
    let code = u64::from_be_bytes(head) % 1_000_000;
    PairingCode(code as u32)
}

/// Primer mensaje, de quien inicia a quien responde.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingOffer {
    pub public_key: PublicKey,
    pub name: String,
    pub commitment: PairingCommitment,
}

/// Respuesta, de quien responde a quien inicia.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingAnswer {
    pub public_key: PublicKey,
    pub name: String,
    pub nonce: PairingNonce,
}

/// Último mensaje: quien inicia revela el nonce con el que se comprometió.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairingReveal {
    pub nonce: PairingNonce,
}

/// Lado que inicia, tras enviar su [`PairingOffer`].
#[derive(Debug)]
pub struct PairingInitiator {
    local: PublicKey,
    nonce: PairingNonce,
}

impl PairingInitiator {
    /// `nonce` debe venir de [`PairingNonce::generate`] en cada intento.
    pub fn start(
        local: PublicKey,
        local_name: impl Into<String>,
        nonce: PairingNonce,
    ) -> (Self, PairingOffer) {
        let offer = PairingOffer {
            public_key: local,
            name: local_name.into(),
            commitment: PairingCommitment::new(&local, &nonce),
        };
        (Self { local, nonce }, offer)
    }

    /// Recibe la respuesta, revela el nonce y pasa a la comparación de códigos.
    pub fn receive_answer(
        self,
        answer: PairingAnswer,
    ) -> Result<(Pairing, PairingReveal), IdentityError> {
        if answer.public_key == self.local {
            return Err(IdentityError::SelfPairing);
        }
        let code = pairing_code(&self.local, &answer.public_key, &self.nonce, &answer.nonce);
        let pairing = Pairing::new(
            TrustedPeer {
                public_key: answer.public_key,
                name: answer.name,
            },
            code,
        );
        Ok((pairing, PairingReveal { nonce: self.nonce }))
    }
}

/// Lado que responde, tras recibir la oferta y enviar su [`PairingAnswer`].
#[derive(Debug)]
pub struct PairingResponder {
    local: PublicKey,
    nonce: PairingNonce,
    remote: TrustedPeer,
    commitment: PairingCommitment,
}

impl PairingResponder {
    /// `nonce` debe venir de [`PairingNonce::generate`] en cada intento.
    pub fn receive_offer(
        local: PublicKey,
        local_name: impl Into<String>,
        offer: PairingOffer,
        nonce: PairingNonce,
    ) -> Result<(Self, PairingAnswer), IdentityError> {
        if offer.public_key == local {
            return Err(IdentityError::SelfPairing);
        }
        let answer = PairingAnswer {
            public_key: local,
            name: local_name.into(),
            nonce,
        };
        let responder = Self {
            local,
            nonce,
            remote: TrustedPeer {
                public_key: offer.public_key,
                name: offer.name,
            },
            commitment: offer.commitment,
        };
        Ok((responder, answer))
    }

    /// Comprueba que el nonce revelado es el comprometido para la llave recibida.
    pub fn receive_reveal(self, reveal: PairingReveal) -> Result<Pairing, IdentityError> {
        if PairingCommitment::new(&self.remote.public_key, &reveal.nonce) != self.commitment {
            return Err(IdentityError::CommitMismatch);
        }
        let code = pairing_code(
            &self.local,
            &self.remote.public_key,
            &reveal.nonce,
            &self.nonce,
        );
        Ok(Pairing::new(self.remote, code))
    }
}

/// Un emparejamiento en fase de comparación del código.
///
/// La seguridad la da que el usuario compare el código en las dos pantallas.
/// El aviso del par ([`Pairing::confirm_remote`]) solo coordina que ambos
/// lados terminen: viaja por el mismo canal que un intermediario controlaría.
#[derive(Debug)]
pub struct Pairing {
    remote: TrustedPeer,
    code: PairingCode,
    local_confirmed: bool,
    remote_confirmed: bool,
}

impl Pairing {
    fn new(remote: TrustedPeer, code: PairingCode) -> Self {
        Self {
            remote,
            code,
            local_confirmed: false,
            remote_confirmed: false,
        }
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

    /// El par informa que su usuario confirmó `code`. Un código distinto
    /// delata un fallo del protocolo o un intermediario torpe; no sustituye a
    /// la comparación del usuario.
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

    fn nonce(seed: u8) -> PairingNonce {
        PairingNonce::from_bytes([seed; NONCE_LEN])
    }

    /// Ejecuta los tres mensajes entre dos máquinas sin intermediario.
    fn emparejar(
        iniciador: PublicKey,
        ni: PairingNonce,
        respondedor: PublicKey,
        nr: PairingNonce,
    ) -> (Pairing, Pairing) {
        let (ini, offer) = PairingInitiator::start(iniciador, "Mac", ni);
        let (resp, answer) = PairingResponder::receive_offer(respondedor, "PC", offer, nr).unwrap();
        let (en_ini, reveal) = ini.receive_answer(answer).unwrap();
        let en_resp = resp.receive_reveal(reveal).unwrap();
        (en_ini, en_resp)
    }

    #[test]
    fn el_codigo_tiene_seis_digitos() {
        let code = pairing_code(&key(1), &key(2), &nonce(1), &nonce(2));
        assert!(code.value() < 1_000_000);
        assert_eq!(code.to_string().len(), 6);
        assert_eq!(PairingCode(42).to_string(), "000042");
    }

    #[test]
    fn el_codigo_es_estable() {
        // Fija el algoritmo: cambiarlo rompe el emparejamiento entre versiones.
        assert_eq!(
            pairing_code(&key(1), &key(2), &nonce(3), &nonce(4)).to_string(),
            "307316"
        );
    }

    #[test]
    fn los_nonces_generados_son_distintos() {
        assert_ne!(
            PairingNonce::generate().unwrap(),
            PairingNonce::generate().unwrap()
        );
    }

    proptest! {
        #[test]
        fn mismo_codigo_en_ambos_lados(
            a in any::<[u8; 32]>(),
            b in any::<[u8; 32]>(),
            ni in any::<[u8; 32]>(),
            nr in any::<[u8; 32]>(),
        ) {
            let a = Identity::from_seed(a).public_key();
            let b = Identity::from_seed(b).public_key();
            prop_assume!(a != b);
            let (ni, nr) = (PairingNonce::from_bytes(ni), PairingNonce::from_bytes(nr));
            prop_assert_eq!(pairing_code(&a, &b, &ni, &nr), pairing_code(&b, &a, &ni, &nr));
            let (en_a, en_b) = emparejar(a, ni, b, nr);
            prop_assert_eq!(en_a.code(), en_b.code());
        }

        #[test]
        fn con_el_compromiso_fijado_cambiar_la_llave_cambia_el_codigo(
            a in any::<[u8; 32]>(),
            b in any::<[u8; 32]>(),
            c in any::<[u8; 32]>(),
            ni in any::<[u8; 32]>(),
            nr in any::<[u8; 32]>(),
        ) {
            let (a, b, c) = (
                Identity::from_seed(a).public_key(),
                Identity::from_seed(b).public_key(),
                Identity::from_seed(c).public_key(),
            );
            prop_assume!(a != b && c != a && c != b);
            let (ni, nr) = (PairingNonce::from_bytes(ni), PairingNonce::from_bytes(nr));
            // Una colisión real tiene probabilidad 1e-6 por caso; con las
            // semillas de proptest es determinista y no ocurre.
            let base = pairing_code(&a, &b, &ni, &nr);
            prop_assert_ne!(base, pairing_code(&c, &b, &ni, &nr));
            prop_assert_ne!(base, pairing_code(&a, &c, &ni, &nr));
        }

        #[test]
        fn cambiar_cualquier_nonce_cambia_el_codigo(
            ni in any::<[u8; 32]>(),
            nr in any::<[u8; 32]>(),
            otro in any::<[u8; 32]>(),
        ) {
            prop_assume!(otro != ni && otro != nr);
            let (a, b) = (key(1), key(2));
            let (ni, nr, otro) = (
                PairingNonce::from_bytes(ni),
                PairingNonce::from_bytes(nr),
                PairingNonce::from_bytes(otro),
            );
            let base = pairing_code(&a, &b, &ni, &nr);
            prop_assert_ne!(base, pairing_code(&a, &b, &otro, &nr));
            prop_assert_ne!(base, pairing_code(&a, &b, &ni, &otro));
        }

        #[test]
        fn el_compromiso_ata_llave_y_nonce(
            a in any::<[u8; 32]>(),
            c in any::<[u8; 32]>(),
            n in any::<[u8; 32]>(),
            m in any::<[u8; 32]>(),
        ) {
            let (a, c) = (Identity::from_seed(a).public_key(), Identity::from_seed(c).public_key());
            let (n, m) = (PairingNonce::from_bytes(n), PairingNonce::from_bytes(m));
            let commit = PairingCommitment::new(&a, &n);
            if c != a {
                prop_assert_ne!(commit, PairingCommitment::new(&c, &n));
            }
            if m != n {
                prop_assert_ne!(commit, PairingCommitment::new(&a, &m));
            }
        }
    }

    #[test]
    fn no_se_empareja_consigo_mismo() {
        let (ini, offer) = PairingInitiator::start(key(1), "yo", nonce(1));
        assert!(matches!(
            PairingResponder::receive_offer(key(1), "yo", offer, nonce(2)),
            Err(IdentityError::SelfPairing)
        ));
        let answer = PairingAnswer {
            public_key: key(1),
            name: "yo".into(),
            nonce: nonce(2),
        };
        assert!(matches!(
            ini.receive_answer(answer),
            Err(IdentityError::SelfPairing)
        ));
    }

    #[test]
    fn flujo_completo_entre_dos_maquinas() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, pc) = (key(10), key(20));
        let mut store_mac = TrustStore::open(dir.path().join("mac.toml")).unwrap();
        let mut store_pc = TrustStore::open(dir.path().join("pc.toml")).unwrap();

        let (mut en_mac, mut en_pc) = emparejar(
            mac,
            PairingNonce::generate().unwrap(),
            pc,
            PairingNonce::generate().unwrap(),
        );
        assert_eq!(en_mac.code(), en_pc.code());
        assert_eq!(en_mac.remote().name, "PC");
        assert_eq!(en_pc.remote().name, "Mac");

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

        let (mut solo_local, _) = emparejar(key(1), nonce(1), key(2), nonce(2));
        solo_local.confirm_local();
        assert!(matches!(
            solo_local.complete(&mut store),
            Err(IdentityError::NotConfirmed)
        ));

        let (mut solo_remoto, _) = emparejar(key(1), nonce(1), key(2), nonce(2));
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
    fn llave_cambiada_tras_el_compromiso_se_rechaza() {
        // Un intermediario reemplaza la llave de la oferta pero no puede
        // rehacer el compromiso sin conocer el nonce de quien inicia.
        let (mac, pc, intruso) = (key(1), key(2), key(3));
        let (ini, mut offer) = PairingInitiator::start(mac, "Mac", nonce(1));
        offer.public_key = intruso;
        let (resp, answer) = PairingResponder::receive_offer(pc, "PC", offer, nonce(2)).unwrap();
        let (_, reveal) = ini.receive_answer(answer).unwrap();
        assert!(matches!(
            resp.receive_reveal(reveal),
            Err(IdentityError::CommitMismatch)
        ));
    }

    #[test]
    fn nonce_revelado_distinto_del_comprometido_se_rechaza() {
        let (ini, offer) = PairingInitiator::start(key(1), "Mac", nonce(1));
        let (resp, answer) =
            PairingResponder::receive_offer(key(2), "PC", offer, nonce(2)).unwrap();
        let (_, mut reveal) = ini.receive_answer(answer).unwrap();
        reveal.nonce = nonce(9);
        assert!(matches!(
            resp.receive_reveal(reveal),
            Err(IdentityError::CommitMismatch)
        ));
    }

    #[test]
    fn intermediario_que_reenvia_con_sus_llaves_muestra_otro_codigo() {
        // Mac <-> intruso <-> PC. El intruso se compromete con la PC antes de
        // conocer nonce_r, y responde a la Mac antes de conocer nonce_i: no
        // puede elegir sus valores para igualar los códigos.
        let (mac, pc, m1, m2) = (key(1), key(2), key(3), key(4));

        let (mac_ini, oferta_mac) = PairingInitiator::start(mac, "Mac", nonce(10));
        let (intruso_ini, oferta_intruso) = PairingInitiator::start(m1, "Mac", nonce(11));
        let (pc_resp, respuesta_pc) =
            PairingResponder::receive_offer(pc, "PC", oferta_intruso, nonce(12)).unwrap();
        let (intruso_resp, respuesta_intruso) =
            PairingResponder::receive_offer(m2, "PC", oferta_mac, nonce(13)).unwrap();

        let (en_mac, revelado_mac) = mac_ini.receive_answer(respuesta_intruso).unwrap();
        let (_, revelado_intruso) = intruso_ini.receive_answer(respuesta_pc).unwrap();
        let en_pc = pc_resp.receive_reveal(revelado_intruso).unwrap();
        intruso_resp.receive_reveal(revelado_mac).unwrap();

        assert_ne!(en_mac.code(), en_pc.code());
    }

    #[test]
    fn codigo_distinto_del_par_se_rechaza() {
        let (mut en_mac, _) = emparejar(key(1), nonce(1), key(2), nonce(2));
        en_mac.confirm_local();
        let otro = PairingCode((en_mac.code().value() + 1) % 1_000_000);
        assert!(matches!(
            en_mac.confirm_remote(otro),
            Err(IdentityError::CodeMismatch { .. })
        ));
        assert!(!en_mac.is_confirmed());
    }
}
