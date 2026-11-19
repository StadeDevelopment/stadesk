use chacha20poly1305::ChaCha20Poly1305;
use zeroize::Zeroizing;

use crate::error::CoreError;
use crate::primitives::{KEY_LEN, TAG_LEN, cipher_from_key, open, seal};
use crate::wire::{Reader, Writer};

const COUNTER_LEN: usize = 8;
pub const FRAME_OVERHEAD: usize = COUNTER_LEN + TAG_LEN;
const REPLAY_WINDOW_WORDS: usize = 32;
const REPLAY_WINDOW_BITS: u64 = (REPLAY_WINDOW_WORDS * 64) as u64;

pub struct SessionSealer {
    cipher: ChaCha20Poly1305,
    next_counter: u64,
}

impl SessionSealer {
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, CoreError> {
        let counter = self.next_counter;
        if counter == u64::MAX {
            return Err(CoreError::CounterExhausted);
        }
        self.next_counter = counter + 1;
        let header = counter.to_be_bytes();
        let ciphertext = seal(&self.cipher, counter, plaintext, &header);
        let mut writer = Writer::with_capacity(COUNTER_LEN + ciphertext.len());
        writer.bytes(&header).bytes(&ciphertext);
        Ok(writer.finish())
    }
}

pub struct SessionOpener {
    cipher: ChaCha20Poly1305,
    replay: ReplayWindow,
}

impl SessionOpener {
    pub fn open(&mut self, frame: &[u8]) -> Result<Zeroizing<Vec<u8>>, CoreError> {
        let mut reader = Reader::new(frame);
        let header = reader.array::<COUNTER_LEN>()?;
        let ciphertext = reader.rest();
        if ciphertext.len() < TAG_LEN {
            return Err(CoreError::Malformed);
        }
        let counter = u64::from_be_bytes(header);
        if counter == u64::MAX {
            return Err(CoreError::Malformed);
        }
        self.replay.check(counter)?;
        let plaintext = open(&self.cipher, counter, ciphertext, &header)?;
        self.replay.accept(counter);
        Ok(plaintext)
    }
}

pub struct SessionCipher {
    sealer: SessionSealer,
    opener: SessionOpener,
}

impl SessionCipher {
    pub(crate) fn new(send_key: &[u8; KEY_LEN], receive_key: &[u8; KEY_LEN]) -> Self {
        Self {
            sealer: SessionSealer {
                cipher: cipher_from_key(send_key),
                next_counter: 0,
            },
            opener: SessionOpener {
                cipher: cipher_from_key(receive_key),
                replay: ReplayWindow::new(),
            },
        }
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, CoreError> {
        self.sealer.seal(plaintext)
    }

    pub fn open(&mut self, frame: &[u8]) -> Result<Zeroizing<Vec<u8>>, CoreError> {
        self.opener.open(frame)
    }

    pub fn split(self) -> (SessionSealer, SessionOpener) {
        (self.sealer, self.opener)
    }
}

struct ReplayWindow {
    highest: Option<u64>,
    bits: [u64; REPLAY_WINDOW_WORDS],
}

impl ReplayWindow {
    fn new() -> Self {
        Self {
            highest: None,
            bits: [0; REPLAY_WINDOW_WORDS],
        }
    }

    fn slot(counter: u64) -> (usize, u64) {
        let position = counter % REPLAY_WINDOW_BITS;
        ((position / 64) as usize, 1u64 << (position % 64))
    }

    fn check(&self, counter: u64) -> Result<(), CoreError> {
        let Some(highest) = self.highest else {
            return Ok(());
        };
        if counter > highest {
            return Ok(());
        }
        if highest - counter >= REPLAY_WINDOW_BITS {
            return Err(CoreError::Replay);
        }
        let (word, mask) = Self::slot(counter);
        if self.bits[word] & mask != 0 {
            return Err(CoreError::Replay);
        }
        Ok(())
    }

