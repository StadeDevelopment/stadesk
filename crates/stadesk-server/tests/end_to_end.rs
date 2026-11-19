use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use stadesk_core::{
    AccessGate, AccessMethods, AccessRequest, AccessResponse, Identity, PermanentPassword,
    StaDeskId,
};
use stadesk_net::{
    DirectAcceptor, HostRegistration, NetError, RelayRole, SecureChannel, ServerAddress,
    ServerCertificate, bind_controller_endpoint, bind_host_endpoint, connect_direct, join_relay,
    punch, request_connection,
};
use stadesk_server::RunningServer;
use tokio::time::timeout;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const PASSWORD: &str = "correct horse";
const STEP_TIMEOUT: Duration = Duration::from_secs(20);

fn localhost() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

fn start_server() -> TestResult<(RunningServer, ServerAddress)> {
    let certificate = ServerCertificate::generate()?;
    let server = RunningServer::start(localhost(), &certificate)?;
    let address = ServerAddress {
        address: server.local_address(),
        fingerprint: server.fingerprint(),
    };
    Ok((server, address))
}

fn host_gate() -> TestResult<AccessGate> {
    let mut gate = AccessGate::new(AccessMethods {
        accept_prompt: false,
        one_time_password: true,
        permanent_password: true,
        trusted_devices: false,
    })?;
    gate.set_permanent_password(Some(PermanentPassword::from_plaintext(PASSWORD)?));
    Ok(gate)
}

async fn receive_some(channel: &mut SecureChannel) -> TestResult<Vec<u8>> {
    Ok(channel
        .receive()
        .await?
        .ok_or("stream ended early")?
        .to_vec())
}

async fn host_session(mut channel: SecureChannel) -> TestResult<StaDeskId> {
    let mut gate = host_gate()?;
    let request = AccessRequest::decode(&receive_some(&mut channel).await?)?;
    let decision = gate.evaluate(channel.peer(), &request, Instant::now())?;
    channel.send(&decision.response().encode()).await?;
    let echoed = receive_some(&mut channel).await?;
    channel.send(&echoed).await?;
    assert!(channel.receive().await?.is_none());
    let peer = channel.peer().id();
    channel.close().await?;
    Ok(peer)
}

async fn controller_session(mut channel: SecureChannel) -> TestResult {
    channel
        .send(&AccessRequest::with_password(PASSWORD).encode()?)
        .await?;
    let response = AccessResponse::decode(&receive_some(&mut channel).await?)?;
    assert_eq!(response, AccessResponse::Granted);
    let payload: Vec<u8> = (0..1_000_000u32).map(|value| value as u8).collect();
    channel.send(&payload).await?;
    assert_eq!(receive_some(&mut channel).await?, payload);
    channel.close().await?;
    Ok(())
}

