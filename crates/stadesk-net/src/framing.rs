use quinn::{RecvStream, SendStream};

use crate::error::NetError;

pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;
const LENGTH_PREFIX_LEN: usize = 4;

pub async fn write_frame(send: &mut SendStream, payload: &[u8]) -> Result<(), NetError> {
    if payload.len() > MAX_FRAME_LEN {
        return Err(NetError::FrameTooLarge(payload.len()));
    }
    let length =
        u32::try_from(payload.len()).map_err(|_| NetError::FrameTooLarge(payload.len()))?;
    let mut buffer = Vec::with_capacity(LENGTH_PREFIX_LEN + payload.len());
    buffer.extend_from_slice(&length.to_be_bytes());
    buffer.extend_from_slice(payload);
    send.write_all(&buffer).await?;
    Ok(())
}

pub async fn read_frame(recv: &mut RecvStream, limit: usize) -> Result<Vec<u8>, NetError> {
    read_frame_or_end(recv, limit)
        .await?
        .ok_or(NetError::Read(quinn::ReadExactError::FinishedEarly(0)))
}

pub async fn read_frame_or_end(
    recv: &mut RecvStream,
    limit: usize,
) -> Result<Option<Vec<u8>>, NetError> {
    let mut header = [0u8; LENGTH_PREFIX_LEN];
    match recv.read_exact(&mut header).await {
        Ok(()) => {}
        Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let length = u32::from_be_bytes(header) as usize;
    if length > limit.min(MAX_FRAME_LEN) {
        return Err(NetError::FrameTooLarge(length));
    }
    let mut payload = vec![0u8; length];
    recv.read_exact(&mut payload).await?;
    Ok(Some(payload))
}
