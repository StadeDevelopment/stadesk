use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    ScreenRecording,
    InputControl,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DesktopError {
    #[error("permission has not been granted: {0:?}")]
    PermissionDenied(Permission),
    #[error("not supported here: {0}")]
    Unsupported(&'static str),
    #[error("key {0:#06x} cannot be produced on this platform")]
    UnmappedKey(u16),
    #[error("display {0} was not found")]
    DisplayNotFound(u32),
    #[error("invalid capture option: {0}")]
    InvalidOption(&'static str),
    #[error("the system did not answer in time")]
    Timeout,
    #[error("capture stopped: {0}")]
    Stopped(String),
    #[error("platform capture error {code}: {message}")]
    Platform { code: i64, message: String },
}
