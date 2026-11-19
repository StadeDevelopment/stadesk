use ml_kem::{
    Decapsulate, DecapsulationKey, Encapsulate, EncapsulationKey, KeyExport, MlKem768, TryKeyInit,
};
use x25519_dalek::{EphemeralSecret, PublicKey};
use zeroize::Zeroizing;

use crate::error::CoreError;
use crate::id::StaDeskId;
use crate::identity::{DUAL_SIGNATURE_LEN, Identity, PUBLIC_IDENTITY_LEN, PublicIdentity};
use crate::primitives::{
    HASH_LEN, KEY_LEN, KeySchedule, cipher_from_key, hash, open, random_array, seal,
};
use crate::session::SessionCipher;
use crate::wire::{Reader, Writer};

pub const PROTOCOL_VERSION: u16 = 1;

const X25519_LEN: usize = 32;
const MLKEM768_ENCAPSULATION_KEY_LEN: usize = 1184;
const MLKEM768_CIPHERTEXT_LEN: usize = 1088;
const HELLO_NONCE_LEN: usize = 32;
const MLKEM_SEED_LEN: usize = 64;
const HANDSHAKE_COUNTER: u64 = 0;

const TRANSCRIPT_DOMAIN: &[u8] = b"stadesk-hs-v1\0";
const AUTH_DOMAIN: &[u8] = b"stadesk-auth-v1\0";
const RESPONDER_ROLE: &[u8] = b"R";
const INITIATOR_ROLE: &[u8] = b"I";
const LABEL_HANDSHAKE_RESPONDER: &[u8] = b"stadesk hs responder";
const LABEL_HANDSHAKE_INITIATOR: &[u8] = b"stadesk hs initiator";
const LABEL_SESSION_INITIATOR_TO_RESPONDER: &[u8] = b"stadesk session initiator to responder";
const LABEL_SESSION_RESPONDER_TO_INITIATOR: &[u8] = b"stadesk session responder to initiator";

pub struct HandshakeOutcome {
    pub peer: PublicIdentity,
    pub cipher: SessionCipher,
    pub channel_binding: [u8; HASH_LEN],
}

pub struct Initiator;

pub struct InitiatorAwaitingReply<'a> {
    identity: &'a Identity,
    expected_peer: StaDeskId,
    ephemeral_dh: EphemeralSecret,
    ephemeral_kem: DecapsulationKey<MlKem768>,
    transcript: [u8; HASH_LEN],
}

pub struct Responder;

pub struct ResponderAwaitingFinish {
    input_key_material: Zeroizing<[u8; 2 * KEY_LEN]>,
    initiator_handshake_key: Zeroizing<[u8; KEY_LEN]>,
    transcript: [u8; HASH_LEN],
}

impl Initiator {
    pub fn start(
        identity: &Identity,
        expected_peer: StaDeskId,
    ) -> Result<(InitiatorAwaitingReply<'_>, Vec<u8>), CoreError> {
        let ephemeral_dh = EphemeralSecret::random();
        let ephemeral_dh_public = PublicKey::from(&ephemeral_dh);
        let kem_seed = random_array::<MLKEM_SEED_LEN>()?;
        let ephemeral_kem = DecapsulationKey::<MlKem768>::from_seed(ml_kem::Seed::from(*kem_seed));
        let encapsulation_key = ephemeral_kem.encapsulation_key().to_bytes();
        let nonce = random_array::<HELLO_NONCE_LEN>()?;
        let mut writer = Writer::with_capacity(
            2 + X25519_LEN + MLKEM768_ENCAPSULATION_KEY_LEN + HELLO_NONCE_LEN,
        );
        writer
            .u16(PROTOCOL_VERSION)
            .bytes(ephemeral_dh_public.as_bytes())
            .bytes(encapsulation_key.as_slice())
            .bytes(nonce.as_slice());
        let hello = writer.finish();
        let transcript = hash(&[TRANSCRIPT_DOMAIN, &hello]);
        Ok((
            InitiatorAwaitingReply {
                identity,
                expected_peer,
                ephemeral_dh,
                ephemeral_kem,
                transcript,
            },
            hello,
        ))
    }
}