#[tokio::test]
async fn controller_reaches_host_directly_and_is_authorized_by_password() -> TestResult {
    let (server, server_address) = start_server()?;
    let host_identity = Arc::new(Identity::generate()?);
    let controller_identity = Identity::generate()?;
    let host_id = host_identity.id();

    let host_endpoint = bind_host_endpoint(localhost())?;
    let mut registration =
        HostRegistration::register(&host_endpoint, &server_address, &host_identity).await?;
    assert_eq!(
        registration.observed_address().port(),
        host_endpoint.local_addr()?.port()
    );
    let acceptor = DirectAcceptor::spawn(host_endpoint.clone());
    let host_task = tokio::spawn(async move {
        let request = registration.next_request().await?;
        let waiting = acceptor.expect(request.token);
        punch(&host_endpoint, request.controller)?;
        let streams = timeout(STEP_TIMEOUT, waiting).await??;
        let channel = SecureChannel::respond(streams.send, streams.recv, &host_identity).await?;
        let peer = host_session(channel).await?;
        drop(streams.connection);
        Ok::<StaDeskId, Box<dyn Error + Send + Sync>>(peer)
    });

    let controller_endpoint = bind_controller_endpoint(localhost())?;
    let (_server_connection, offer) =
        request_connection(&controller_endpoint, &server_address, host_id).await?;
    let direct =
        connect_direct(&controller_endpoint, offer.host, offer.token, STEP_TIMEOUT).await?;
    let channel =
        SecureChannel::initiate(direct.send, direct.recv, &controller_identity, host_id).await?;
    controller_session(channel).await?;

    let seen_by_host = timeout(STEP_TIMEOUT, host_task).await???;
    assert_eq!(seen_by_host, controller_identity.id());
    server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn controller_reaches_host_through_relay() -> TestResult {
    let (server, server_address) = start_server()?;
    let host_identity = Arc::new(Identity::generate()?);
    let controller_identity = Identity::generate()?;
    let host_id = host_identity.id();

    let host_endpoint = bind_host_endpoint(localhost())?;
    let mut registration =
        HostRegistration::register(&host_endpoint, &server_address, &host_identity).await?;
    let host_task = tokio::spawn(async move {
        let request = registration.next_request().await?;
        let (send, recv) = join_relay(
            registration.server_connection(),
            request.token,
            RelayRole::Host,
        )
        .await?;
        let channel = SecureChannel::respond(send, recv, &host_identity).await?;
        let peer = host_session(channel).await?;
        Ok::<StaDeskId, Box<dyn Error + Send + Sync>>(peer)
    });

    let controller_endpoint = bind_controller_endpoint(localhost())?;
    let (server_connection, offer) =
        request_connection(&controller_endpoint, &server_address, host_id).await?;
    let (send, recv) = join_relay(&server_connection, offer.token, RelayRole::Controller).await?;
    let channel = SecureChannel::initiate(send, recv, &controller_identity, host_id).await?;
    controller_session(channel).await?;

    let seen_by_host = timeout(STEP_TIMEOUT, host_task).await???;
    assert_eq!(seen_by_host, controller_identity.id());
    server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn unknown_target_is_reported_offline() -> TestResult {
    let (server, server_address) = start_server()?;
    let controller_endpoint = bind_controller_endpoint(localhost())?;
    let absent = Identity::generate()?.id();
    let result = request_connection(&controller_endpoint, &server_address, absent).await;
    assert!(matches!(result, Err(NetError::PeerOffline)));
    server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn server_with_unexpected_key_is_refused() -> TestResult {
    let (server, server_address) = start_server()?;
    let wrong = ServerAddress {
        address: server_address.address,
        fingerprint: ServerCertificate::generate()?.fingerprint(),
    };
    let host_endpoint = bind_host_endpoint(localhost())?;
    let identity = Identity::generate()?;
    let result = HostRegistration::register(&host_endpoint, &wrong, &identity).await;
    assert!(matches!(result, Err(NetError::Connection(_))));
    server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn relay_refuses_unknown_token() -> TestResult {
    let (server, server_address) = start_server()?;
    let controller_endpoint = bind_controller_endpoint(localhost())?;
    let connection = stadesk_net::connect_server(&controller_endpoint, &server_address).await?;
    let result = join_relay(&connection, [9u8; 16], RelayRole::Controller).await;
    assert!(matches!(result, Err(NetError::RelayRejected)));
    server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn direct_connection_with_unknown_token_is_closed_by_host() -> TestResult {
    let host_endpoint = bind_host_endpoint(localhost())?;
    let _acceptor = DirectAcceptor::spawn(host_endpoint.clone());
    let controller_endpoint = bind_controller_endpoint(localhost())?;
    let mut direct = connect_direct(
        &controller_endpoint,
        host_endpoint.local_addr()?,
        [7u8; 16],
        STEP_TIMEOUT,
    )
    .await?;
    let mut buffer = [0u8; 1];
    let read = timeout(STEP_TIMEOUT, direct.recv.read(&mut buffer)).await?;
    assert!(read.is_err());
    Ok(())
}
