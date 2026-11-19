mod pins;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use quinn::{Connection, Endpoint, Incoming, RecvStream, SendStream, VarInt};
use stadesk_core::{CHALLENGE_LEN, PublicIdentity, StaDeskId, verify_ownership};
use stadesk_net::{
    ClientMessage, MAX_CONTROL_FRAME_LEN, NetError, RelayRole, SESSION_TOKEN_LEN,
    ServerCertificate, ServerFingerprint, ServerMessage, SessionToken, read_frame, server_config,
    write_frame,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;

pub use pins::{PinCheck, PinStore, default_path as default_pin_path};

const CLIENT_MESSAGE_TIMEOUT: Duration = Duration::from_secs(10);
const RELAY_TOKEN_LIFETIME: Duration = Duration::from_secs(60);
const SWEEP_INTERVAL: Duration = Duration::from_secs(10);
const RATE_WINDOW: Duration = Duration::from_secs(60);
const CONNECT_REQUESTS_PER_WINDOW: u32 = 20;
const PENDING_REQUESTS_PER_HOST: usize = 16;
const SHUTDOWN: VarInt = VarInt::from_u32(0);

pub struct RunningServer {
    endpoint: Endpoint,
    local_address: SocketAddr,
    fingerprint: ServerFingerprint,
    tasks: Vec<JoinHandle<()>>,
}

impl RunningServer {
    pub fn start(listen: SocketAddr, certificate: &ServerCertificate) -> Result<Self, NetError> {
        Self::start_with_pins(listen, certificate, PinStore::in_memory())
    }

    pub fn start_with_pins(
        listen: SocketAddr,
        certificate: &ServerCertificate,
        pins: PinStore,
    ) -> Result<Self, NetError> {
        let endpoint = Endpoint::server(server_config(certificate)?, listen)?;
        let local_address = endpoint.local_addr()?;
        let fingerprint = certificate.fingerprint();
        let state = Arc::new(State::new(fingerprint, pins));
        let accept_endpoint = endpoint.clone();
        let accept_state = Arc::clone(&state);
        let accept_task = tokio::spawn(async move {
            while let Some(incoming) = accept_endpoint.accept().await {
                tokio::spawn(serve_connection(incoming, Arc::clone(&accept_state)));
            }
        });
        let sweep_task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
            loop {
                ticker.tick().await;
                state.sweep(Instant::now());
            }
        });
        Ok(Self {
            endpoint,
            local_address,
            fingerprint,
            tasks: vec![accept_task, sweep_task],
        })
    }

    pub fn local_address(&self) -> SocketAddr {
        self.local_address
    }

    pub fn fingerprint(&self) -> ServerFingerprint {
        self.fingerprint
    }

    pub async fn shutdown(self) {
        self.endpoint.close(SHUTDOWN, b"shutdown");
        for task in &self.tasks {
            task.abort();
        }
        self.endpoint.wait_idle().await;
    }
}

struct HostEntry {
    identity: PublicIdentity,
    address: SocketAddr,
    connection_id: usize,
    requests: mpsc::Sender<ServerMessage>,
}

struct WaitingParty {
    role: RelayRole,
    send: SendStream,
    recv: RecvStream,
}

struct RelaySlot {
    expires: Instant,
    waiting: Option<WaitingParty>,
}

#[derive(Default)]
struct RateLimiter {
    windows: HashMap<IpAddr, (Instant, u32)>,
}

impl RateLimiter {
    fn allow(&mut self, ip: IpAddr, now: Instant) -> bool {
        let entry = self.windows.entry(ip).or_insert((now, 0));
        if now.duration_since(entry.0) >= RATE_WINDOW {
            *entry = (now, 0);
        }
        if entry.1 >= CONNECT_REQUESTS_PER_WINDOW {
            return false;
        }
        entry.1 += 1;
        true
    }

