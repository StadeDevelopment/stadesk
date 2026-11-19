use stadesk_codec::CodecError;
use stadesk_core::{AccessResponse, CoreError};
use stadesk_desktop::DesktopError;
use stadesk_net::NetError;
use thiserror::Error;

use crate::message::ByeReason;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error(transparent)]
    Net(#[from] NetError),
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error(transparent)]
    Desktop(#[from] DesktopError),
    #[error("access was not granted: {0:?}")]
    Access(AccessResponse),
    #[error("the peer ended the session: {0:?}")]
    Ended(ByeReason),
    #[error("the peer closed the connection")]
    Closed,
    #[error("the peer did not answer in time")]
    Timeout,
    #[error("unexpected message from the peer")]
    Protocol,
    #[error("an internal task stopped unexpectedly")]
    Internal,
}

impl From<tokio::time::error::Elapsed> for SessionError {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        Self::Timeout
    }
}
