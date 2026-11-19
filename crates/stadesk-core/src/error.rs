use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CoreError {
    #[error("system random number generator failed")]
    Randomness,
    #[error("malformed message")]
    Malformed,
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("invalid public key")]
    InvalidKey,
    #[error("signature verification failed")]
    BadSignature,
    #[error("peer identity does not match the expected StaDesk ID")]
    IdentityMismatch,
    #[error("authenticated decryption failed")]
    Decryption,
    #[error("frame was replayed or is too old")]
    Replay,
    #[error("session counter exhausted, a new handshake is required")]
    CounterExhausted,
    #[error("invalid StaDesk ID")]
    InvalidId,
    #[error("password is too short")]
    PasswordTooShort,
    #[error("password is too long")]
    PasswordTooLong,
}
