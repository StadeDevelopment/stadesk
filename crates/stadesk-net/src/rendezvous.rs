use std::net::SocketAddr;
use std::time::Duration;

use quinn::{Connection, Endpoint, RecvStream, SendStream};
use stadesk_core::{Identity, StaDeskId, prove_ownership};
use tokio::time::timeout;

use crate::error::NetError;
use crate::framing::{read_frame, write_frame};
use crate::protocol::{
    ClientMessage, MAX_CONTROL_FRAME_LEN, RelayRole, ServerMessage, SessionToken,
};
use crate::tls::{
    SERVER_NAME, ServerCertificate, ServerFingerprint, rendezvous_client_config, server_config,
};

const SERVER_TIMEOUT: Duration = Duration::from_secs(10);
const RELAY_PAIRING_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerAddress {
    pub address: SocketAddr,
    pub fingerprint: ServerFingerprint,
}

pub fn bind_host_endpoint(local: SocketAddr) -> Result<Endpoint, NetError> {
    let certificate = ServerCertificate::generate()?;
    Ok(Endpoint::server(server_config(&certificate)?, local)?)
}

pub fn bind_controller_endpoint(local: SocketAddr) -> Result<Endpoint, NetError> {
    Ok(Endpoint::client(local)?)
}

pub async fn connect_server(
    endpoint: &Endpoint,
    server: &ServerAddress,
) -> Result<Connection, NetError> {
    let connecting = endpoint.connect_with(
        rendezvous_client_config(server.fingerprint)?,
        server.address,
        SERVER_NAME,
    )?;
    Ok(timeout(SERVER_TIMEOUT, connecting).await??)
}

pub(crate) async fn send_client_message(
    send: &mut SendStream,
    message: &ClientMessage,
) -> Result<(), NetError> {
    write_frame(send, &message.encode()).await
}

pub(crate) async fn receive_server_message(
    recv: &mut RecvStream,
) -> Result<ServerMessage, NetError> {
    Ok(ServerMessage::decode(
        &read_frame(recv, MAX_CONTROL_FRAME_LEN).await?,
    )?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IncomingRequest {
    pub token: SessionToken,
    pub controller: SocketAddr,
}

pub struct HostRegistration {
    connection: Connection,
    _send: SendStream,
    recv: RecvStream,
    observed: SocketAddr,
}

impl HostRegistration {
    pub async fn register(
        endpoint: &Endpoint,
        server: &ServerAddress,
        identity: &Identity,
    ) -> Result<Self, NetError> {
        let connection = connect_server(endpoint, server).await?;
        let (send, recv, observed) = timeout(SERVER_TIMEOUT, async {
            let (mut send, mut recv) = connection.open_bi().await?;
            send_client_message(
                &mut send,
                &ClientMessage::Register {
                    identity: Box::new(identity.public().clone()),
                },
            )
            .await?;
            let ServerMessage::Challenge { challenge } = receive_server_message(&mut recv).await?
            else {
                return Err(NetError::UnexpectedMessage);
            };
            let signature = prove_ownership(identity, server.fingerprint.as_bytes(), &challenge)?;
            send_client_message(&mut send, &ClientMessage::Proof { signature }).await?;
            match receive_server_message(&mut recv).await? {
                ServerMessage::Registered { observed } => Ok((send, recv, observed)),
                ServerMessage::RegistrationRejected => Err(NetError::RegistrationRejected),
                _ => Err(NetError::UnexpectedMessage),
            }
        })
        .await??;
        Ok(Self {
            connection,
            _send: send,
            recv,
            observed,
        })
    }

    pub fn observed_address(&self) -> SocketAddr {
        self.observed
    }

    pub fn server_connection(&self) -> &Connection {
        &self.connection
    }

    pub async fn next_request(&mut self) -> Result<IncomingRequest, NetError> {
        match receive_server_message(&mut self.recv).await? {
            ServerMessage::IncomingRequest { token, controller } => {
                Ok(IncomingRequest { token, controller })
            }
            _ => Err(NetError::UnexpectedMessage),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectOffer {
    pub token: SessionToken,
    pub host: SocketAddr,
}

pub async fn request_connection(
    endpoint: &Endpoint,
    server: &ServerAddress,
    target: StaDeskId,
) -> Result<(Connection, ConnectOffer), NetError> {
    let connection = connect_server(endpoint, server).await?;
    let offer = timeout(SERVER_TIMEOUT, async {
        let (mut send, mut recv) = connection.open_bi().await?;
        send_client_message(&mut send, &ClientMessage::Connect { target }).await?;
        let reply = receive_server_message(&mut recv).await?;
        send.finish()?;
        match reply {
            ServerMessage::PeerFound { token, host } => Ok(ConnectOffer { token, host }),
            ServerMessage::PeerOffline => Err(NetError::PeerOffline),
            ServerMessage::RateLimited => Err(NetError::RateLimited),
            _ => Err(NetError::UnexpectedMessage),
        }
    })
    .await??;
    Ok((connection, offer))
}

pub async fn join_relay(
    server_connection: &Connection,
    token: SessionToken,
    role: RelayRole,
) -> Result<(SendStream, RecvStream), NetError> {
    timeout(RELAY_PAIRING_TIMEOUT, async {
        let (mut send, mut recv) = server_connection.open_bi().await?;
        send_client_message(&mut send, &ClientMessage::RelayJoin { token, role }).await?;
        match receive_server_message(&mut recv).await? {
            ServerMessage::RelayReady => Ok((send, recv)),
            ServerMessage::RelayRejected => Err(NetError::RelayRejected),
            _ => Err(NetError::UnexpectedMessage),
        }
    })
    .await?
}
