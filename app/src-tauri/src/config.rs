use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use stadesk_core::{AccessGate, AccessMethods, PermanentPassword, PublicIdentity};

const CONFIG_FILE: &str = "config.json";
const IDENTITY_FILE: &str = "identity.key";
const IDENTITY_SEED_LEN: usize = 64;
pub const DEFAULT_SERVER_ADDRESS: &str = "relay.stade.dev:21300";
pub const DEFAULT_SERVER_KEY: &str =
    "0XVC-KY51-CPRH-HA18-AQSD-RMJM-93ZN-F4CY-GWE8-1RNX-2N96-RFY4-5NG0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Methods {
    pub accept_prompt: bool,
    pub one_time_password: bool,
    pub permanent_password: bool,
    pub trusted_devices: bool,
}

impl From<AccessMethods> for Methods {
    fn from(methods: AccessMethods) -> Self {
        Self {
            accept_prompt: methods.accept_prompt,
            one_time_password: methods.one_time_password,
            permanent_password: methods.permanent_password,
            trusted_devices: methods.trusted_devices,
        }
    }
}

impl From<Methods> for AccessMethods {
    fn from(methods: Methods) -> Self {
        Self {
            accept_prompt: methods.accept_prompt,
            one_time_password: methods.one_time_password,
            permanent_password: methods.permanent_password,
            trusted_devices: methods.trusted_devices,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub server_address: String,
    pub server_key: String,
    pub methods: Methods,
    pub permanent_password: Option<String>,
    pub trusted_devices: Vec<String>,
    pub language: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_address: DEFAULT_SERVER_ADDRESS.to_owned(),
            server_key: DEFAULT_SERVER_KEY.to_owned(),
            methods: AccessMethods::default().into(),
            permanent_password: None,
            trusted_devices: Vec::new(),
            language: "auto".to_owned(),
        }
    }
}

pub fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

impl Config {
    pub fn trusted_identities(&self) -> Vec<PublicIdentity> {
        self.trusted_devices
            .iter()
            .filter_map(|encoded| decode_hex(encoded))
            .filter_map(|bytes| PublicIdentity::decode(&bytes).ok())
            .collect()
    }

    pub fn permanent(&self) -> Option<PermanentPassword> {
        self.permanent_password
            .as_deref()
            .and_then(decode_hex)
            .and_then(|bytes| PermanentPassword::decode(&bytes).ok())
    }

    pub fn apply_to(&self, gate: &mut AccessGate) {
        gate.set_methods(self.methods.into());
        gate.set_permanent_password(self.permanent());
        for device in gate.trusted_devices().to_vec() {
            gate.forget_device(&device);
        }
        for device in self.trusted_identities() {
            gate.trust_device(device);
        }
    }
}

