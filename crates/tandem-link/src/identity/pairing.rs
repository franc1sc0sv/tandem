//! Pairing by numeric comparison with a prior commitment.
//!
//! A code derived only from the two public keys is no good: a man in the middle
//! can brute-force a key of their own that gives the same code on the
//! two screens (a million attempts take seconds). That is why, as in
//! Bluetooth SSP or ZRTP's SAS, each side contributes a random nonce and whoever
//! initiates commits to theirs before seeing the other's:
//!
//! 1. The initiator sends its key and `commit = H(key ‖ nonce_i)` ([`PairingOffer`]).
//! 2. The responder sends its key and `nonce_r` ([`PairingAnswer`]).
//! 3. The initiator reveals `nonce_i` ([`PairingReveal`]); the other checks the
//!    commitment.
//!
//! The code depends on the two keys and the two nonces. When an attacker
//! has to fix their key and their nonce, they do not yet know the nonce that decides
//! the code on the other side, so they succeed with probability 1 in a million per
//! attempt, and users see every failed attempt.

use std::fmt;

use sha2::{Digest, Sha256};

use super::{IdentityError, PublicKey, TrustStore, TrustedPeer};

/// Separates the code's hash from any other hash over the same keys.
const CODE_DOMAIN: &[u8] = b"tandem-pairing-v2";
/// Separates the commitment's hash from the code's hash.
const COMMIT_DOMAIN: &[u8] = b"tandem-pairing-commit-v1";

pub const NONCE_LEN: usize = 32;

/// Single-use nonce that each side contributes to a pairing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairingNonce([u8; NONCE_LEN]);

impl PairingNonce {
    /// New nonce with the system's entropy. It must be different on every
    /// attempt: reusing it hands the attacker back the brute-force advantage.
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

/// Commitment of the initiator with its key and its nonce.
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

/// 6-digit code the user compares on the two screens.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PairingCode(u32);

impl PairingCode {
    pub fn value(self) -> u32 {
        self.0
    }

    /// The code the peer sends on confirming; `None` if it does not have 6 digits.
    pub fn from_value(value: u32) -> Option<Self> {
        (value < 1_000_000).then_some(Self(value))
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

/// Derives the comparison code from the two keys and the two nonces.
///
/// The keys are sorted so both sides get the same code without
/// agreeing on who is "A"; the nonces go in role order (initiator first).
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
    // With 64 bits the modulo bias is negligible (< 2^-44).
    let code = u64::from_be_bytes(head) % 1_000_000;
    PairingCode(code as u32)
}

/// First message, from the initiator to the responder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingOffer {
    pub public_key: PublicKey,
    pub name: String,
    pub commitment: PairingCommitment,
}

/// Answer, from the responder to the initiator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingAnswer {
    pub public_key: PublicKey,
    pub name: String,
    pub nonce: PairingNonce,
}

/// Last message: the initiator reveals the nonce it committed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairingReveal {
    pub nonce: PairingNonce,
}

/// Initiating side, after sending its [`PairingOffer`].
#[derive(Debug)]
pub struct PairingInitiator {
    local: PublicKey,
    nonce: PairingNonce,
}

impl PairingInitiator {
    /// `nonce` must come from [`PairingNonce::generate`] on every attempt.
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

    /// Receives the answer, reveals the nonce and moves on to the code comparison.
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

/// Responding side, after receiving the offer and sending its [`PairingAnswer`].
#[derive(Debug)]
pub struct PairingResponder {
    local: PublicKey,
    nonce: PairingNonce,
    remote: TrustedPeer,
    commitment: PairingCommitment,
}

impl PairingResponder {
    /// `nonce` must come from [`PairingNonce::generate`] on every attempt.
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

    /// Checks that the revealed nonce is the one committed to for the received key.
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

/// A pairing in the code comparison phase.
///
/// Security comes from the user comparing the code on the two screens.
/// The peer's notice ([`Pairing::confirm_remote`]) only coordinates that both
/// sides finish: it travels over the same channel that a man in the middle would control.
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

    /// Code this machine must show.
    pub fn code(&self) -> PairingCode {
        self.code
    }

    pub fn remote(&self) -> &TrustedPeer {
        &self.remote
    }

    /// The user of this machine confirmed that the codes match.
    pub fn confirm_local(&mut self) {
        self.local_confirmed = true;
    }

