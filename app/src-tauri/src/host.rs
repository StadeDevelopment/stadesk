use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use stadesk_core::{PublicIdentity, StaDeskId};
use stadesk_desktop::{CaptureOptions, DesktopError};
use stadesk_net::bind_host_endpoint;
use stadesk_session::{
    Approval, ByeReason, HostDelegate, HostService, HostSettings, HostStatus, ScreenProvider,
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::oneshot;

use crate::state::{
    AppState, Decision, HostStatusView, RunningHost, ServerProblem, lock, resolve_server,
    unspecified_for,
};

const RESOLVE_RETRY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccessRequestEvent {
    request_id: u64,
    peer_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostSessionEvent {
    peer_id: String,
    active: bool,
    input_error: Option<String>,
    reason: Option<String>,
}

pub fn set_status(app: &AppHandle, status: HostStatusView) {
    let state = app.state::<AppState>();
    *lock(&state.host_status) = status.clone();
    let _ = app.emit("host-status", status);
}

struct PendingGuard {
    app: AppHandle,
    request_id: u64,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        let state = self.app.state::<AppState>();
        lock(&state.pending).remove(&self.request_id);
        let _ = self.app.emit("access-request-closed", self.request_id);
    }
}

fn bring_to_front(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

struct AppDelegate {
    app: AppHandle,
}

impl HostDelegate for AppDelegate {
    fn approve(&self, peer: &PublicIdentity) -> Approval {
        let app = self.app.clone();
        let peer = peer.clone();
        Box::pin(async move {
            let state = app.state::<AppState>();
            let request_id = state.next_id();
            let (sender, receiver) = oneshot::channel();
            lock(&state.pending).insert(request_id, sender);
            let _guard = PendingGuard {
                app: app.clone(),
                request_id,
            };
            let _ = app.emit(
                "access-request",
                AccessRequestEvent {
                    request_id,
                    peer_id: peer.id().to_string(),
                },
            );
            bring_to_front(&app);
            let decision = receiver.await.unwrap_or(Decision {
                accept: false,
                trust: false,
            });
            if decision.accept && decision.trust {
                let _ = state.trust(&peer);
            }
            decision.accept
        })
    }

    fn session_started(&self, peer: StaDeskId, input: Result<(), DesktopError>) {
        let _ = self.app.emit(
            "host-session",
            HostSessionEvent {
                peer_id: peer.to_string(),
                active: true,
                input_error: input.err().map(|error| error.to_string()),
                reason: None,
            },
        );
        let _ = self.app.emit("overview-changed", ());
    }

    fn session_ended(&self, peer: StaDeskId, reason: ByeReason) {
        let _ = self.app.emit(
            "host-session",
            HostSessionEvent {
                peer_id: peer.to_string(),
                active: false,
                input_error: None,
                reason: Some(format!("{reason:?}")),
            },
        );
        let _ = self.app.emit("overview-changed", ());
    }
}

fn status_view(status: &HostStatus) -> HostStatusView {
    match status {
        HostStatus::Connecting => HostStatusView::Connecting,
        HostStatus::Online { observed } => HostStatusView::Online {
            observed: observed.to_string(),
        },
        HostStatus::Offline { reason } => HostStatusView::Offline {
            reason: reason.clone(),
        },
    }
}

pub fn restart(app: AppHandle) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(restart_host(app))
}

async fn restart_host(app: AppHandle) {
    let state = app.state::<AppState>();
    let mut running = state.host.lock().await;
    *running = None;
    let config = state.config();
    let server = match resolve_server(&config).await {
        Ok(server) => server,
        Err(ServerProblem::NotConfigured) => {
            set_status(&app, HostStatusView::NotConfigured);
            return;
        }
        Err(problem) => {
            let reason = match problem {
                ServerProblem::InvalidAddress => "invalidServerAddress".to_owned(),
                ServerProblem::InvalidKey => "invalidServerKey".to_owned(),
                ServerProblem::Unresolved(detail) => format!("unresolved: {detail}"),
                ServerProblem::NotConfigured => String::new(),
            };
            set_status(&app, HostStatusView::Offline { reason });
            let retry = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(RESOLVE_RETRY).await;
                restart(retry).await;
            });
            return;
        }
    };
    let endpoint = match bind_host_endpoint(unspecified_for(&server.address)) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            set_status(
                &app,
                HostStatusView::Offline {
                    reason: error.to_string(),
                },
            );
            return;
        }
    };
    let settings = HostSettings {
        gate: Arc::clone(&state.gate),
        provider: Arc::new(ScreenProvider {
            display: None,
            options: CaptureOptions::default(),
        }),
        delegate: Arc::new(AppDelegate { app: app.clone() }),
    };
    let service = HostService::start(
        endpoint,
        server,
        Arc::clone(&state.identity),
        Arc::new(settings),
    );
    let mut status = service.status();
    let watcher_app = app.clone();
    let watcher = tauri::async_runtime::spawn(async move {
        loop {
            let view = status_view(&status.borrow_and_update());
            set_status(&watcher_app, view);
            if status.changed().await.is_err() {
                return;
            }
        }
    });
    *running = Some(RunningHost {
        _service: service,
        watcher,
    });
}
