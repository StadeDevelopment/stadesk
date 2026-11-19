use std::time::Duration;

use quinn::{RecvStream, SendStream};
use stadesk_core::{
    FRAME_OVERHEAD, HandshakeOutcome, Identity, Initiator, PublicIdentity, Responder,
    SessionCipher, SessionOpener, SessionSealer, StaDeskId,
};
use tokio::time::timeout;
use zeroize::Zeroizing;

use crate::error::NetError;
use crate::framing::{MAX_FRAME_LEN, read_frame, read_frame_or_end, write_frame};

const HANDSHAKE_FRAME_LIMIT: usize = 16 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
pub const MAX_PLAINTEXT_LEN: usize = MAX_FRAME_LEN - FRAME_OVERHEAD;

pub struct SecureChannel {
    send: SendStream,
    recv: RecvStream,
    cipher: SessionCipher,
    peer: PublicIdentity,
    channel_binding: [u8; 32],
}

impl SecureChannel {
    pub async fn initiate(
        mut send: SendStream,
        mut recv: RecvStream,
        identity: &Identity,
        expected_peer: StaDeskId,
    ) -> Result<Self, NetError> {
        let outcome = timeout(HANDSHAKE_TIMEOUT, async {
            let (waiting, hello) = Initiator::start(identity, expected_peer)?;
            write_frame(&mut send, &hello).await?;
            let reply = read_frame(&mut recv, HANDSHAKE_FRAME_LIMIT).await?;
            let (outcome, finish) = waiting.finish(&reply)?;
            write_frame(&mut send, &finish).await?;
            Ok::<HandshakeOutcome, NetError>(outcome)
        })
        .await??;
        Ok(Self::from_outcome(send, recv, outcome))
    }

    pub async fn respond(
        mut send: SendStream,
        mut recv: RecvStream,
        identity: &Identity,
    ) -> Result<Self, NetError> {
        let outcome = timeout(HANDSHAKE_TIMEOUT, async {
            let hello = read_frame(&mut recv, HANDSHAKE_FRAME_LIMIT).await?;
            let (waiting, reply) = Responder::respond(identity, &hello)?;
            write_frame(&mut send, &reply).await?;
            let finish = read_frame(&mut recv, HANDSHAKE_FRAME_LIMIT).await?;
            Ok::<HandshakeOutcome, NetError>(waiting.finish(&finish)?)
        })
        .await??;
        Ok(Self::from_outcome(send, recv, outcome))
    }

    fn from_outcome(send: SendStream, recv: RecvStream, outcome: HandshakeOutcome) -> Self {
        Self {
            send,
            recv,
            cipher: outcome.cipher,
            peer: outcome.peer,
            channel_binding: outcome.channel_binding,
        }
    }

    pub fn peer(&self) -> &PublicIdentity {
        &self.peer
    }

    pub fn channel_binding(&self) -> &[u8; 32] {
        &self.channel_binding
    }

    pub async fn send(&mut self, plaintext: &[u8]) -> Result<(), NetError> {
        if plaintext.len() > MAX_PLAINTEXT_LEN {
            return Err(NetError::FrameTooLarge(plaintext.len()));
        }
        let frame = self.cipher.seal(plaintext)?;
        write_frame(&mut self.send, &frame).await
    }

    pub async fn receive(&mut self) -> Result<Option<Zeroizing<Vec<u8>>>, NetError> {
        match read_frame_or_end(&mut self.recv, MAX_FRAME_LEN).await? {
            Some(frame) => Ok(Some(self.cipher.open(&frame)?)),
            None => Ok(None),
        }
    }

    pub async fn close(self) -> Result<(), NetError> {
        finish_stream(self.send).await
    }

    pub fn split(self) -> (SecureSender, SecureReceiver) {
        let (sealer, opener) = self.cipher.split();
        (
            SecureSender {
                send: self.send,
                sealer,
            },
            SecureReceiver {
                recv: self.recv,
                opener,
            },
        )
    }
}

async fn finish_stream(mut send: SendStream) -> Result<(), NetError> {
    send.finish()?;
    match send.stopped().await {
        Ok(_) | Err(quinn::StoppedError::ConnectionLost(_)) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub struct SecureSender {
    send: SendStream,
    sealer: SessionSealer,
}

impl SecureSender {
    pub async fn send(&mut self, plaintext: &[u8]) -> Result<(), NetError> {
        if plaintext.len() > MAX_PLAINTEXT_LEN {
            return Err(NetError::FrameTooLarge(plaintext.len()));
        }
        let frame = self.sealer.seal(plaintext)?;
        write_frame(&mut self.send, &frame).await
    }

    pub async fn close(self) -> Result<(), NetError> {
        finish_stream(self.send).await
    }
}

pub struct SecureReceiver {
    recv: RecvStream,
    opener: SessionOpener,
}

impl SecureReceiver {
    pub async fn receive(&mut self) -> Result<Option<Zeroizing<Vec<u8>>>, NetError> {
        match read_frame_or_end(&mut self.recv, MAX_FRAME_LEN).await? {
            Some(frame) => Ok(Some(self.opener.open(&frame)?)),
            None => Ok(None),
        }
    }
}
