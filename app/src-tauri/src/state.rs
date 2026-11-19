use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use stadesk_core::{AccessGate, Identity, PublicIdentity};
use stadesk_net::{ServerAddress, ServerFingerprint};
use stadesk_session::HostService;
use tokio::sync::oneshot;

use crate::config::{Config, Store, encode_hex};
use crate::viewer::ViewerEntry;

pub const DEFAULT_PORT: u16 = 21300;
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum HostStatusView {
    NotConfigured,
    Connecting,
    Online { observed: String },
    Offline { reason: String },
}

#[derive(Debug, Clone, Copy)]
pub struct Decision {
    pub accept: bool,
    pub trust: bool,
}

pub struct RunningHost {
    pub _service: HostService,
    pub watcher: tauri::async_runtime::JoinHandle<()>,
}

impl Drop for RunningHost {
    fn drop(&mut self) {
        self.watcher.abort();
    }
}

pub struct AppState {
    pub store: Store,
    pub identity: Arc<Identity>,
    pub config: Mutex<Config>,
    pub gate: Arc<Mutex<AccessGate>>,
    pub host: tokio::sync::Mutex<Option<RunningHost>>,
    pub host_status: Mutex<HostStatusView>,
    pub pending: Mutex<HashMap<u64, oneshot::Sender<Decision>>>,
    pub viewers: Mutex<HashMap<u64, ViewerEntry>>,
    pub startup_target: Mutex<Option<String>>,
    next_id: AtomicU64,
}

impl AppState {
    pub fn new(
        store: Store,
        identity: Identity,
        config: Config,
        gate: AccessGate,
        startup_target: Option<String>,
    ) -> Self {
        Self {
            store,
            identity: Arc::new(identity),
            config: Mutex::new(config),
            gate: Arc::new(Mutex::new(gate)),
            host: tokio::sync::Mutex::new(None),
            host_status: Mutex::new(HostStatusView::NotConfigured),
            pending: Mutex::new(HashMap::new()),
            viewers: Mutex::new(HashMap::new()),
            startup_target: Mutex::new(startup_target),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn config(&self) -> Config {
        lock(&self.config).clone()
    }

    pub fn update_config(&self, change: impl FnOnce(&mut Config)) -> std::io::Result<Config> {
        let mut config = lock(&self.config);
        let mut updated = config.clone();
        change(&mut updated);
        self.store.save_config(&updated)?;
        updated.apply_to(&mut lock(&self.gate));
        *config = updated.clone();
        Ok(updated)
    }

    pub fn trust(&self, peer: &PublicIdentity) -> std::io::Result<()> {
        let encoded = encode_hex(peer.encode());
        self.update_config(|config| {
            if !config.trusted_devices.contains(&encoded) {
                config.trusted_devices.push(encoded);
            }
        })
        .map(|_| ())
    }
}

pub fn server_endpoint(address: &str) -> Option<(String, u16)> {
    let address = address.trim();
    if address.is_empty() {
        return None;
    }
    if let Ok(socket) = address.parse::<SocketAddr>() {
        return Some((socket.ip().to_string(), socket.port()));
    }
    if let Ok(ip) = address.trim_matches(['[', ']']).parse::<IpAddr>() {
        return Some((ip.to_string(), DEFAULT_PORT));
    }
    match address.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && !host.contains(':') => {
            port.parse().ok().map(|port| (host.to_owned(), port))
        }
        Some(_) => None,
        None => Some((address.to_owned(), DEFAULT_PORT)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerProblem {
    NotConfigured,
    InvalidAddress,
    InvalidKey,
    Unresolved(String),
}

pub async fn resolve_server(config: &Config) -> Result<ServerAddress, ServerProblem> {
    if config.server_address.trim().is_empty() || config.server_key.trim().is_empty() {
        return Err(ServerProblem::NotConfigured);
    }
    let (host, port) =
        server_endpoint(&config.server_address).ok_or(ServerProblem::InvalidAddress)?;
    let fingerprint: ServerFingerprint = config
        .server_key
        .parse()
        .map_err(|_| ServerProblem::InvalidKey)?;
    let mut candidates: Vec<SocketAddr> = Vec::new();
    for address in tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|error| ServerProblem::Unresolved(error.to_string()))?
    {
        if !candidates.contains(&address) {
            candidates.push(address);
        }
    }
    let first = *candidates
        .first()
        .ok_or_else(|| ServerProblem::Unresolved(host.clone()))?;
    let address = if candidates.len() == 1 {
        first
    } else {
        reachable_server(&candidates, fingerprint)
            .await
            .unwrap_or(first)
    };
    Ok(ServerAddress {
        address,
        fingerprint,
    })
}

async fn probe_server(address: SocketAddr, fingerprint: ServerFingerprint) -> Option<SocketAddr> {
    let endpoint = stadesk_net::bind_controller_endpoint(unspecified_for(&address)).ok()?;
    let server = ServerAddress {
        address,
        fingerprint,
    };
    let connection = tokio::time::timeout(
        PROBE_TIMEOUT,
        stadesk_net::connect_server(&endpoint, &server),
    )
    .await
    .ok()?
    .ok()?;
    connection.close(quinn::VarInt::from_u32(0), b"");
    endpoint.wait_idle().await;
    Some(address)
}

async fn reachable_server(
    candidates: &[SocketAddr],
    fingerprint: ServerFingerprint,
) -> Option<SocketAddr> {
    let mut probes = tokio::task::JoinSet::new();
    for &address in candidates {
        probes.spawn(probe_server(address, fingerprint));
    }
    while let Some(result) = probes.join_next().await {
        if let Ok(Some(address)) = result {
            probes.abort_all();
            return Some(address);
        }
    }
    None
}

pub fn unspecified_for(server: &SocketAddr) -> SocketAddr {
    match server {
        SocketAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        SocketAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_addresses_accept_host_port_ip_and_default_port() {
        assert_eq!(
            server_endpoint("relay.example.com:4000"),
            Some(("relay.example.com".to_owned(), 4000))
        );
        assert_eq!(
            server_endpoint(" relay.example.com "),
            Some(("relay.example.com".to_owned(), DEFAULT_PORT))
        );
        assert_eq!(
            server_endpoint("203.0.113.9:21300"),
            Some(("203.0.113.9".to_owned(), 21300))
        );
        assert_eq!(
            server_endpoint("[2001:db8::1]:9000"),
            Some(("2001:db8::1".to_owned(), 9000))
        );
        assert_eq!(
            server_endpoint("2001:db8::1"),
            Some(("2001:db8::1".to_owned(), DEFAULT_PORT))
        );
        assert_eq!(server_endpoint("relay:notaport"), None);
        assert_eq!(server_endpoint(":4000"), None);
        assert_eq!(server_endpoint(""), None);
    }
}
