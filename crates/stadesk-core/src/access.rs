use std::fmt;
use std::time::{Duration, Instant};

use argon2::{Algorithm, Argon2, Params, Version};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::error::CoreError;
use crate::identity::PublicIdentity;
use crate::primitives::random_array;
use crate::wire::{Reader, Writer};

const ONE_TIME_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const ONE_TIME_PASSWORD_LEN: usize = 8;
const PERMANENT_PASSWORD_MIN_CHARS: usize = 8;
const PASSWORD_MAX_BYTES: usize = 128;
const ARGON2_MEMORY_KIB: u32 = 19_456;
const ARGON2_TIME_COST: u32 = 2;
const ARGON2_PARALLELISM: u32 = 1;
const PERMANENT_FORMAT_VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const DIGEST_LEN: usize = 32;
const PERMANENT_ENCODED_LEN: usize = 1 + SALT_LEN + DIGEST_LEN;

const REQUEST_WITHOUT_PASSWORD: u8 = 0;
const REQUEST_WITH_PASSWORD: u8 = 1;
const RESPONSE_GRANTED: u8 = 0;
const RESPONSE_PENDING_APPROVAL: u8 = 1;
const RESPONSE_PASSWORD_REQUIRED: u8 = 2;
const RESPONSE_WRONG_PASSWORD: u8 = 3;
const RESPONSE_LOCKED_OUT: u8 = 4;
const RESPONSE_DENIED: u8 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessMethods {
    pub accept_prompt: bool,
    pub one_time_password: bool,
    pub permanent_password: bool,
    pub trusted_devices: bool,
}

impl Default for AccessMethods {
    fn default() -> Self {
        Self {
            accept_prompt: true,
            one_time_password: true,
            permanent_password: false,
            trusted_devices: true,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct OneTimePassword(Zeroizing<[u8; ONE_TIME_PASSWORD_LEN]>);

impl OneTimePassword {
    pub fn generate() -> Result<Self, CoreError> {
        let random = random_array::<ONE_TIME_PASSWORD_LEN>()?;
        let mut out = Zeroizing::new([0u8; ONE_TIME_PASSWORD_LEN]);
        for (slot, byte) in out.iter_mut().zip(random.iter()) {
            *slot = ONE_TIME_ALPHABET[usize::from(byte & 0x1f)];
        }
        Ok(Self(out))
    }

    fn matches(&self, input: &str) -> bool {
        let normalized = normalize_one_time_input(input);
        bool::from(normalized.as_slice().ct_eq(self.0.as_slice()))
    }
}

impl fmt::Display for OneTimePassword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = std::str::from_utf8(self.0.as_slice())
            .expect("one-time password only holds ASCII alphabet bytes");
        write!(formatter, "{}-{}", &text[..4], &text[4..])
    }
}

impl fmt::Debug for OneTimePassword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OneTimePassword(..)")
    }
}

fn normalize_one_time_input(input: &str) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(Vec::with_capacity(input.len()));
    for character in input.trim().chars().flat_map(char::to_uppercase) {
        match character {
            'O' => out.push(b'0'),
            'I' | 'L' => out.push(b'1'),
            ' ' | '-' | '_' => {}
            other if other.is_ascii() => out.push(other as u8),
            _ => out.push(0),
        }
    }
    out
}

#[derive(Clone, PartialEq, Eq)]
pub struct PermanentPassword {
    salt: [u8; SALT_LEN],
    digest: [u8; DIGEST_LEN],
}

impl PermanentPassword {
    pub fn from_plaintext(password: &str) -> Result<Self, CoreError> {
        if password.chars().count() < PERMANENT_PASSWORD_MIN_CHARS {
            return Err(CoreError::PasswordTooShort);
        }
        if password.len() > PASSWORD_MAX_BYTES {
            return Err(CoreError::PasswordTooLong);
        }
        let salt = *random_array::<SALT_LEN>()?;
        let digest = derive_digest(password.as_bytes(), &salt);
        Ok(Self {
            salt,
            digest: *digest,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::with_capacity(PERMANENT_ENCODED_LEN);
        writer
            .bytes(&[PERMANENT_FORMAT_VERSION])
            .bytes(&self.salt)
            .bytes(&self.digest);
        writer.finish()
    }

    pub fn decode(input: &[u8]) -> Result<Self, CoreError> {
        let mut reader = Reader::new(input);
        let [version] = reader.array::<1>()?;
        if version != PERMANENT_FORMAT_VERSION {
            return Err(CoreError::Malformed);
        }
        let salt = reader.array::<SALT_LEN>()?;
        let digest = reader.array::<DIGEST_LEN>()?;
        reader.finish()?;
        Ok(Self { salt, digest })
    }

    fn matches(&self, input: &str) -> bool {
        if input.len() > PASSWORD_MAX_BYTES {
            return false;
        }
        let candidate = derive_digest(input.as_bytes(), &self.salt);
        bool::from(candidate.as_slice().ct_eq(self.digest.as_slice()))
    }
}

impl fmt::Debug for PermanentPassword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PermanentPassword(..)")
    }
}

