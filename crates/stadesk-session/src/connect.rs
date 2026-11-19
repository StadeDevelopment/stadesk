use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use quinn::Endpoint;
use stadesk_core::{Identity, StaDeskId};
use stadesk_net::{
    DirectAcceptor, HostRegistration, IncomingRequest, RelayRole, SecureChannel, ServerAddress,
    connect_direct, join_relay, punch, request_connection,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::error::SessionError;
use crate::host::{HostSettings, serve};

const DIRECT_ATTEMPT: Duration = Duration::from_secs(3);
const DIRECT_WAIT: Duration = Duration::from_secs(10);
const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Automatic,
    RelayOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    Direct,
    Relay,
}

pub async fn connect(
    endpoint: &Endpoint,
    server: &ServerAddress,
    identity: &Identity,
    target: StaDeskId,
    route: Route,
) -> Result<(SecureChannel, Path), SessionError> {
    let (server_connection, offer) = request_connection(endpoint, server, target).await?;
    if route == Route::Automatic
        && let Ok(direct) = connect_direct(endpoint, offer.host, offer.token, DIRECT_ATTEMPT).await
    {
        let channel = SecureChannel::initiate(direct.send, direct.recv, identity, target).await?;
        return Ok((channel, Path::Direct));
    }
    let (send, recv) = join_relay(&server_connection, offer.token, RelayRole::Controller).await?;
    let channel = SecureChannel::initiate(send, recv, identity, target).await?;
    Ok((channel, Path::Relay))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostStatus {
    Connecting,
    Online { observed: SocketAddr },
    Offline { reason: String },
}

pub struct HostService {
    status: watch::Receiver<HostStatus>,
    task: JoinHandle<()>,
}

impl HostService {
    pub fn start(
        endpoint: Endpoint,
        server: ServerAddress,
        identity: Arc<Identity>,
        settings: Arc<HostSettings>,
    ) -> Self {
        let (status_sender, status) = watch::channel(HostStatus::Connecting);
        let task = tokio::spawn(run_host(
            endpoint,
            server,
            identity,
            settings,
            status_sender,
        ));
        Self { status, task }
    }

    pub fn status(&self) -> watch::Receiver<HostStatus> {
        self.status.clone()
    }

    pub fn stop(&self) {
        self.task.abort();
    }
}

impl Drop for HostService {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run_host(
    endpoint: Endpoint,
    server: ServerAddress,
    identity: Arc<Identity>,
    settings: Arc<HostSettings>,
    status: watch::Sender<HostStatus>,
) {
    let acceptor = Arc::new(DirectAcceptor::spawn(endpoint.clone()));
    let mut backoff = RECONNECT_MIN;
    loop {
        status.send_replace(HostStatus::Connecting);
        let mut registration = match HostRegistration::register(&endpoint, &server, &identity).await
        {
            Ok(registration) => registration,
            Err(error) => {
                status.send_replace(HostStatus::Offline {
                    reason: error.to_string(),
                });
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(RECONNECT_MAX);
                continue;
            }
        };
        backoff = RECONNECT_MIN;
        status.send_replace(HostStatus::Online {
            observed: registration.observed_address(),
        });
        let reason = loop {
            match registration.next_request().await {
                Ok(request) => {
                    let context = RequestContext {
                        endpoint: endpoint.clone(),
                        server_connection: registration.server_connection().clone(),
                        acceptor: Arc::clone(&acceptor),
                        identity: Arc::clone(&identity),
                        settings: Arc::clone(&settings),
                    };
                    tokio::spawn(handle_request(context, request));
                }
                Err(error) => break error.to_string(),
            }
        };
        status.send_replace(HostStatus::Offline { reason });
        tokio::time::sleep(backoff).await;
    }
}

struct RequestContext {
    endpoint: Endpoint,
    server_connection: quinn::Connection,
    acceptor: Arc<DirectAcceptor>,
    identity: Arc<Identity>,
    settings: Arc<HostSettings>,
}

async fn handle_request(context: RequestContext, request: IncomingRequest) {
    let waiting = context.acceptor.expect(request.token);
    let _ = punch(&context.endpoint, request.controller);
    let direct = async {
        let streams = timeout(DIRECT_WAIT, waiting).await.ok()?.ok()?;
        Some((streams.send, streams.recv))
    };
    let relay = async {
        join_relay(&context.server_connection, request.token, RelayRole::Host)
            .await
            .ok()
    };
    tokio::pin!(direct, relay);
    let streams = tokio::select! {
        Some(streams) = &mut direct => Some(streams),
        Some(streams) = &mut relay => Some(streams),
        else => None,
    };
    context.acceptor.cancel(&request.token);
    let Some((send, recv)) = streams else {
        return;
    };
    let Ok(channel) = SecureChannel::respond(send, recv, &context.identity).await else {
        return;
    };
    let _ = serve(channel, &context.settings).await;
}