    fn accept(&mut self, counter: u64) {
        match self.highest {
            Some(highest) if counter <= highest => {}
            Some(highest) if counter - highest >= REPLAY_WINDOW_BITS => {
                self.bits = [0; REPLAY_WINDOW_WORDS];
                self.highest = Some(counter);
            }
            Some(highest) => {
                for skipped in highest + 1..=counter {
                    let (word, mask) = Self::slot(skipped);
                    self.bits[word] &= !mask;
                }
                self.highest = Some(counter);
            }
            None => self.highest = Some(counter),
        }
        let (word, mask) = Self::slot(counter);
        self.bits[word] |= mask;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (SessionCipher, SessionCipher) {
        let a = [1u8; KEY_LEN];
        let b = [2u8; KEY_LEN];
        (SessionCipher::new(&a, &b), SessionCipher::new(&b, &a))
    }

    #[test]
    fn frames_flow_in_both_directions() {
        let (mut left, mut right) = pair();
        let frame = left.seal(b"to right").unwrap();
        assert_eq!(frame.len(), b"to right".len() + FRAME_OVERHEAD);
        assert_eq!(right.open(&frame).unwrap().as_slice(), b"to right");
        let back = right.seal(b"to left").unwrap();
        assert_eq!(left.open(&back).unwrap().as_slice(), b"to left");
    }

    #[test]
    fn replayed_frame_is_rejected() {
        let (mut left, mut right) = pair();
        let frame = left.seal(b"once").unwrap();
        right.open(&frame).unwrap();
        assert_eq!(right.open(&frame), Err(CoreError::Replay));
    }

    #[test]
    fn reordered_frames_inside_window_are_accepted_once() {
        let (mut left, mut right) = pair();
        let frames: Vec<_> = (0..10).map(|_| left.seal(b"x").unwrap()).collect();
        for index in [9, 3, 0, 5, 1, 2, 4, 8, 6, 7] {
            right.open(&frames[index]).unwrap();
        }
        for frame in &frames {
            assert_eq!(right.open(frame), Err(CoreError::Replay));
        }
    }

    #[test]
    fn frame_older_than_window_is_rejected() {
        let (mut left, mut right) = pair();
        let old = left.seal(b"old").unwrap();
        let mut newest = Vec::new();
        for _ in 0..REPLAY_WINDOW_BITS {
            newest = left.seal(b"new").unwrap();
        }
        right.open(&newest).unwrap();
        assert_eq!(right.open(&old), Err(CoreError::Replay));
    }

    #[test]
    fn slot_reused_after_window_advance_does_not_block_new_counter() {
        let mut window = ReplayWindow::new();
        window.accept(5);
        window.accept(5 + REPLAY_WINDOW_BITS - 1);
        assert!(window.check(5 + REPLAY_WINDOW_BITS).is_ok());
        window.accept(5 + REPLAY_WINDOW_BITS);
        assert_eq!(window.check(5 + REPLAY_WINDOW_BITS), Err(CoreError::Replay));
        assert_eq!(window.check(5), Err(CoreError::Replay));
    }

    #[test]
    fn tampered_frame_fails_and_does_not_poison_replay_window() {
        let (mut left, mut right) = pair();
        let frame = left.seal(b"payload").unwrap();
        let mut tampered = frame.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert_eq!(right.open(&tampered), Err(CoreError::Decryption));
        assert_eq!(right.open(&frame).unwrap().as_slice(), b"payload");
    }

    #[test]
    fn tampered_counter_header_fails_authentication() {
        let (mut left, mut right) = pair();
        let mut frame = left.seal(b"payload").unwrap();
        frame[7] ^= 1;
        assert_eq!(right.open(&frame), Err(CoreError::Decryption));
    }

    #[test]
    fn short_frames_are_malformed() {
        let (_, mut right) = pair();
        assert_eq!(right.open(&[0u8; 7]), Err(CoreError::Malformed));
        assert_eq!(
            right.open(&[0u8; COUNTER_LEN + TAG_LEN - 1]),
            Err(CoreError::Malformed)
        );
    }

    #[test]
    fn split_halves_keep_counters_and_replay_state() {
        let (left, right) = pair();
        let (mut left_sealer, _) = left.split();
        let (_, mut right_opener) = right.split();
        let first = left_sealer.seal(b"one").unwrap();
        let second = left_sealer.seal(b"two").unwrap();
        assert_eq!(right_opener.open(&second).unwrap().as_slice(), b"two");
        assert_eq!(right_opener.open(&first).unwrap().as_slice(), b"one");
        assert_eq!(right_opener.open(&first), Err(CoreError::Replay));
    }

    #[test]
    fn exhausted_send_counter_refuses_to_seal() {
        let (mut left, _) = pair();
        left.sealer.next_counter = u64::MAX;
        assert_eq!(left.seal(b"x"), Err(CoreError::CounterExhausted));
    }
}
