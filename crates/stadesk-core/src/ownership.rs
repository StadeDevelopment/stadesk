use crate::error::CoreError;
use crate::identity::{Identity, PublicIdentity};
use crate::wire::Writer;

pub const CHALLENGE_LEN: usize = 32;
pub const SERVER_FINGERPRINT_LEN: usize = 32;
const OWNERSHIP_DOMAIN: &[u8] = b"stadesk-register-v1\0";

fn ownership_message(
    server_fingerprint: &[u8; SERVER_FINGERPRINT_LEN],
    challenge: &[u8; CHALLENGE_LEN],
) -> Vec<u8> {
    let mut writer =
        Writer::with_capacity(OWNERSHIP_DOMAIN.len() + SERVER_FINGERPRINT_LEN + CHALLENGE_LEN);
    writer
        .bytes(OWNERSHIP_DOMAIN)
        .bytes(server_fingerprint)
        .bytes(challenge);
    writer.finish()
}

pub fn prove_ownership(
    identity: &Identity,
    server_fingerprint: &[u8; SERVER_FINGERPRINT_LEN],
    challenge: &[u8; CHALLENGE_LEN],
) -> Result<Vec<u8>, CoreError> {
    identity.sign(&ownership_message(server_fingerprint, challenge))
}

pub fn verify_ownership(
    public: &PublicIdentity,
    server_fingerprint: &[u8; SERVER_FINGERPRINT_LEN],
    challenge: &[u8; CHALLENGE_LEN],
    proof: &[u8],
) -> Result<(), CoreError> {
    public.verify(&ownership_message(server_fingerprint, challenge), proof)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_verifies_only_for_same_identity_server_and_challenge() {
        let identity = Identity::generate().unwrap();
        let stranger = Identity::generate().unwrap();
        let server = [7u8; SERVER_FINGERPRINT_LEN];
        let challenge = [9u8; CHALLENGE_LEN];
        let proof = prove_ownership(&identity, &server, &challenge).unwrap();
        assert!(verify_ownership(identity.public(), &server, &challenge, &proof).is_ok());
        assert!(verify_ownership(stranger.public(), &server, &challenge, &proof).is_err());
        assert!(verify_ownership(identity.public(), &[8u8; 32], &challenge, &proof).is_err());
        assert!(verify_ownership(identity.public(), &server, &[1u8; 32], &proof).is_err());
    }
}