    fn sweep(&mut self, now: Instant) {
        self.windows
            .retain(|_, (started, _)| now.duration_since(*started) < RATE_WINDOW);
    }
}

struct State {
    fingerprint: ServerFingerprint,
    hosts: Mutex<HashMap<StaDeskId, HostEntry>>,
    relays: Mutex<HashMap<SessionToken, RelaySlot>>,
    limiter: Mutex<RateLimiter>,
    pins: Mutex<PinStore>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl State {
    fn new(fingerprint: ServerFingerprint, pins: PinStore) -> Self {
        Self {
            fingerprint,
            hosts: Mutex::default(),
            relays: Mutex::default(),
            limiter: Mutex::default(),
            pins: Mutex::new(pins),
        }
    }

    fn sweep(&self, now: Instant) {
        lock(&self.relays).retain(|_, slot| slot.expires > now);
        lock(&self.limiter).sweep(now);
    }
}

fn random_bytes<const N: usize>() -> Result<[u8; N], NetError> {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).map_err(|_| NetError::Randomness)?;
    Ok(out)
}

async fn send_server_message(
    send: &mut SendStream,
    message: &ServerMessage,
) -> Result<(), NetError> {
    write_frame(send, &message.encode()).await
}

async fn receive_client_message(recv: &mut RecvStream) -> Result<ClientMessage, NetError> {
    let frame = timeout(
        CLIENT_MESSAGE_TIMEOUT,
        read_frame(recv, MAX_CONTROL_FRAME_LEN),
    )
    .await??;
    Ok(ClientMessage::decode(&frame)?)
}

async fn serve_connection(incoming: Incoming, state: Arc<State>) {
    let Ok(connection) = incoming.await else {
        return;
    };
    while let Ok((send, recv)) = connection.accept_bi().await {
        tokio::spawn(serve_stream(
            connection.clone(),
            send,
            recv,
            Arc::clone(&state),
        ));
    }
}

async fn serve_stream(
    connection: Connection,
    send: SendStream,
    mut recv: RecvStream,
    state: Arc<State>,
) -> Result<(), NetError> {
    match receive_client_message(&mut recv).await? {
        ClientMessage::Register { identity } => {
            serve_registration(connection, send, recv, *identity, state).await
        }
        ClientMessage::Connect { target } => serve_connect(connection, send, target, state).await,
        ClientMessage::RelayJoin { token, role } => {
            serve_relay(send, recv, token, role, state).await
        }
        ClientMessage::Proof { .. } => Err(NetError::UnexpectedMessage),
    }
}

async fn serve_registration(
    connection: Connection,
    mut send: SendStream,
    mut recv: RecvStream,
    identity: PublicIdentity,
    state: Arc<State>,
) -> Result<(), NetError> {
    let challenge = random_bytes::<CHALLENGE_LEN>()?;
    send_server_message(&mut send, &ServerMessage::Challenge { challenge }).await?;
    let ClientMessage::Proof { signature } = receive_client_message(&mut recv).await? else {
        return Err(NetError::UnexpectedMessage);
    };
    if verify_ownership(
        &identity,
        state.fingerprint.as_bytes(),
        &challenge,
        &signature,
    )
    .is_err()
    {
        send_server_message(&mut send, &ServerMessage::RegistrationRejected).await?;
        return Ok(());
    }
    let pinned = lock(&state.pins).check(&identity);
    if !matches!(pinned, Ok(PinCheck::Matches | PinCheck::New)) {
        send_server_message(&mut send, &ServerMessage::RegistrationRejected).await?;
        return Ok(());
    }

    let id = identity.id();
    let address = connection.remote_address();
    let connection_id = connection.stable_id();
    let (requests, mut pending) = mpsc::channel(PENDING_REQUESTS_PER_HOST);
    let accepted = {
        let mut hosts = lock(&state.hosts);
        match hosts.get(&id) {
            Some(existing) if existing.identity != identity => false,
            _ => {
                hosts.insert(
                    id,
                    HostEntry {
                        identity: identity.clone(),
                        address,
                        connection_id,
                        requests,
                    },
                );
                true
            }
        }
    };
    if !accepted {
        send_server_message(&mut send, &ServerMessage::RegistrationRejected).await?;
        return Ok(());
    }

    let served = async {
        send_server_message(&mut send, &ServerMessage::Registered { observed: address }).await?;
        let mut probe = [0u8; 1];
        loop {
            tokio::select! {
                request = pending.recv() => match request {
                    Some(message) => send_server_message(&mut send, &message).await?,
                    None => return Ok::<(), NetError>(()),
                },
                _ = recv.read(&mut probe) => return Ok(()),
                _ = connection.closed() => return Ok(()),
            }
        }
    }
    .await;

    let mut hosts = lock(&state.hosts);
    if hosts
        .get(&id)
        .is_some_and(|entry| entry.connection_id == connection_id)
    {
        hosts.remove(&id);
    }
    served
}

