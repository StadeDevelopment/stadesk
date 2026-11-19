use std::fmt;
use std::str::FromStr;

use crate::error::CoreError;
use crate::primitives::hash;

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const DISPLAY_PREFIX: &str = "STADESK-";
const BARE_PREFIX: &str = "STADESK";
const DERIVATION_DOMAIN: &[u8] = b"stadesk-id-v1\0";
const PAYLOAD_LEN: usize = 12;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StaDeskId([u8; PAYLOAD_LEN]);

impl StaDeskId {
    pub fn derive(ed25519_public_key: &[u8], mldsa_public_key: &[u8]) -> Self {
        let digest = hash(&[DERIVATION_DOMAIN, ed25519_public_key, mldsa_public_key]);
        let mut top = [0u8; 8];
        top.copy_from_slice(&digest[..8]);
        let id60 = u64::from_be_bytes(top) >> 4;
        let extra4 = u64::from(digest[8] >> 4);
        let combined = (id60 << 4) | extra4;
        let mut payload = [0u8; PAYLOAD_LEN];
        for (index, slot) in payload.iter_mut().enumerate() {
            let shift = 64 - 5 * (index as u32 + 1);
            *slot = ALPHABET[((combined >> shift) & 0x1f) as usize];
        }
        Self(payload)
    }

    pub fn parse(input: &str) -> Result<Self, CoreError> {
        let mut cleaned = String::with_capacity(input.len());
        for character in input.trim().chars().flat_map(char::to_uppercase) {
            match character {
                'O' => cleaned.push('0'),
                'I' | 'L' => cleaned.push('1'),
                ' ' | '-' | '_' => {}
                other => cleaned.push(other),
            }
        }
        let payload = if cleaned.len() == BARE_PREFIX.len() + PAYLOAD_LEN {
            cleaned.strip_prefix(BARE_PREFIX).unwrap_or(&cleaned)
        } else {
            &cleaned
        };
        if payload.len() != PAYLOAD_LEN || !payload.bytes().all(|byte| ALPHABET.contains(&byte)) {
            return Err(CoreError::InvalidId);
        }
        let mut out = [0u8; PAYLOAD_LEN];
        out.copy_from_slice(payload.as_bytes());
        Ok(Self(out))
    }

    fn payload_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("payload only ever holds ASCII alphabet bytes")
    }
}

impl fmt::Display for StaDeskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let payload = self.payload_str();
        write!(
            formatter,
            "{DISPLAY_PREFIX}{}-{}-{}",
            &payload[0..4],
            &payload[4..8],
            &payload[8..12]
        )
    }
}

impl fmt::Debug for StaDeskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl FromStr for StaDeskId {
    type Err = CoreError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_id_has_display_shape_and_round_trips_through_parse() {
        let id = StaDeskId::derive(&[1u8; 32], &[2u8; 64]);
        let text = id.to_string();
        assert_eq!(text.len(), DISPLAY_PREFIX.len() + PAYLOAD_LEN + 2);
        assert!(text.starts_with(DISPLAY_PREFIX));
        assert_eq!(StaDeskId::parse(&text).unwrap(), id);
    }

    #[test]
    fn derivation_is_deterministic_and_key_sensitive() {
        let a = StaDeskId::derive(&[1u8; 32], &[2u8; 64]);
        assert_eq!(a, StaDeskId::derive(&[1u8; 32], &[2u8; 64]));
        assert_ne!(a, StaDeskId::derive(&[1u8; 32], &[3u8; 64]));
        assert_ne!(a, StaDeskId::derive(&[0u8; 32], &[2u8; 64]));
    }

    #[test]
    fn parse_accepts_human_typing_variations() {
        let id = StaDeskId::parse("STADESK-0123-4567-89AB").unwrap();
        assert_eq!(StaDeskId::parse(" stadesk 0123 4567 89ab ").unwrap(), id);
        assert_eq!(StaDeskId::parse("o123-4567-89ab").unwrap(), id);
        assert_eq!(StaDeskId::parse("STADESK_0123_4567_89AB").unwrap(), id);
        assert_eq!(id.to_string(), "STADESK-0123-4567-89AB");
    }

    #[test]
    fn bare_payload_that_begins_with_prefix_letters_is_kept_intact() {
        let id = StaDeskId::parse("STADESK12345").unwrap();
        assert_eq!(id.to_string(), "STADESK-STAD-ESK1-2345");
        assert_eq!(StaDeskId::parse(&id.to_string()).unwrap(), id);
    }

    #[test]
    fn parse_rejects_wrong_length_and_foreign_characters() {
        assert_eq!(
            StaDeskId::parse("STADESK-0123-4567-89A"),
            Err(CoreError::InvalidId)
        );
        assert_eq!(
            StaDeskId::parse("STADESK-0123-4567-89ABC"),
            Err(CoreError::InvalidId)
        );
        assert_eq!(
            StaDeskId::parse("STADESK-0123-4567-89AU"),
            Err(CoreError::InvalidId)
        );
        assert_eq!(
            StaDeskId::parse("STADESK-0123-4567-89A!"),
            Err(CoreError::InvalidId)
        );
        assert_eq!(
            StaDeskId::parse("STADESK-0123-4567-89Aç"),
            Err(CoreError::InvalidId)
        );
    }
}
