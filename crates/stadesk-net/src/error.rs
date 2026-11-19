use stadesk_core::CoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NetError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Tls(#[from] quinn::rustls::Error),
    #[error(transparent)]
    Certificate(#[from] rcgen::Error),
    #[error("no QUIC-compatible TLS cipher suite is available")]
    NoQuicCipherSuite,
    #[error(transparent)]
    Connect(#[from] quinn::ConnectError),
    #[error(transparent)]
    Connection(#[from] quinn::ConnectionError),
    #[error(transparent)]
    Write(#[from] quinn::WriteError),
    #[error(transparent)]
    Read(#[from] quinn::ReadExactError),
    #[error(transparent)]
    Stopped(#[from] quinn::StoppedError),
    #[error("stream already closed")]
    StreamClosed,
    #[error("frame of {0} bytes exceeds the allowed size")]
    FrameTooLarge(usize),
    #[error("operation timed out")]
    Timeout,
    #[error("unexpected message")]
    UnexpectedMessage,
    #[error("the requested device is offline")]
    PeerOffline,
    #[error("the server rejected the registration")]
    RegistrationRejected,
    #[error("too many requests, try again later")]
    RateLimited,
    #[error("invalid server key")]
    InvalidServerKey,
    #[error("the relay rejected the session")]
    RelayRejected,
    #[error("system random number generator failed")]
    Randomness,
}

impl From<quinn::crypto::rustls::NoInitialCipherSuite> for NetError {
    fn from(_: quinn::crypto::rustls::NoInitialCipherSuite) -> Self {
        Self::NoQuicCipherSuite
    }
}

impl From<quinn::ClosedStream> for NetError {
    fn from(_: quinn::ClosedStream) -> Self {
        Self::StreamClosed
    }
}

impl From<tokio::time::error::Elapsed> for NetError {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        Self::Timeout
    }
}