    /// The peer reports that its user confirmed `code`. A different code
    /// betrays a protocol failure or a clumsy man in the middle; it does not replace
    /// the user's comparison.
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

    /// Stores the peer's key in the trust store and persists it.
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

    /// Runs the three messages between two machines without a man in the middle.
    fn pair_up(
        initiator: PublicKey,
        ni: PairingNonce,
        responder: PublicKey,
        nr: PairingNonce,
    ) -> (Pairing, Pairing) {
        let (init, offer) = PairingInitiator::start(initiator, "Mac", ni);
        let (resp, answer) = PairingResponder::receive_offer(responder, "PC", offer, nr).unwrap();
        let (at_init, reveal) = init.receive_answer(answer).unwrap();
        let at_resp = resp.receive_reveal(reveal).unwrap();
        (at_init, at_resp)
    }

    #[test]
    fn the_code_has_six_digits() {
        let code = pairing_code(&key(1), &key(2), &nonce(1), &nonce(2));
        assert!(code.value() < 1_000_000);
        assert_eq!(code.to_string().len(), 6);
        assert_eq!(PairingCode(42).to_string(), "000042");
    }

    #[test]
    fn the_received_code_has_six_digits() {
        assert_eq!(PairingCode::from_value(0), Some(PairingCode(0)));
        assert_eq!(PairingCode::from_value(999_999), Some(PairingCode(999_999)));
        assert_eq!(PairingCode::from_value(1_000_000), None);
        assert_eq!(PairingCode::from_value(u32::MAX), None);
    }

    #[test]
    fn the_code_is_stable() {
        // Fixes the algorithm: changing it breaks pairing between versions.
        assert_eq!(
            pairing_code(&key(1), &key(2), &nonce(3), &nonce(4)).to_string(),
            "307316"
        );
    }

    #[test]
    fn the_generated_nonces_are_different() {
        assert_ne!(
            PairingNonce::generate().unwrap(),
            PairingNonce::generate().unwrap()
        );
    }

