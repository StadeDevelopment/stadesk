use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CodecError {
    #[error("invalid frame dimensions {width}x{height}")]
    InvalidDimensions { width: u32, height: u32 },
    #[error("pixel buffer is too small for the frame")]
    BufferTooSmall,
    #[error(
        "frame is {actual_width}x{actual_height} but the encoder expects {expected_width}x{expected_height}"
    )]
    FrameSizeMismatch {
        expected_width: u32,
        expected_height: u32,
        actual_width: u32,
        actual_height: u32,
    },
    #[error("invalid encoder setting: {0}")]
    InvalidSetting(&'static str),
    #[error("libvpx {operation} failed: {message}")]
    Libvpx {
        operation: &'static str,
        message: String,
    },
    #[error("decoder produced an unsupported image format")]
    UnsupportedImage,
    #[error("compressed frame is empty or too large")]
    InvalidPacket,
}