async fn serve_connect(
    connection: Connection,
    mut send: SendStream,
    target: StaDeskId,
    state: Arc<State>,
) -> Result<(), NetError> {
    let controller = connection.remote_address();
    let now = Instant::now();
    if !lock(&state.limiter).allow(controller.ip(), now) {
        send_server_message(&mut send, &ServerMessage::RateLimited).await?;
        return Ok(());
    }
    let host = lock(&state.hosts)
        .get(&target)
        .map(|entry| (entry.address, entry.requests.clone()));
    let Some((host_address, requests)) = host else {
        send_server_message(&mut send, &ServerMessage::PeerOffline).await?;
        return Ok(());
    };
    let token = random_bytes::<SESSION_TOKEN_LEN>()?;
    lock(&state.relays).insert(
        token,
        RelaySlot {
            expires: now + RELAY_TOKEN_LIFETIME,
            waiting: None,
        },
    );
    if requests
        .try_send(ServerMessage::IncomingRequest { token, controller })
        .is_err()
    {
        lock(&state.relays).remove(&token);
        send_server_message(&mut send, &ServerMessage::RateLimited).await?;
        return Ok(());
    }
    send_server_message(
        &mut send,
        &ServerMessage::PeerFound {
            token,
            host: host_address,
        },
    )
    .await
}

enum RelayJoin {
    Paired(WaitingParty),
    Rejected,
}

async fn serve_relay(
    mut send: SendStream,
    recv: RecvStream,
    token: SessionToken,
    role: RelayRole,
    state: Arc<State>,
) -> Result<(), NetError> {
    let now = Instant::now();
    let (join, recv) = {
        let mut relays = lock(&state.relays);
        match relays.get_mut(&token) {
            Some(slot) if slot.expires > now => match slot.waiting.take() {
                None => {
                    slot.waiting = Some(WaitingParty { role, send, recv });
                    return Ok(());
                }
                Some(other) if other.role != role => {
                    relays.remove(&token);
                    (RelayJoin::Paired(other), recv)
                }
                Some(other) => {
                    slot.waiting = Some(other);
                    (RelayJoin::Rejected, recv)
                }
            },
            _ => (RelayJoin::Rejected, recv),
        }
    };
    let mut other = match join {
        RelayJoin::Paired(other) => other,
        RelayJoin::Rejected => {
            send_server_message(&mut send, &ServerMessage::RelayRejected).await?;
            return Ok(());
        }
    };
    send_server_message(&mut send, &ServerMessage::RelayReady).await?;
    send_server_message(&mut other.send, &ServerMessage::RelayReady).await?;
    tokio::join!(pipe(recv, other.send), pipe(other.recv, send));
    Ok(())
}

async fn pipe(mut from: RecvStream, mut to: SendStream) {
    let _ = tokio::io::copy(&mut from, &mut to).await;
    let _ = to.finish();
}
