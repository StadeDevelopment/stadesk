use blake2::{Blake2b256, Digest};
use chacha20poly1305::aead::Aead;
use chacha20poly1305::aead::Payload;
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce};
use hkdf::SimpleHkdf;
use zeroize::Zeroizing;

use crate::error::CoreError;

pub(crate) const KEY_LEN: usize = 32;
pub(crate) const HASH_LEN: usize = 32;
pub(crate) const TAG_LEN: usize = 16;

pub(crate) fn random_array<const N: usize>() -> Result<Zeroizing<[u8; N]>, CoreError> {
    let mut out = Zeroizing::new([0u8; N]);
    getrandom::fill(out.as_mut_slice()).map_err(|_| CoreError::Randomness)?;
    Ok(out)
}

pub(crate) fn hash(parts: &[&[u8]]) -> [u8; HASH_LEN] {
    let mut hasher = Blake2b256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

pub(crate) struct KeySchedule {
    hkdf: SimpleHkdf<Blake2b256>,
}

impl KeySchedule {
    pub(crate) fn extract(salt: &[u8], input_key_material: &[u8]) -> Self {
        Self {
            hkdf: SimpleHkdf::<Blake2b256>::new(Some(salt), input_key_material),
        }
    }

    pub(crate) fn expand_key(&self, label: &[u8]) -> Zeroizing<[u8; KEY_LEN]> {
        let mut out = Zeroizing::new([0u8; KEY_LEN]);
        self.hkdf
            .expand(label, out.as_mut_slice())
            .expect("32 bytes is always a valid HKDF-BLAKE2b-256 output length");
        out
    }
}

pub(crate) fn cipher_from_key(key: &[u8; KEY_LEN]) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(&Key::from(*key))
}

pub(crate) fn counter_nonce(counter: u64) -> Nonce {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&counter.to_be_bytes());
    Nonce::from(nonce)
}

pub(crate) fn seal(
    cipher: &ChaCha20Poly1305,
    counter: u64,
    plaintext: &[u8],
    associated_data: &[u8],
) -> Vec<u8> {
    cipher
        .encrypt(
            &counter_nonce(counter),
            Payload {
                msg: plaintext,
                aad: associated_data,
            },
        )
        .expect("ChaCha20-Poly1305 encryption cannot fail for in-memory buffers")
}

pub(crate) fn open(
    cipher: &ChaCha20Poly1305,
    counter: u64,
    ciphertext: &[u8],
    associated_data: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CoreError> {
    cipher
        .decrypt(
            &counter_nonce(counter),
            Payload {
                msg: ciphertext,
                aad: associated_data,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CoreError::Decryption)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_of_concatenated_parts_equals_hash_of_joined_input() {
        assert_eq!(hash(&[b"stade", b"sk"]), hash(&[b"stadesk"]));
    }

    #[test]
    fn blake2b256_matches_rfc7693_style_known_answer_for_abc() {
        let expected = [
            0xbd, 0xdd, 0x81, 0x3c, 0x63, 0x42, 0x39, 0x72, 0x31, 0x71, 0xef, 0x3f, 0xee, 0x98,
            0x57, 0x9b, 0x94, 0x96, 0x4e, 0x3b, 0xb1, 0xcb, 0x3e, 0x42, 0x72, 0x62, 0xc8, 0xc0,
            0x68, 0xd5, 0x23, 0x19,
        ];
        assert_eq!(hash(&[b"abc"]), expected);
    }

    #[test]
    fn sealed_message_opens_with_same_counter_and_associated_data() {
        let key = random_array::<KEY_LEN>().unwrap();
        let cipher = cipher_from_key(&key);
        let sealed = seal(&cipher, 7, b"frame", b"header");
        assert_eq!(sealed.len(), 5 + TAG_LEN);
        assert_eq!(
            open(&cipher, 7, &sealed, b"header").unwrap().as_slice(),
            b"frame"
        );
        assert_eq!(
            open(&cipher, 8, &sealed, b"header"),
            Err(CoreError::Decryption)
        );
        assert_eq!(
            open(&cipher, 7, &sealed, b"other"),
            Err(CoreError::Decryption)
        );
    }

    #[test]
    fn different_labels_expand_to_different_keys() {
        let schedule = KeySchedule::extract(b"salt", b"secret");
        assert_ne!(*schedule.expand_key(b"a"), *schedule.expand_key(b"b"));
    }
}