fn derive_digest(password: &[u8], salt: &[u8; SALT_LEN]) -> Zeroizing<[u8; DIGEST_LEN]> {
    let params = Params::new(
        ARGON2_MEMORY_KIB,
        ARGON2_TIME_COST,
        ARGON2_PARALLELISM,
        Some(DIGEST_LEN),
    )
    .expect("StaDesk Argon2id parameters are within the algorithm's limits");
    let mut out = Zeroizing::new([0u8; DIGEST_LEN]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password, salt, out.as_mut_slice())
        .expect("Argon2id with fixed valid parameters and a 16-byte salt cannot fail");
    out
}

#[derive(Clone, PartialEq, Eq)]
pub struct AccessRequest {
    pub password: Option<Zeroizing<String>>,
}

impl AccessRequest {
    pub fn without_password() -> Self {
        Self { password: None }
    }

    pub fn with_password(password: &str) -> Self {
        Self {
            password: Some(Zeroizing::new(password.to_owned())),
        }
    }

    pub fn encode(&self) -> Result<Zeroizing<Vec<u8>>, CoreError> {
        let mut writer = Writer::with_capacity(3 + PASSWORD_MAX_BYTES);
        match &self.password {
            None => {
                writer.bytes(&[REQUEST_WITHOUT_PASSWORD]);
            }
            Some(password) => {
                if password.len() > PASSWORD_MAX_BYTES {
                    return Err(CoreError::PasswordTooLong);
                }
                writer
                    .bytes(&[REQUEST_WITH_PASSWORD])
                    .u16(password.len() as u16)
                    .bytes(password.as_bytes());
            }
        }
        Ok(Zeroizing::new(writer.finish()))
    }

    pub fn decode(input: &[u8]) -> Result<Self, CoreError> {
        let mut reader = Reader::new(input);
        let [tag] = reader.array::<1>()?;
        let password = match tag {
            REQUEST_WITHOUT_PASSWORD => None,
            REQUEST_WITH_PASSWORD => {
                let length = usize::from(reader.u16()?);
                if length > PASSWORD_MAX_BYTES {
                    return Err(CoreError::Malformed);
                }
                let text =
                    std::str::from_utf8(reader.slice(length)?).map_err(|_| CoreError::Malformed)?;
                Some(Zeroizing::new(text.to_owned()))
            }
            _ => return Err(CoreError::Malformed),
        };
        reader.finish()?;
        Ok(Self { password })
    }
}

