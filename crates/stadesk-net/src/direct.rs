use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use quinn::{Connection, Endpoint, Incoming, RecvStream, SendStream, VarInt};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::error::NetError;
use crate::protocol::{SESSION_TOKEN_LEN, SessionToken};
use crate::tls::{SERVER_NAME, peer_client_config};

const PUNCH_DURATION: Duration = Duration::from_secs(5);
const TOKEN_TIMEOUT: Duration = Duration::from_secs(10);
const UNKNOWN_SESSION: VarInt = VarInt::from_u32(1);

pub struct DirectStreams {
    pub connection: Connection,
    pub send: SendStream,
    pub recv: RecvStream,
}

pub async fn connect_direct(
    endpoint: &Endpoint,
    host: SocketAddr,
    token: SessionToken,
    wait: Duration,
) -> Result<DirectStreams, NetError> {
    timeout(wait, async {
        let connection = endpoint
            .connect_with(peer_client_config()?, host, SERVER_NAME)?
            .await?;
        let (mut send, recv) = connection.open_bi().await?;
        send.write_all(&token).await?;
        Ok(DirectStreams {
            connection,
            send,
            recv,
        })
    })
    .await?
}

pub fn punch(endpoint: &Endpoint, controller: SocketAddr) -> Result<(), NetError> {
    let connecting = endpoint.connect_with(peer_client_config()?, controller, SERVER_NAME)?;
    tokio::spawn(async move {
        if let Ok(Ok(connection)) = timeout(PUNCH_DURATION, connecting).await {
            connection.close(VarInt::from_u32(0), b"punch");
        }
    });
    Ok(())
}

type Waiters = Arc<Mutex<HashMap<SessionToken, oneshot::Sender<DirectStreams>>>>;

pub struct DirectAcceptor {
    waiters: Waiters,
    task: JoinHandle<()>,
}

impl DirectAcceptor {
    pub fn spawn(endpoint: Endpoint) -> Self {
        let waiters: Waiters = Arc::default();
        let loop_waiters = Arc::clone(&waiters);
        let task = tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                tokio::spawn(route_incoming(incoming, Arc::clone(&loop_waiters)));
            }
        });
        Self { waiters, task }
    }

    pub fn expect(&self, token: SessionToken) -> oneshot::Receiver<DirectStreams> {
        let (sender, receiver) = oneshot::channel();
        self.waiters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(token, sender);
        receiver
    }

    pub fn cancel(&self, token: &SessionToken) {
        self.waiters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(token);
    }
}

impl Drop for DirectAcceptor {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn route_incoming(incoming: Incoming, waiters: Waiters) {
    let accepted = timeout(TOKEN_TIMEOUT, async {
        let connection = incoming.await?;
        let (send, mut recv) = connection.accept_bi().await?;
        let mut token = [0u8; SESSION_TOKEN_LEN];
        recv.read_exact(&mut token).await?;
        Ok::<_, NetError>((
            token,
            DirectStreams {
                connection,
                send,
                recv,
            },
        ))
    })
    .await;
    let Ok(Ok((token, streams))) = accepted else {
        return;
    };
    let waiter = waiters
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&token);
    match waiter {
        Some(sender) => {
            if let Err(rejected) = sender.send(streams) {
                rejected
                    .connection
                    .close(UNKNOWN_SESSION, b"session no longer awaited");
            }
        }
        None => streams
            .connection
            .close(UNKNOWN_SESSION, b"unknown session"),
    }
}