impl InitiatorAwaitingReply<'_> {
    pub fn finish(self, reply: &[u8]) -> Result<(HandshakeOutcome, Vec<u8>), CoreError> {
        let mut reader = Reader::new(reply);
        let responder_dh_public = PublicKey::from(reader.array::<X25519_LEN>()?);
        let kem_ciphertext = reader.slice(MLKEM768_CIPHERTEXT_LEN)?;
        let sealed_responder_auth = reader.rest();

        let dh_shared = self.ephemeral_dh.diffie_hellman(&responder_dh_public);
        if !dh_shared.was_contributory() {
            return Err(CoreError::InvalidKey);
        }
        let kem_shared = self
            .ephemeral_kem
            .decapsulate_slice(kem_ciphertext)
            .map_err(|_| CoreError::Malformed)?;
        let input_key_material = combine_secrets(dh_shared.as_bytes(), kem_shared.as_slice());

        let transcript = hash(&[
            &self.transcript,
            responder_dh_public.as_bytes(),
            kem_ciphertext,
        ]);
        let schedule = KeySchedule::extract(&transcript, input_key_material.as_slice());
        let responder_handshake_key = schedule.expand_key(LABEL_HANDSHAKE_RESPONDER);
        let initiator_handshake_key = schedule.expand_key(LABEL_HANDSHAKE_INITIATOR);

        let responder_auth = open(
            &cipher_from_key(&responder_handshake_key),
            HANDSHAKE_COUNTER,
            sealed_responder_auth,
            &transcript,
        )?;
        let (peer, signature) = split_auth_payload(&responder_auth)?;
        if peer.id() != self.expected_peer {
            return Err(CoreError::IdentityMismatch);
        }
        peer.verify(&auth_message(RESPONDER_ROLE, &transcript), signature)?;

        let transcript = hash(&[&transcript, sealed_responder_auth]);
        let own_signature = self
            .identity
            .sign(&auth_message(INITIATOR_ROLE, &transcript))?;
        let auth_payload = auth_payload(self.identity.public(), &own_signature);
        let sealed_initiator_auth = seal(
            &cipher_from_key(&initiator_handshake_key),
            HANDSHAKE_COUNTER,
            &auth_payload,
            &transcript,
        );

        let transcript = hash(&[&transcript, &sealed_initiator_auth]);
        let (to_responder, to_initiator) = session_keys(&transcript, &input_key_material);
        Ok((
            HandshakeOutcome {
                peer,
                cipher: SessionCipher::new(&to_responder, &to_initiator),
                channel_binding: transcript,
            },
            sealed_initiator_auth,
        ))
    }
}

