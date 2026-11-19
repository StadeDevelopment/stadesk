use ed25519_dalek::Signer as _;
use ml_dsa::{KeyExport, KeyInit, MlDsa65};
use zeroize::Zeroizing;

use crate::error::CoreError;
use crate::id::StaDeskId;
use crate::primitives::random_array;
use crate::wire::{Reader, Writer};

pub(crate) const ED25519_PUBLIC_LEN: usize = 32;
pub(crate) const ED25519_SIGNATURE_LEN: usize = 64;
pub(crate) const MLDSA65_PUBLIC_LEN: usize = 1952;
pub(crate) const MLDSA65_SIGNATURE_LEN: usize = 3309;
pub const PUBLIC_IDENTITY_LEN: usize = ED25519_PUBLIC_LEN + MLDSA65_PUBLIC_LEN;
pub const DUAL_SIGNATURE_LEN: usize = ED25519_SIGNATURE_LEN + MLDSA65_SIGNATURE_LEN;
const SECRET_SEEDS_LEN: usize = 64;

pub struct Identity {
    ed25519: ed25519_dalek::SigningKey,
    mldsa: ml_dsa::SigningKey<MlDsa65>,
    public: PublicIdentity,
}

impl Identity {
    pub fn generate() -> Result<Self, CoreError> {
        Ok(Self::from_secret_seeds(
            &*random_array::<SECRET_SEEDS_LEN>()?
        ))
    }

    pub fn from_secret_seeds(seeds: &[u8; SECRET_SEEDS_LEN]) -> Self {
        let mut ed25519_seed = Zeroizing::new([0u8; 32]);
        ed25519_seed.copy_from_slice(&seeds[..32]);
        let mut mldsa_seed = Zeroizing::new([0u8; 32]);
        mldsa_seed.copy_from_slice(&seeds[32..]);
        let ed25519 = ed25519_dalek::SigningKey::from_bytes(&ed25519_seed);
        let mldsa = ml_dsa::SigningKey::<MlDsa65>::from_seed(&ml_dsa::Seed::from(*mldsa_seed));
        let public = PublicIdentity::from_keys(
            ed25519.verifying_key(),
            ml_dsa::signature::Keypair::verifying_key(&mldsa),
        );
        Self {
            ed25519,
            mldsa,
            public,
        }
    }

    pub fn secret_seeds(&self) -> Zeroizing<[u8; SECRET_SEEDS_LEN]> {
        let mut out = Zeroizing::new([0u8; SECRET_SEEDS_LEN]);
        out[..32].copy_from_slice(self.ed25519.as_bytes());
        out[32..].copy_from_slice(self.mldsa.as_seed().as_slice());
        out
    }

    pub fn public(&self) -> &PublicIdentity {
        &self.public
    }

    pub fn id(&self) -> StaDeskId {
        self.public.id
    }

    pub(crate) fn sign(&self, message: &[u8]) -> Result<Vec<u8>, CoreError> {
        let classical = self.ed25519.sign(message);
        let post_quantum = self
            .mldsa
            .try_sign(message)
            .map_err(|_| CoreError::BadSignature)?;
        let mut writer = Writer::with_capacity(DUAL_SIGNATURE_LEN);
        writer
            .bytes(&classical.to_bytes())
            .bytes(post_quantum.encode().as_slice());
        Ok(writer.finish())
    }
}

#[derive(Clone)]
pub struct PublicIdentity {
    ed25519: ed25519_dalek::VerifyingKey,
    mldsa: ml_dsa::VerifyingKey<MlDsa65>,
    encoded: Vec<u8>,
    id: StaDeskId,
}

impl PublicIdentity {
    fn from_keys(
        ed25519: ed25519_dalek::VerifyingKey,
        mldsa: ml_dsa::VerifyingKey<MlDsa65>,
    ) -> Self {
        let mldsa_bytes = mldsa.to_bytes();
        let mut writer = Writer::with_capacity(PUBLIC_IDENTITY_LEN);
        writer
            .bytes(ed25519.as_bytes())
            .bytes(mldsa_bytes.as_slice());
        let id = StaDeskId::derive(ed25519.as_bytes(), mldsa_bytes.as_slice());
        Self {
            ed25519,
            mldsa,
            encoded: writer.finish(),
            id,
        }
    }

