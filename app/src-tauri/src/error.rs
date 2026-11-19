use serde::Serialize;
use stadesk_core::{AccessResponse, CoreError};
use stadesk_net::NetError;
use stadesk_session::SessionError;

use crate::state::ServerProblem;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiError {
    pub code: String,
    pub detail: Option<String>,
}

impl UiError {
    pub fn new(code: &str) -> Self {
        Self {
            code: code.to_owned(),
            detail: None,
        }
    }

    pub fn with_detail(code: &str, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            detail: Some(detail.into()),
        }
    }
}

impl From<ServerProblem> for UiError {
    fn from(problem: ServerProblem) -> Self {
        match problem {
            ServerProblem::NotConfigured => Self::new("serverNotConfigured"),
            ServerProblem::InvalidAddress => Self::new("invalidServerAddress"),
            ServerProblem::InvalidKey => Self::new("invalidServerKey"),
            ServerProblem::Unresolved(detail) => Self::with_detail("serverUnresolved", detail),
        }
    }
}

impl From<NetError> for UiError {
    fn from(error: NetError) -> Self {
        match error {
            NetError::PeerOffline => Self::new("peerOffline"),
            NetError::RateLimited => Self::new("rateLimited"),
            NetError::Timeout => Self::new("timeout"),
            NetError::Core(CoreError::IdentityMismatch) => Self::new("identityMismatch"),
            NetError::Core(other) => Self::with_detail("protocol", other.to_string()),
            other => Self::with_detail("network", other.to_string()),
        }
    }
}

impl From<SessionError> for UiError {
    fn from(error: SessionError) -> Self {
        match error {
            SessionError::Net(error) => error.into(),
            SessionError::Access(AccessResponse::WrongPassword) => Self::new("wrongPassword"),
            SessionError::Access(AccessResponse::PasswordRequired) => Self::new("passwordRequired"),
            SessionError::Access(AccessResponse::LockedOut {
                retry_after_seconds,
            }) => Self::with_detail("lockedOut", retry_after_seconds.to_string()),
            SessionError::Access(_) => Self::new("denied"),
            SessionError::Ended(reason) => Self::with_detail("ended", format!("{reason:?}")),
            SessionError::Timeout => Self::new("timeout"),
            SessionError::Closed => Self::new("closed"),
            SessionError::Core(CoreError::IdentityMismatch) => Self::new("identityMismatch"),
            other => Self::with_detail("unknown", other.to_string()),
        }
    }
}

impl From<std::io::Error> for UiError {
    fn from(error: std::io::Error) -> Self {
        Self::with_detail("storage", error.to_string())
    }
}