    proptest! {
        #[test]
        fn same_code_on_both_sides(
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
            let (at_a, at_b) = pair_up(a, ni, b, nr);
            prop_assert_eq!(at_a.code(), at_b.code());
        }

        #[test]
        fn with_the_commitment_fixed_changing_the_key_changes_the_code(
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
            // A real collision has probability 1e-6 per case; with proptest's
            // seeds it is deterministic and does not happen.
            let base = pairing_code(&a, &b, &ni, &nr);
            prop_assert_ne!(base, pairing_code(&c, &b, &ni, &nr));
            prop_assert_ne!(base, pairing_code(&a, &c, &ni, &nr));
        }

        #[test]
        fn changing_any_nonce_changes_the_code(
            ni in any::<[u8; 32]>(),
            nr in any::<[u8; 32]>(),
            other in any::<[u8; 32]>(),
        ) {
            prop_assume!(other != ni && other != nr);
            let (a, b) = (key(1), key(2));
            let (ni, nr, other) = (
                PairingNonce::from_bytes(ni),
                PairingNonce::from_bytes(nr),
                PairingNonce::from_bytes(other),
            );
            let base = pairing_code(&a, &b, &ni, &nr);
            prop_assert_ne!(base, pairing_code(&a, &b, &other, &nr));
            prop_assert_ne!(base, pairing_code(&a, &b, &ni, &other));
        }

        #[test]
        fn the_commitment_binds_key_and_nonce(
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
    fn does_not_pair_with_itself() {
        let (init, offer) = PairingInitiator::start(key(1), "me", nonce(1));
        assert!(matches!(
            PairingResponder::receive_offer(key(1), "me", offer, nonce(2)),
            Err(IdentityError::SelfPairing)
        ));
        let answer = PairingAnswer {
            public_key: key(1),
            name: "me".into(),
            nonce: nonce(2),
        };
        assert!(matches!(
            init.receive_answer(answer),
            Err(IdentityError::SelfPairing)
        ));
    }

    #[test]
    fn full_flow_between_two_machines() {
        let dir = tempfile::tempdir().unwrap();
        let (mac, pc) = (key(10), key(20));
        let mut store_mac = TrustStore::open(dir.path().join("mac.toml")).unwrap();
        let mut store_pc = TrustStore::open(dir.path().join("pc.toml")).unwrap();

        let (mut at_mac, mut at_pc) = pair_up(
            mac,
            PairingNonce::generate().unwrap(),
            pc,
            PairingNonce::generate().unwrap(),
        );
        assert_eq!(at_mac.code(), at_pc.code());
        assert_eq!(at_mac.remote().name, "PC");
        assert_eq!(at_pc.remote().name, "Mac");

        at_mac.confirm_local();
        at_pc.confirm_local();
        at_mac.confirm_remote(at_pc.code()).unwrap();
        at_pc.confirm_remote(at_mac.code()).unwrap();

        at_mac.complete(&mut store_mac).unwrap();
        at_pc.complete(&mut store_pc).unwrap();
        assert!(store_mac.is_trusted(&pc));
        assert!(store_pc.is_trusted(&mac));
        assert!(!store_mac.is_trusted(&key(30)));
    }

    #[test]
    fn without_both_confirmations_nothing_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = TrustStore::open(dir.path().join("t.toml")).unwrap();

        let (mut solo_local, _) = pair_up(key(1), nonce(1), key(2), nonce(2));
        solo_local.confirm_local();
        assert!(matches!(
            solo_local.complete(&mut store),
            Err(IdentityError::NotConfirmed)
        ));

        let (mut solo_remoto, _) = pair_up(key(1), nonce(1), key(2), nonce(2));
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
    fn a_key_changed_after_the_commitment_is_rejected() {
        // A man in the middle replaces the offer's key but cannot
        // redo the commitment without knowing the initiator's nonce.
        let (mac, pc, intruder) = (key(1), key(2), key(3));
        let (init, mut offer) = PairingInitiator::start(mac, "Mac", nonce(1));
        offer.public_key = intruder;
        let (resp, answer) = PairingResponder::receive_offer(pc, "PC", offer, nonce(2)).unwrap();
        let (_, reveal) = init.receive_answer(answer).unwrap();
        assert!(matches!(
            resp.receive_reveal(reveal),
            Err(IdentityError::CommitMismatch)
        ));
    }

    #[test]
    fn a_revealed_nonce_different_from_the_committed_one_is_rejected() {
        let (init, offer) = PairingInitiator::start(key(1), "Mac", nonce(1));
        let (resp, answer) =
            PairingResponder::receive_offer(key(2), "PC", offer, nonce(2)).unwrap();
        let (_, mut reveal) = init.receive_answer(answer).unwrap();
        reveal.nonce = nonce(9);
        assert!(matches!(
            resp.receive_reveal(reveal),
            Err(IdentityError::CommitMismatch)
        ));
    }

    #[test]
    fn a_man_in_the_middle_relaying_with_his_keys_shows_another_code() {
        // Mac <-> intruder <-> PC. The intruder commits to the PC before
        // knowing nonce_r, and answers the Mac before knowing nonce_i: it
        // cannot choose its values to match the codes.
        let (mac, pc, m1, m2) = (key(1), key(2), key(3), key(4));

        let (mac_ini, oferta_mac) = PairingInitiator::start(mac, "Mac", nonce(10));
        let (intruso_ini, oferta_intruso) = PairingInitiator::start(m1, "Mac", nonce(11));
        let (pc_resp, respuesta_pc) =
            PairingResponder::receive_offer(pc, "PC", oferta_intruso, nonce(12)).unwrap();
        let (intruso_resp, respuesta_intruso) =
            PairingResponder::receive_offer(m2, "PC", oferta_mac, nonce(13)).unwrap();

        let (at_mac, revelado_mac) = mac_ini.receive_answer(respuesta_intruso).unwrap();
        let (_, revelado_intruso) = intruso_ini.receive_answer(respuesta_pc).unwrap();
        let at_pc = pc_resp.receive_reveal(revelado_intruso).unwrap();
        intruso_resp.receive_reveal(revelado_mac).unwrap();

        assert_ne!(at_mac.code(), at_pc.code());
    }

    #[test]
    fn a_code_different_from_the_peers_is_rejected() {
        let (mut at_mac, _) = pair_up(key(1), nonce(1), key(2), nonce(2));
        at_mac.confirm_local();
        let other = PairingCode((at_mac.code().value() + 1) % 1_000_000);
        assert!(matches!(
            at_mac.confirm_remote(other),
            Err(IdentityError::CodeMismatch { .. })
        ));
        assert!(!at_mac.is_confirmed());
    }
}
