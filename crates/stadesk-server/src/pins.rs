use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use blake2::{Blake2b256, Digest};
use stadesk_core::{PublicIdentity, StaDeskId};

const ID_TEXT_LEN: usize = 22;
const DIGEST_LEN: usize = 32;
const RECORD_LEN: usize = ID_TEXT_LEN + DIGEST_LEN;
const PIN_DOMAIN: &[u8] = b"stadesk-pin-v1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinCheck {
    Matches,
    New,
    Conflict,
}

fn digest(identity: &PublicIdentity) -> [u8; DIGEST_LEN] {
    let mut hasher = Blake2b256::new();
    hasher.update(PIN_DOMAIN);
    hasher.update(identity.encode());
    hasher.finalize().into()
}

pub struct PinStore {
    pins: HashMap<StaDeskId, [u8; DIGEST_LEN]>,
    file: Option<File>,
}

impl PinStore {
    pub fn in_memory() -> Self {
        Self {
            pins: HashMap::new(),
            file: None,
        }
    }

    pub fn open(path: &Path) -> std::io::Result<Self> {
        let mut pins = HashMap::new();
        if let Ok(mut existing) = File::open(path) {
            let mut bytes = Vec::new();
            existing.read_to_end(&mut bytes)?;
            let complete = bytes.len() - bytes.len() % RECORD_LEN;
            for record in bytes[..complete].as_chunks::<RECORD_LEN>().0 {
                let Ok(text) = std::str::from_utf8(&record[..ID_TEXT_LEN]) else {
                    continue;
                };
                let Ok(id) = StaDeskId::parse(text) else {
                    continue;
                };
                let mut hash = [0u8; DIGEST_LEN];
                hash.copy_from_slice(&record[ID_TEXT_LEN..]);
                pins.entry(id).or_insert(hash);
            }
            if complete != bytes.len() {
                OpenOptions::new()
                    .write(true)
                    .open(path)?
                    .set_len(complete as u64)?;
            }
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options.open(path)?;
        Ok(Self {
            pins,
            file: Some(file),
        })
    }

    pub fn check(&mut self, identity: &PublicIdentity) -> std::io::Result<PinCheck> {
        let id = identity.id();
        let hash = digest(identity);
        match self.pins.get(&id) {
            Some(existing) if *existing == hash => Ok(PinCheck::Matches),
            Some(_) => Ok(PinCheck::Conflict),
            None => {
                if let Some(file) = self.file.as_mut() {
                    let text = id.to_string();
                    let mut record = Vec::with_capacity(RECORD_LEN);
                    record.extend_from_slice(text.as_bytes());
                    record.extend_from_slice(&hash);
                    file.write_all(&record)?;
                    file.sync_data()?;
                }
                self.pins.insert(id, hash);
                Ok(PinCheck::New)
            }
        }
    }

    pub fn len(&self) -> usize {
        self.pins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pins.is_empty()
    }
}

pub fn default_path(data_dir: &Path) -> PathBuf {
    data_dir.join("pins.bin")
}

#[cfg(test)]
mod tests {
    use super::*;
    use stadesk_core::Identity;

    fn temporary(name: &str) -> PathBuf {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).unwrap();
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        std::env::temp_dir().join(format!("stadesk-pins-{name}-{suffix}"))
    }

    #[test]
    fn identity_text_has_the_record_width() {
        let identity = Identity::generate().unwrap();
        assert_eq!(identity.id().to_string().len(), ID_TEXT_LEN);
    }

    #[test]
    fn pins_survive_reopening_and_detect_conflicts() {
        let path = temporary("reopen");
        let first = Identity::generate().unwrap();
        let second = Identity::generate().unwrap();
        {
            let mut store = PinStore::open(&path).unwrap();
            assert_eq!(store.check(first.public()).unwrap(), PinCheck::New);
            assert_eq!(store.check(first.public()).unwrap(), PinCheck::Matches);
            assert_eq!(store.check(second.public()).unwrap(), PinCheck::New);
        }
        let mut store = PinStore::open(&path).unwrap();
        assert_eq!(store.len(), 2);
        assert_eq!(store.check(first.public()).unwrap(), PinCheck::Matches);
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            2 * RECORD_LEN as u64
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_torn_final_record_is_discarded() {
        let path = temporary("torn");
        let identity = Identity::generate().unwrap();
        {
            let mut store = PinStore::open(&path).unwrap();
            store.check(identity.public()).unwrap();
        }
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"STADESK-00").unwrap();
        drop(file);
        let store = PinStore::open(&path).unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), RECORD_LEN as u64);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn same_id_with_different_keys_conflicts() {
        let identity = Identity::generate().unwrap();
        let mut store = PinStore::in_memory();
        store.check(identity.public()).unwrap();
        let id = identity.id();
        store.pins.insert(id, [0u8; DIGEST_LEN]);
        assert_eq!(store.check(identity.public()).unwrap(), PinCheck::Conflict);
        assert!(!store.is_empty());
    }
}