    pub fn decode(input: &[u8]) -> Result<Self, CoreError> {
        let mut reader = Reader::new(input);
        let ed25519_bytes = reader.array::<ED25519_PUBLIC_LEN>()?;
        let mldsa_bytes = reader.slice(MLDSA65_PUBLIC_LEN)?;
        reader.finish()?;
        let ed25519 = ed25519_dalek::VerifyingKey::from_bytes(&ed25519_bytes)
            .map_err(|_| CoreError::InvalidKey)?;
        if ed25519.is_weak() {
            return Err(CoreError::InvalidKey);
        }
        let mldsa = ml_dsa::VerifyingKey::<MlDsa65>::new_from_slice(mldsa_bytes)
            .map_err(|_| CoreError::InvalidKey)?;
        Ok(Self::from_keys(ed25519, mldsa))
    }

    pub fn encode(&self) -> &[u8] {
        &self.encoded
    }

    pub fn id(&self) -> StaDeskId {
        self.id
    }

    pub(crate) fn verify(&self, message: &[u8], signature: &[u8]) -> Result<(), CoreError> {
        let mut reader = Reader::new(signature);
        let classical_bytes = reader.array::<ED25519_SIGNATURE_LEN>()?;
        let post_quantum_bytes = reader.slice(MLDSA65_SIGNATURE_LEN)?;
        reader.finish()?;
        let classical = ed25519_dalek::Signature::from_bytes(&classical_bytes);
        let post_quantum = ml_dsa::Signature::<MlDsa65>::try_from(post_quantum_bytes)
            .map_err(|_| CoreError::BadSignature)?;
        let classical_ok = self.ed25519.verify_strict(message, &classical).is_ok();
        let post_quantum_ok =
            ml_dsa::signature::Verifier::verify(&self.mldsa, message, &post_quantum).is_ok();
        if classical_ok && post_quantum_ok {
            Ok(())
        } else {
            Err(CoreError::BadSignature)
        }
    }
}

impl PartialEq for PublicIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.encoded == other.encoded
    }
}

impl Eq for PublicIdentity {}

impl std::fmt::Debug for PublicIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicIdentity")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_sizes_match_declared_wire_constants() {
        let identity = Identity::generate().unwrap();
        assert_eq!(identity.public().encode().len(), PUBLIC_IDENTITY_LEN);
        assert_eq!(identity.sign(b"m").unwrap().len(), DUAL_SIGNATURE_LEN);
    }

    #[test]
    fn identity_restored_from_seeds_has_same_public_keys_and_id() {
        let identity = Identity::generate().unwrap();
        let restored = Identity::from_secret_seeds(&identity.secret_seeds());
        assert_eq!(restored.public(), identity.public());
        assert_eq!(restored.id(), identity.id());
    }

    #[test]
    fn public_identity_round_trips_through_encoding() {
        let identity = Identity::generate().unwrap();
        let decoded = PublicIdentity::decode(identity.public().encode()).unwrap();
        assert_eq!(&decoded, identity.public());
        assert_eq!(decoded.id(), identity.id());
    }

    #[test]
    fn dual_signature_verifies_only_for_signed_message_and_signer() {
        let signer = Identity::generate().unwrap();
        let stranger = Identity::generate().unwrap();
        let signature = signer.sign(b"message").unwrap();
        assert!(signer.public().verify(b"message", &signature).is_ok());
        assert_eq!(
            signer.public().verify(b"other", &signature),
            Err(CoreError::BadSignature)
        );
        assert_eq!(
            stranger.public().verify(b"message", &signature),
            Err(CoreError::BadSignature)
        );
    }

    #[test]
    fn tampering_with_either_half_of_dual_signature_fails_verification() {
        let signer = Identity::generate().unwrap();
        let signature = signer.sign(b"message").unwrap();
        let mut classical_broken = signature.clone();
        classical_broken[0] ^= 1;
        assert!(
            signer
                .public()
                .verify(b"message", &classical_broken)
                .is_err()
        );
        let mut post_quantum_broken = signature.clone();
        post_quantum_broken[ED25519_SIGNATURE_LEN + 100] ^= 1;
        assert!(
            signer
                .public()
                .verify(b"message", &post_quantum_broken)
                .is_err()
        );
        assert!(signer.public().verify(b"message", &signature[1..]).is_err());
    }

    #[test]
    fn decode_rejects_wrong_length_and_weak_ed25519_key() {
        let identity = Identity::generate().unwrap();
        let encoded = identity.public().encode();
        assert_eq!(
            PublicIdentity::decode(&encoded[1..]),
            Err(CoreError::Malformed)
        );
        let mut weak = encoded.to_vec();
        weak[..32].copy_from_slice(&[0u8; 32]);
        weak[0] = 1;
        assert_eq!(PublicIdentity::decode(&weak), Err(CoreError::InvalidKey));
    }
}