impl Responder {
    pub fn respond(
        identity: &Identity,
        hello: &[u8],
    ) -> Result<(ResponderAwaitingFinish, Vec<u8>), CoreError> {
        let mut reader = Reader::new(hello);
        let version = reader.u16()?;
        if version != PROTOCOL_VERSION {
            return Err(CoreError::UnsupportedVersion(version));
        }
        let initiator_dh_public = PublicKey::from(reader.array::<X25519_LEN>()?);
        let encapsulation_key_bytes = reader.slice(MLKEM768_ENCAPSULATION_KEY_LEN)?;
        reader.slice(HELLO_NONCE_LEN)?;
        reader.finish()?;

        let encapsulation_key =
            EncapsulationKey::<MlKem768>::new_from_slice(encapsulation_key_bytes)
                .map_err(|_| CoreError::InvalidKey)?;
        let (kem_ciphertext, kem_shared) = encapsulation_key.encapsulate();
        let ephemeral_dh = EphemeralSecret::random();
        let ephemeral_dh_public = PublicKey::from(&ephemeral_dh);
        let dh_shared = ephemeral_dh.diffie_hellman(&initiator_dh_public);
        if !dh_shared.was_contributory() {
            return Err(CoreError::InvalidKey);
        }
        let input_key_material = combine_secrets(dh_shared.as_bytes(), kem_shared.as_slice());

        let transcript = hash(&[TRANSCRIPT_DOMAIN, hello]);
        let transcript = hash(&[
            &transcript,
            ephemeral_dh_public.as_bytes(),
            kem_ciphertext.as_slice(),
        ]);
        let schedule = KeySchedule::extract(&transcript, input_key_material.as_slice());
        let responder_handshake_key = schedule.expand_key(LABEL_HANDSHAKE_RESPONDER);
        let initiator_handshake_key = schedule.expand_key(LABEL_HANDSHAKE_INITIATOR);

        let signature = identity.sign(&auth_message(RESPONDER_ROLE, &transcript))?;
        let sealed_responder_auth = seal(
            &cipher_from_key(&responder_handshake_key),
            HANDSHAKE_COUNTER,
            &auth_payload(identity.public(), &signature),
            &transcript,
        );

        let mut writer = Writer::with_capacity(
            X25519_LEN + MLKEM768_CIPHERTEXT_LEN + sealed_responder_auth.len(),
        );
        writer
            .bytes(ephemeral_dh_public.as_bytes())
            .bytes(kem_ciphertext.as_slice())
            .bytes(&sealed_responder_auth);
        let transcript = hash(&[&transcript, &sealed_responder_auth]);
        Ok((
            ResponderAwaitingFinish {
                input_key_material,
                initiator_handshake_key,
                transcript,
            },
            writer.finish(),
        ))
    }
}

impl ResponderAwaitingFinish {
    pub fn finish(self, sealed_initiator_auth: &[u8]) -> Result<HandshakeOutcome, CoreError> {
        let initiator_auth = open(
            &cipher_from_key(&self.initiator_handshake_key),
            HANDSHAKE_COUNTER,
            sealed_initiator_auth,
            &self.transcript,
        )?;
        let (peer, signature) = split_auth_payload(&initiator_auth)?;
        peer.verify(&auth_message(INITIATOR_ROLE, &self.transcript), signature)?;

        let transcript = hash(&[&self.transcript, sealed_initiator_auth]);
        let (to_responder, to_initiator) = session_keys(&transcript, &self.input_key_material);
        Ok(HandshakeOutcome {
            peer,
            cipher: SessionCipher::new(&to_initiator, &to_responder),
            channel_binding: transcript,
        })
    }
}

fn combine_secrets(dh_shared: &[u8; 32], kem_shared: &[u8]) -> Zeroizing<[u8; 2 * KEY_LEN]> {
    let mut out = Zeroizing::new([0u8; 2 * KEY_LEN]);
    out[..KEY_LEN].copy_from_slice(dh_shared);
    out[KEY_LEN..].copy_from_slice(kem_shared);
    out
}

fn session_keys(
    transcript: &[u8; HASH_LEN],
    input_key_material: &[u8; 2 * KEY_LEN],
) -> (Zeroizing<[u8; KEY_LEN]>, Zeroizing<[u8; KEY_LEN]>) {
    let schedule = KeySchedule::extract(transcript, input_key_material);
    (
        schedule.expand_key(LABEL_SESSION_INITIATOR_TO_RESPONDER),
        schedule.expand_key(LABEL_SESSION_RESPONDER_TO_INITIATOR),
    )
}

fn auth_message(role: &[u8], transcript: &[u8; HASH_LEN]) -> Vec<u8> {
    let mut writer = Writer::with_capacity(AUTH_DOMAIN.len() + role.len() + HASH_LEN);
    writer.bytes(AUTH_DOMAIN).bytes(role).bytes(transcript);
    writer.finish()
}

fn auth_payload(public: &PublicIdentity, signature: &[u8]) -> Vec<u8> {
    let mut writer = Writer::with_capacity(PUBLIC_IDENTITY_LEN + DUAL_SIGNATURE_LEN);
    writer.bytes(public.encode()).bytes(signature);
    writer.finish()
}