impl fmt::Debug for AccessRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccessRequest")
            .field("has_password", &self.password.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessResponse {
    Granted,
    PendingApproval,
    PasswordRequired,
    WrongPassword,
    LockedOut { retry_after_seconds: u32 },
    Denied,
}

impl AccessResponse {
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::with_capacity(5);
        match self {
            Self::Granted => writer.bytes(&[RESPONSE_GRANTED]),
            Self::PendingApproval => writer.bytes(&[RESPONSE_PENDING_APPROVAL]),
            Self::PasswordRequired => writer.bytes(&[RESPONSE_PASSWORD_REQUIRED]),
            Self::WrongPassword => writer.bytes(&[RESPONSE_WRONG_PASSWORD]),
            Self::LockedOut {
                retry_after_seconds,
            } => writer
                .bytes(&[RESPONSE_LOCKED_OUT])
                .bytes(&retry_after_seconds.to_be_bytes()),
            Self::Denied => writer.bytes(&[RESPONSE_DENIED]),
        };
        writer.finish()
    }

    pub fn decode(input: &[u8]) -> Result<Self, CoreError> {
        let mut reader = Reader::new(input);
        let [tag] = reader.array::<1>()?;
        let response = match tag {
            RESPONSE_GRANTED => Self::Granted,
            RESPONSE_PENDING_APPROVAL => Self::PendingApproval,
            RESPONSE_PASSWORD_REQUIRED => Self::PasswordRequired,
            RESPONSE_WRONG_PASSWORD => Self::WrongPassword,
            RESPONSE_LOCKED_OUT => Self::LockedOut {
                retry_after_seconds: u32::from_be_bytes(reader.array::<4>()?),
            },
            RESPONSE_DENIED => Self::Denied,
            _ => return Err(CoreError::Malformed),
        };
        reader.finish()?;
        Ok(response)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantReason {
    TrustedDevice,
    OneTimePassword,
    PermanentPassword,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessDecision {
    Granted(GrantReason),
    AwaitPrompt,
    PasswordRequired,
    WrongPassword,
    LockedOut { retry_after: Duration },
    Denied,
}

impl AccessDecision {
    pub fn response(&self) -> AccessResponse {
        match self {
            Self::Granted(_) => AccessResponse::Granted,
            Self::AwaitPrompt => AccessResponse::PendingApproval,
            Self::PasswordRequired => AccessResponse::PasswordRequired,
            Self::WrongPassword => AccessResponse::WrongPassword,
            Self::LockedOut { retry_after } => AccessResponse::LockedOut {
                retry_after_seconds: u32::try_from(retry_after.as_secs().max(1))
                    .unwrap_or(u32::MAX),
            },
            Self::Denied => AccessResponse::Denied,
        }
    }
}

pub struct AccessGate {
    methods: AccessMethods,
    one_time_password: OneTimePassword,
    permanent_password: Option<PermanentPassword>,
    trusted_devices: Vec<PublicIdentity>,
    failed_attempts: u32,
    locked_until: Option<Instant>,
}

impl AccessGate {
    pub fn new(methods: AccessMethods) -> Result<Self, CoreError> {
        Ok(Self {
            methods,
            one_time_password: OneTimePassword::generate()?,
            permanent_password: None,
            trusted_devices: Vec::new(),
            failed_attempts: 0,
            locked_until: None,
        })
    }

    pub fn methods(&self) -> AccessMethods {
        self.methods
    }

    pub fn set_methods(&mut self, methods: AccessMethods) {
        self.methods = methods;
    }

    pub fn one_time_password(&self) -> &OneTimePassword {
        &self.one_time_password
    }

    pub fn rotate_one_time_password(&mut self) -> Result<(), CoreError> {
        self.one_time_password = OneTimePassword::generate()?;
        Ok(())
    }

    pub fn permanent_password(&self) -> Option<&PermanentPassword> {
        self.permanent_password.as_ref()
    }

    pub fn set_permanent_password(&mut self, password: Option<PermanentPassword>) {
        self.permanent_password = password;
    }

    pub fn trusted_devices(&self) -> &[PublicIdentity] {
        &self.trusted_devices
    }

    pub fn trust_device(&mut self, device: PublicIdentity) {
        if !self.trusted_devices.contains(&device) {
            self.trusted_devices.push(device);
        }
    }

    pub fn forget_device(&mut self, device: &PublicIdentity) {
        self.trusted_devices.retain(|known| known != device);
    }

    pub fn reset_lockout(&mut self) {
        self.failed_attempts = 0;
        self.locked_until = None;
    }

    pub fn evaluate(
        &mut self,
        peer: &PublicIdentity,
        request: &AccessRequest,
        now: Instant,
    ) -> Result<AccessDecision, CoreError> {
        if self.methods.trusted_devices && self.trusted_devices.contains(peer) {
            return Ok(AccessDecision::Granted(GrantReason::TrustedDevice));
        }
        let password_methods_enabled =
            self.methods.one_time_password || self.methods.permanent_password;
        if let (Some(password), true) = (&request.password, password_methods_enabled) {
            return self.evaluate_password(password, now);
        }
        if self.methods.accept_prompt {
            Ok(AccessDecision::AwaitPrompt)
        } else if password_methods_enabled {
            Ok(AccessDecision::PasswordRequired)
        } else {
            Ok(AccessDecision::Denied)
        }
    }

    fn evaluate_password(
        &mut self,
        password: &str,
        now: Instant,
    ) -> Result<AccessDecision, CoreError> {
        if let Some(until) = self.locked_until
            && until > now
        {
            return Ok(AccessDecision::LockedOut {
                retry_after: until - now,
            });
        }
        let one_time_ok =
            self.methods.one_time_password && self.one_time_password.matches(password);
        let permanent_ok = self.methods.permanent_password
            && self
                .permanent_password
                .as_ref()
                .is_some_and(|stored| stored.matches(password));
        if one_time_ok {
            self.reset_lockout();
            self.rotate_one_time_password()?;
            return Ok(AccessDecision::Granted(GrantReason::OneTimePassword));
        }
        if permanent_ok {
            self.reset_lockout();
            return Ok(AccessDecision::Granted(GrantReason::PermanentPassword));
        }
        self.failed_attempts = self.failed_attempts.saturating_add(1);
        let delay = lockout_delay(self.failed_attempts);
        self.locked_until = (!delay.is_zero()).then(|| now + delay);
        Ok(AccessDecision::WrongPassword)
    }
}

fn lockout_delay(failed_attempts: u32) -> Duration {
    let seconds = match failed_attempts {
        0..=2 => 0,
        3 => 5,
        4 => 15,
        5 => 30,
        6 => 60,
        7 => 5 * 60,
        8 => 15 * 60,
        9 => 60 * 60,
        _ => 24 * 60 * 60,
    };
    Duration::from_secs(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    fn gate(methods: AccessMethods) -> AccessGate {
        AccessGate::new(methods).unwrap()
    }

    fn peer() -> PublicIdentity {
        Identity::generate().unwrap().public().clone()
    }

    fn only(configure: impl FnOnce(&mut AccessMethods)) -> AccessMethods {
        let mut methods = AccessMethods {
            accept_prompt: false,
            one_time_password: false,
            permanent_password: false,
            trusted_devices: false,
        };
        configure(&mut methods);
        methods
    }

    #[test]
    fn one_time_password_has_display_shape_and_accepts_typing_variations() {
        let password = OneTimePassword::generate().unwrap();
        let shown = password.to_string();
        assert_eq!(shown.len(), ONE_TIME_PASSWORD_LEN + 1);
        assert!(password.matches(&shown));
        assert!(password.matches(&shown.to_lowercase().replace('-', " ")));
        assert!(!password.matches(&shown[..shown.len() - 1]));
        assert!(!password.matches(""));
    }

    #[test]
    fn one_time_password_treats_non_ascii_as_mismatch() {
        let password = OneTimePassword::generate().unwrap();
        let mut shown = password.to_string();
        shown.pop();
        shown.push('ç');
        assert!(!password.matches(&shown));
    }

    #[test]
    fn generated_one_time_passwords_differ() {
        let first = OneTimePassword::generate().unwrap();
        let second = OneTimePassword::generate().unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn permanent_password_verifies_and_round_trips_through_encoding() {
        let stored = PermanentPassword::from_plaintext("correct horse").unwrap();
        let restored = PermanentPassword::decode(&stored.encode()).unwrap();
        assert_eq!(restored, stored);
        assert!(restored.matches("correct horse"));
        assert!(!restored.matches("correct horsE"));
    }

    #[test]
    fn permanent_password_uses_fresh_salt_each_time() {
        let first = PermanentPassword::from_plaintext("correct horse").unwrap();
        let second = PermanentPassword::from_plaintext("correct horse").unwrap();
        assert_ne!(first.encode(), second.encode());
    }

    #[test]
    fn permanent_password_enforces_length_limits() {
        assert_eq!(
            PermanentPassword::from_plaintext("1234567"),
            Err(CoreError::PasswordTooShort)
        );
        assert!(PermanentPassword::from_plaintext("şifreşifre").is_ok());
        assert_eq!(
            PermanentPassword::from_plaintext(&"a".repeat(PASSWORD_MAX_BYTES + 1)),
            Err(CoreError::PasswordTooLong)
        );
    }

    #[test]
    fn permanent_password_decode_rejects_bad_version_and_length() {
        let mut encoded = PermanentPassword::from_plaintext("correct horse")
            .unwrap()
            .encode();
        assert!(PermanentPassword::decode(&encoded[1..]).is_err());
        encoded[0] = 2;
        assert_eq!(
            PermanentPassword::decode(&encoded),
            Err(CoreError::Malformed)
        );
    }

    #[test]
    fn requests_and_responses_round_trip() {
        for request in [
            AccessRequest::without_password(),
            AccessRequest::with_password("şifre-1234"),
        ] {
            let encoded = request.encode().unwrap();
            assert_eq!(AccessRequest::decode(&encoded).unwrap(), request);
        }
        for response in [
            AccessResponse::Granted,
            AccessResponse::PendingApproval,
            AccessResponse::PasswordRequired,
            AccessResponse::WrongPassword,
            AccessResponse::LockedOut {
                retry_after_seconds: 900,
            },
            AccessResponse::Denied,
        ] {
            assert_eq!(
                AccessResponse::decode(&response.encode()).unwrap(),
                response
            );
        }
    }

    #[test]
    fn malformed_requests_and_responses_are_rejected() {
        assert_eq!(AccessRequest::decode(&[]), Err(CoreError::Malformed));
        assert_eq!(AccessRequest::decode(&[9]), Err(CoreError::Malformed));
        assert_eq!(AccessRequest::decode(&[0, 0]), Err(CoreError::Malformed));
        assert_eq!(
            AccessRequest::decode(&[1, 0, 2, 0xff, 0xfe]),
            Err(CoreError::Malformed)
        );
        assert_eq!(
            AccessRequest::decode(&[1, 0, 200]),
            Err(CoreError::Malformed)
        );
        assert_eq!(
            AccessRequest::with_password(&"a".repeat(PASSWORD_MAX_BYTES + 1)).encode(),
            Err(CoreError::PasswordTooLong)
        );
        assert_eq!(
            AccessResponse::decode(&[4, 0, 0]),
            Err(CoreError::Malformed)
        );
        assert_eq!(AccessResponse::decode(&[6]), Err(CoreError::Malformed));
        assert_eq!(AccessResponse::decode(&[0, 0]), Err(CoreError::Malformed));
    }

    #[test]
    fn trusted_device_is_granted_without_password_and_only_while_enabled() {
        let device = peer();
        let mut access = gate(only(|m| m.trusted_devices = true));
        access.trust_device(device.clone());
        access.trust_device(device.clone());
        assert_eq!(access.trusted_devices().len(), 1);
        let now = Instant::now();
        let request = AccessRequest::without_password();
        assert_eq!(
            access.evaluate(&device, &request, now).unwrap(),
            AccessDecision::Granted(GrantReason::TrustedDevice)
        );
        assert_eq!(
            access.evaluate(&peer(), &request, now).unwrap(),
            AccessDecision::Denied
        );
        access.set_methods(only(|_| {}));
        assert_eq!(
            access.evaluate(&device, &request, now).unwrap(),
            AccessDecision::Denied
        );
        access.set_methods(only(|m| m.trusted_devices = true));
        access.forget_device(&device);
        assert_eq!(
            access.evaluate(&device, &request, now).unwrap(),
            AccessDecision::Denied
        );
    }

    #[test]
    fn one_time_password_grants_once_then_rotates() {
        let mut access = gate(only(|m| m.one_time_password = true));
        let shown = access.one_time_password().to_string();
        let request = AccessRequest::with_password(&shown);
        let now = Instant::now();
        assert_eq!(
            access.evaluate(&peer(), &request, now).unwrap(),
            AccessDecision::Granted(GrantReason::OneTimePassword)
        );
        assert_ne!(access.one_time_password().to_string(), shown);
        assert_eq!(
            access.evaluate(&peer(), &request, now).unwrap(),
            AccessDecision::WrongPassword
        );
    }

    #[test]
    fn permanent_password_grants_repeatedly_only_when_enabled() {
        let mut access = gate(only(|m| m.permanent_password = true));
        access.set_permanent_password(Some(
            PermanentPassword::from_plaintext("correct horse").unwrap(),
        ));
        let request = AccessRequest::with_password("correct horse");
        let now = Instant::now();
        for _ in 0..2 {
            assert_eq!(
                access.evaluate(&peer(), &request, now).unwrap(),
                AccessDecision::Granted(GrantReason::PermanentPassword)
            );
        }
        access.set_methods(only(|m| m.one_time_password = true));
        assert_eq!(
            access.evaluate(&peer(), &request, now).unwrap(),
            AccessDecision::WrongPassword
        );
    }

    #[test]
    fn disabled_one_time_password_is_not_accepted() {
        let mut access = gate(only(|m| m.permanent_password = true));
        let shown = access.one_time_password().to_string();
        assert_eq!(
            access
                .evaluate(
                    &peer(),
                    &AccessRequest::with_password(&shown),
                    Instant::now()
                )
                .unwrap(),
            AccessDecision::WrongPassword
        );
    }

    #[test]
    fn request_without_password_follows_enabled_methods() {
        let request = AccessRequest::without_password();
        let now = Instant::now();
        let mut prompt_and_password = gate(AccessMethods::default());
        assert_eq!(
            prompt_and_password
                .evaluate(&peer(), &request, now)
                .unwrap(),
            AccessDecision::AwaitPrompt
        );
        let mut password_only = gate(only(|m| m.one_time_password = true));
        assert_eq!(
            password_only.evaluate(&peer(), &request, now).unwrap(),
            AccessDecision::PasswordRequired
        );
        let mut nothing = gate(only(|_| {}));
        assert_eq!(
            nothing.evaluate(&peer(), &request, now).unwrap(),
            AccessDecision::Denied
        );
    }

    #[test]
    fn password_sent_while_password_methods_are_off_falls_back_to_prompt() {
        let mut access = gate(only(|m| m.accept_prompt = true));
        assert_eq!(
            access
                .evaluate(&peer(), &AccessRequest::with_password("x"), Instant::now())
                .unwrap(),
            AccessDecision::AwaitPrompt
        );
    }

    #[test]
    fn repeated_wrong_passwords_lock_out_every_peer_and_lock_expires() {
        let mut access = gate(only(|m| m.one_time_password = true));
        let wrong = AccessRequest::with_password("WRONG-PASS");
        let start = Instant::now();
        for _ in 0..3 {
            assert_eq!(
                access.evaluate(&peer(), &wrong, start).unwrap(),
                AccessDecision::WrongPassword
            );
        }
        let correct = AccessRequest::with_password(&access.one_time_password().to_string());
        assert_eq!(
            access.evaluate(&peer(), &correct, start).unwrap(),
            AccessDecision::LockedOut {
                retry_after: Duration::from_secs(5)
            }
        );
        assert_eq!(
            access
                .evaluate(&peer(), &correct, start + Duration::from_secs(5))
                .unwrap(),
            AccessDecision::Granted(GrantReason::OneTimePassword)
        );
    }

    #[test]
    fn lockout_does_not_block_trusted_devices_or_prompt() {
        let mut access = gate(AccessMethods::default());
        let device = peer();
        access.trust_device(device.clone());
        let start = Instant::now();
        for _ in 0..10 {
            access
                .evaluate(&peer(), &AccessRequest::with_password("WRONG-PASS"), start)
                .unwrap();
        }
        assert_eq!(
            access
                .evaluate(&device, &AccessRequest::with_password("x"), start)
                .unwrap(),
            AccessDecision::Granted(GrantReason::TrustedDevice)
        );
        assert_eq!(
            access
                .evaluate(&peer(), &AccessRequest::without_password(), start)
                .unwrap(),
            AccessDecision::AwaitPrompt
        );
    }

    #[test]
    fn reset_lockout_clears_failures() {
        let mut access = gate(only(|m| m.one_time_password = true));
        let start = Instant::now();
        for _ in 0..5 {
            access
                .evaluate(&peer(), &AccessRequest::with_password("WRONG-PASS"), start)
                .unwrap();
        }
        access.reset_lockout();
        let correct = AccessRequest::with_password(&access.one_time_password().to_string());
        assert_eq!(
            access.evaluate(&peer(), &correct, start).unwrap(),
            AccessDecision::Granted(GrantReason::OneTimePassword)
        );
    }

    #[test]
    fn lockout_schedule_matches_stade() {
        let expected = [0, 0, 0, 5, 15, 30, 60, 300, 900, 3600, 86_400, 86_400];
        for (attempts, seconds) in expected.iter().enumerate() {
            assert_eq!(
                lockout_delay(attempts as u32),
                Duration::from_secs(*seconds)
            );
        }
    }

    #[test]
    fn locked_out_response_never_reports_zero_seconds() {
        let decision = AccessDecision::LockedOut {
            retry_after: Duration::from_millis(300),
        };
        assert_eq!(
            decision.response(),
            AccessResponse::LockedOut {
                retry_after_seconds: 1
            }
        );
    }
}