fn write_atomically(path: &Path, contents: &[u8], private: bool) -> std::io::Result<()> {
    let temporary = path.with_extension("tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = options.open(&temporary)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)
}

pub struct Store {
    directory: PathBuf,
}

impl Store {
    pub fn new(directory: PathBuf) -> std::io::Result<Self> {
        fs::create_dir_all(&directory)?;
        Ok(Self { directory })
    }

    pub fn load_config(&self) -> Config {
        fs::read(self.directory.join(CONFIG_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save_config(&self, config: &Config) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(config).map_err(std::io::Error::other)?;
        write_atomically(&self.directory.join(CONFIG_FILE), &json, true)
    }

    pub fn load_or_create_identity(&self) -> std::io::Result<stadesk_core::Identity> {
        let path = self.directory.join(IDENTITY_FILE);
        match fs::read(&path) {
            Ok(bytes) => return identity_from_bytes(&bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::symlink_metadata(&path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                    Ok(_) => return identity_from_bytes(&fs::read(&path)?),
                }
            }
            Err(error) => return Err(error),
        }
        let identity = stadesk_core::Identity::generate().map_err(std::io::Error::other)?;
        let mut temporary = tempfile::Builder::new()
            .prefix("identity-")
            .tempfile_in(&self.directory)?;
        if let Err(error) = temporary
            .write_all(identity.secret_seeds().as_slice())
            .and_then(|()| temporary.as_file().sync_all())
        {
            return match temporary.close() {
                Ok(()) => Err(error),
                Err(cleanup_error) => Err(std::io::Error::other(format!(
                    "{error}; could not remove the temporary identity file: {cleanup_error}"
                ))),
            };
        }
        let temporary_path = temporary.path().to_owned();
        match temporary.persist_noclobber(&path) {
            Ok(_) => match fs::remove_file(&temporary_path) {
                Ok(()) => Ok(identity),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(identity),
                Err(error) => Err(std::io::Error::other(format!(
                    "could not remove the temporary identity file: {error}"
                ))),
            },
            Err(error) => {
                let kind = error.error.kind();
                let persist_error = error.error;
                if let Err(cleanup_error) = error.file.close() {
                    return Err(std::io::Error::other(format!(
                        "{persist_error}; could not remove the temporary identity file: {cleanup_error}"
                    )));
                }
                if kind == std::io::ErrorKind::AlreadyExists {
                    return identity_from_bytes(&fs::read(&path)?);
                }
                Err(persist_error)
            }
        }
    }
}

fn identity_from_bytes(bytes: &[u8]) -> std::io::Result<stadesk_core::Identity> {
    let seeds: [u8; IDENTITY_SEED_LEN] = bytes
        .try_into()
        .map_err(|_| std::io::Error::other("the identity file is damaged"))?;
    Ok(stadesk_core::Identity::from_secret_seeds(&seeds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_directory() -> PathBuf {
        let mut name = [0u8; 8];
        getrandom::fill(&mut name).unwrap();
        std::env::temp_dir().join(format!("stadesk-test-{}", encode_hex(&name)))
    }

    #[test]
    fn default_server_is_valid() {
        assert!(crate::state::server_endpoint(DEFAULT_SERVER_ADDRESS).is_some());
        assert!(
            DEFAULT_SERVER_KEY
                .parse::<stadesk_net::ServerFingerprint>()
                .is_ok()
        );
    }

    #[test]
    fn hex_round_trips_and_rejects_garbage() {
        assert_eq!(
            decode_hex(&encode_hex(&[0, 1, 0xab, 0xff])),
            Some(vec![0, 1, 0xab, 0xff])
        );
        assert_eq!(decode_hex("abc"), None);
        assert_eq!(decode_hex("zz"), None);
        assert_eq!(decode_hex("é0"), None);
    }

    #[test]
    fn config_and_identity_survive_a_restart() {
        let directory = temporary_directory();
        let store = Store::new(directory.clone()).unwrap();
        assert_eq!(store.load_config(), Config::default());
        let identity = store.load_or_create_identity().unwrap();
        let config = Config {
            server_address: "relay.example:21300".to_owned(),
            trusted_devices: vec![encode_hex(identity.public().encode())],
            permanent_password: Some(encode_hex(
                &PermanentPassword::from_plaintext("correct horse")
                    .unwrap()
                    .encode(),
            )),
            ..Config::default()
        };
        store.save_config(&config).unwrap();

        let reopened = Store::new(directory.clone()).unwrap();
        assert_eq!(reopened.load_config(), config);
        assert_eq!(
            reopened.load_or_create_identity().unwrap().id(),
            identity.id()
        );
        assert_eq!(config.trusted_identities(), vec![identity.public().clone()]);
        assert!(config.permanent().is_some());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(directory.join(IDENTITY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn damaged_files_fall_back_or_fail_loudly() {
        let directory = temporary_directory();
        let store = Store::new(directory.clone()).unwrap();
        fs::write(directory.join(CONFIG_FILE), b"{ not json").unwrap();
        assert_eq!(store.load_config(), Config::default());
        fs::write(directory.join(IDENTITY_FILE), b"short").unwrap();
        assert!(store.load_or_create_identity().is_err());
        assert_eq!(fs::read(directory.join(IDENTITY_FILE)).unwrap(), b"short");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unreadable_identity_is_not_replaced() {
        let directory = temporary_directory();
        let store = Store::new(directory.clone()).unwrap();
        let path = directory.join(IDENTITY_FILE);
        fs::create_dir(&path).unwrap();
        assert!(store.load_or_create_identity().is_err());
        assert!(path.is_dir());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn locked_identity_is_not_replaced() {
        use std::os::windows::fs::OpenOptionsExt;

        let directory = temporary_directory();
        let store = Store::new(directory.clone()).unwrap();
        let path = directory.join(IDENTITY_FILE);
        let original = [42u8; IDENTITY_SEED_LEN];
        fs::write(&path, original).unwrap();
        let locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        assert!(store.load_or_create_identity().is_err());
        drop(locked);
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn dangling_identity_symlink_is_not_replaced() {
        let directory = temporary_directory();
        let store = Store::new(directory.clone()).unwrap();
        let path = directory.join(IDENTITY_FILE);
        std::os::unix::fs::symlink("missing", &path).unwrap();
        assert!(store.load_or_create_identity().is_err());
        assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn concurrent_first_starts_use_one_identity() {
        let directory = temporary_directory();
        Store::new(directory.clone()).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let directory = directory.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let store = Store::new(directory).unwrap();
                    barrier.wait();
                    store.load_or_create_identity().unwrap().id()
                })
            })
            .collect();
        let ids: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert!(ids.iter().all(|id| *id == ids[0]));
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