fn split_auth_payload(payload: &[u8]) -> Result<(PublicIdentity, &[u8]), CoreError> {
    let mut reader = Reader::new(payload);
    let peer = PublicIdentity::decode(reader.slice(PUBLIC_IDENTITY_LEN)?)?;
    let signature = reader.slice(DUAL_SIGNATURE_LEN)?;
    reader.finish()?;
    Ok((peer, signature))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Completed {
        initiator: HandshakeOutcome,
        responder: HandshakeOutcome,
    }

    fn run(initiator: &Identity, responder: &Identity) -> Completed {
        let (waiting, hello) = Initiator::start(initiator, responder.id()).unwrap();
        let (responder_waiting, reply) = Responder::respond(responder, &hello).unwrap();
        let (initiator_outcome, finish) = waiting.finish(&reply).unwrap();
        let responder_outcome = responder_waiting.finish(&finish).unwrap();
        Completed {
            initiator: initiator_outcome,
            responder: responder_outcome,
        }
    }

    #[test]
    fn message_sizes_match_wire_layout() {
        let controller = Identity::generate().unwrap();
        let host = Identity::generate().unwrap();
        let (waiting, hello) = Initiator::start(&controller, host.id()).unwrap();
        assert_eq!(
            hello.len(),
            2 + X25519_LEN + MLKEM768_ENCAPSULATION_KEY_LEN + HELLO_NONCE_LEN
        );
        let (_, reply) = Responder::respond(&host, &hello).unwrap();
        let auth_len = PUBLIC_IDENTITY_LEN + DUAL_SIGNATURE_LEN + crate::primitives::TAG_LEN;
        assert_eq!(reply.len(), X25519_LEN + MLKEM768_CIPHERTEXT_LEN + auth_len);
        let (_, finish) = waiting.finish(&reply).unwrap();
        assert_eq!(finish.len(), auth_len);
    }

    #[test]
    fn both_sides_learn_each_other_and_agree_on_channel_binding() {
        let controller = Identity::generate().unwrap();
        let host = Identity::generate().unwrap();
        let completed = run(&controller, &host);
        assert_eq!(&completed.initiator.peer, host.public());
        assert_eq!(&completed.responder.peer, controller.public());
        assert_eq!(
            completed.initiator.channel_binding,
            completed.responder.channel_binding
        );
    }

    #[test]
    fn session_ciphers_interoperate_in_both_directions() {
        let controller = Identity::generate().unwrap();
        let host = Identity::generate().unwrap();
        let mut completed = run(&controller, &host);
        let frame = completed.initiator.cipher.seal(b"mouse move").unwrap();
        assert_eq!(
            completed.responder.cipher.open(&frame).unwrap().as_slice(),
            b"mouse move"
        );
        let frame = completed.responder.cipher.seal(b"video frame").unwrap();
        assert_eq!(
            completed.initiator.cipher.open(&frame).unwrap().as_slice(),
            b"video frame"
        );
    }

    #[test]
    fn each_handshake_produces_fresh_keys() {
        let controller = Identity::generate().unwrap();
        let host = Identity::generate().unwrap();
        let first = run(&controller, &host);
        let mut second = run(&controller, &host);
        assert_ne!(
            first.initiator.channel_binding,
            second.initiator.channel_binding
        );
        let mut first_initiator = first.initiator.cipher;
        let frame = first_initiator.seal(b"x").unwrap();
        assert_eq!(
            second.responder.cipher.open(&frame),
            Err(CoreError::Decryption)
        );
    }

    #[test]
    fn initiator_rejects_responder_with_unexpected_id() {
        let controller = Identity::generate().unwrap();
        let wanted = Identity::generate().unwrap();
        let impostor = Identity::generate().unwrap();
        let (waiting, hello) = Initiator::start(&controller, wanted.id()).unwrap();
        let (_, reply) = Responder::respond(&impostor, &hello).unwrap();
        assert!(matches!(
            waiting.finish(&reply),
            Err(CoreError::IdentityMismatch)
        ));
    }

    #[test]
    fn responder_rejects_unsupported_version() {
        let controller = Identity::generate().unwrap();
        let host = Identity::generate().unwrap();
        let (_, mut hello) = Initiator::start(&controller, host.id()).unwrap();
        hello[..2].copy_from_slice(&2u16.to_be_bytes());
        assert!(matches!(
            Responder::respond(&host, &hello),
            Err(CoreError::UnsupportedVersion(2))
        ));
    }

    #[test]
    fn responder_rejects_truncated_or_padded_hello() {
        let controller = Identity::generate().unwrap();
        let host = Identity::generate().unwrap();
        let (_, hello) = Initiator::start(&controller, host.id()).unwrap();
        assert!(matches!(
            Responder::respond(&host, &hello[..hello.len() - 1]),
            Err(CoreError::Malformed)
        ));
        let mut padded = hello.clone();
        padded.push(0);
        assert!(matches!(
            Responder::respond(&host, &padded),
            Err(CoreError::Malformed)
        ));
    }

    #[test]
    fn low_order_dh_public_key_is_rejected_by_both_sides() {
        let controller = Identity::generate().unwrap();
        let host = Identity::generate().unwrap();
        let (waiting, mut hello) = Initiator::start(&controller, host.id()).unwrap();
        let original_hello = hello.clone();
        hello[2..2 + X25519_LEN].copy_from_slice(&[0u8; X25519_LEN]);
        assert!(matches!(
            Responder::respond(&host, &hello),
            Err(CoreError::InvalidKey)
        ));
        let (_, mut reply) = Responder::respond(&host, &original_hello).unwrap();
        reply[..X25519_LEN].copy_from_slice(&[0u8; X25519_LEN]);
        assert!(matches!(waiting.finish(&reply), Err(CoreError::InvalidKey)));
    }

    #[test]
    fn tampering_with_any_reply_region_breaks_the_handshake() {
        let controller = Identity::generate().unwrap();
        let host = Identity::generate().unwrap();
        for position in [
            0,
            X25519_LEN + 10,
            X25519_LEN + MLKEM768_CIPHERTEXT_LEN + 10,
        ] {
            let (waiting, hello) = Initiator::start(&controller, host.id()).unwrap();
            let (_, mut reply) = Responder::respond(&host, &hello).unwrap();
            reply[position] ^= 1;
            assert!(waiting.finish(&reply).is_err());
        }
    }

    #[test]
    fn tampering_with_hello_makes_initiator_reject_reply() {
        let controller = Identity::generate().unwrap();
        let host = Identity::generate().unwrap();
        let (waiting, mut hello) = Initiator::start(&controller, host.id()).unwrap();
        let last = hello.len() - 1;
        hello[last] ^= 1;
        let (_, reply) = Responder::respond(&host, &hello).unwrap();
        assert!(matches!(waiting.finish(&reply), Err(CoreError::Decryption)));
    }

    #[test]
    fn tampered_finish_message_is_rejected_by_responder() {
        let controller = Identity::generate().unwrap();
        let host = Identity::generate().unwrap();
        let (waiting, hello) = Initiator::start(&controller, host.id()).unwrap();
        let (responder_waiting, reply) = Responder::respond(&host, &hello).unwrap();
        let (_, mut finish) = waiting.finish(&reply).unwrap();
        finish[0] ^= 1;
        assert!(matches!(
            responder_waiting.finish(&finish),
            Err(CoreError::Decryption)
        ));
    }

    #[test]
    fn finish_message_from_another_session_is_rejected() {
        let controller = Identity::generate().unwrap();
        let host = Identity::generate().unwrap();
        let (first_waiting, first_hello) = Initiator::start(&controller, host.id()).unwrap();
        let (_, first_reply) = Responder::respond(&host, &first_hello).unwrap();
        let (_, first_finish) = first_waiting.finish(&first_reply).unwrap();
        let (_, second_hello) = Initiator::start(&controller, host.id()).unwrap();
        let (second_responder, _) = Responder::respond(&host, &second_hello).unwrap();
        assert!(second_responder.finish(&first_finish).is_err());
    }
}
