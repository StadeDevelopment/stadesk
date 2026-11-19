use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use stadesk_core::StaDeskId;
use stadesk_desktop::{InputEvent, PointerButton, usage_from_code};
use stadesk_net::bind_controller_endpoint;
use stadesk_session::{Delivery, Path, Route, StreamEvent, ViewerSession, connect};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::error::UiError;
use crate::state::{AppState, lock, resolve_server, unspecified_for};

const MONITOR_INTERVAL: Duration = Duration::from_millis(300);
const STREAM_START: u8 = 0;
const STREAM_PACKET: u8 = 1;

type StreamSlot = Arc<tokio::sync::Mutex<Option<UnboundedReceiver<StreamEvent>>>>;

pub struct ViewerEntry {
    session: Option<ViewerSession>,
    stream: StreamSlot,
    attachments: Arc<AtomicU32>,
    _endpoint: quinn::Endpoint,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connected {
    session_id: u64,
    peer_id: String,
    path: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ViewerEnded {
    session_id: u64,
    reason: String,
}

pub fn encode_stream_event(event: &StreamEvent) -> Vec<u8> {
    match event {
        StreamEvent::Start { width, height } => {
            let mut bytes = Vec::with_capacity(9);
            bytes.push(STREAM_START);
            bytes.extend_from_slice(&width.to_be_bytes());
            bytes.extend_from_slice(&height.to_be_bytes());
            bytes
        }
        StreamEvent::Packet {
            timestamp_ms,
            keyframe,
            data,
        } => {
            let mut bytes = Vec::with_capacity(10 + data.len());
            bytes.push(STREAM_PACKET);
            bytes.push(u8::from(*keyframe));
            bytes.extend_from_slice(&timestamp_ms.to_be_bytes());
            bytes.extend_from_slice(data);
            bytes
        }
    }
}

fn window_label(session_id: u64) -> String {
    format!("viewer-{session_id}")
}

pub fn session_from_label(label: &str) -> Option<u64> {
    label.strip_prefix("viewer-")?.parse().ok()
}

async fn finish(app: &AppHandle, session_id: u64, reason: String) {
    let _ = app.emit("viewer-ended", ViewerEnded { session_id, reason });
    if let Some(window) = app.get_webview_window(&window_label(session_id)) {
        let _ = window.destroy();
    }
}

fn spawn_monitor(app: AppHandle, session_id: u64) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(MONITOR_INTERVAL).await;
            let state = app.state::<AppState>();
            let finished = {
                let viewers = lock(&state.viewers);
                match viewers.get(&session_id) {
                    Some(entry) => entry
                        .session
                        .as_ref()
                        .is_none_or(ViewerSession::is_finished),
                    None => return,
                }
            };
            if finished {
                let entry = lock(&state.viewers).remove(&session_id);
                let reason = match entry.and_then(|mut entry| entry.session.take()) {
                    Some(session) => match session.wait().await {
                        Ok(reason) => format!("{reason:?}"),
                        Err(error) => UiError::from(error).code,
                    },
                    None => "Normal".to_owned(),
                };
                finish(&app, session_id, reason).await;
                return;
            }
        }
    });
}

#[tauri::command]
pub async fn connect_to(
    app: AppHandle,
    state: State<'_, AppState>,
    target: String,
    password: Option<String>,
) -> Result<Connected, UiError> {
    let target = StaDeskId::parse(&target).map_err(|_| UiError::new("invalidId"))?;
    if target == state.identity.id() {
        return Err(UiError::new("selfConnection"));
    }
    let server = resolve_server(&state.config())
        .await
        .map_err(UiError::from)?;
    let endpoint =
        bind_controller_endpoint(unspecified_for(&server.address)).map_err(UiError::from)?;
    let (channel, path) = connect(
        &endpoint,
        &server,
        &state.identity,
        target,
        Route::Automatic,
    )
    .await
    .map_err(UiError::from)?;
    let (packets, stream) = unbounded_channel();
    let pending_app = app.clone();
    let password = password.filter(|password| !password.is_empty());
    let session = ViewerSession::start_with(
        channel,
        password.as_deref(),
        move || {
            let _ = pending_app.emit("connect-pending", target.to_string());
        },
        Delivery::Packets(packets),
    )
    .await
    .map_err(UiError::from)?;
    let session_id = state.next_id();
    lock(&state.viewers).insert(
        session_id,
        ViewerEntry {
            session: Some(session),
            stream: Arc::new(tokio::sync::Mutex::new(Some(stream))),
            attachments: Arc::new(AtomicU32::new(0)),
            _endpoint: endpoint,
        },
    );
    let opened = WebviewWindowBuilder::new(
        &app,
        window_label(session_id),
        WebviewUrl::App(format!("index.html#/viewer/{session_id}").into()),
    )
    .title(format!("StaDesk — {target}"))
    .inner_size(1280.0, 800.0)
    .min_inner_size(480.0, 320.0)
    .build();
    if let Err(error) = opened {
        disconnect_session(&app, session_id).await;
        return Err(UiError::with_detail("window", error.to_string()));
    }
    spawn_monitor(app.clone(), session_id);
    Ok(Connected {
        session_id,
        peer_id: target.to_string(),
        path: match path {
            Path::Direct => "direct",
            Path::Relay => "relay",
        },
    })
}

#[tauri::command]
pub async fn attach_stream(
    state: State<'_, AppState>,
    session_id: u64,
    channel: Channel,
) -> Result<(), UiError> {
    let (slot, attachments) = {
        let viewers = lock(&state.viewers);
        let entry = viewers
            .get(&session_id)
            .ok_or_else(|| UiError::new("unknownSession"))?;
        (Arc::clone(&entry.stream), Arc::clone(&entry.attachments))
    };
    let mut receiver = slot
        .lock()
        .await
        .take()
        .ok_or_else(|| UiError::new("alreadyAttached"))?;
    if attachments.fetch_add(1, Ordering::Relaxed) > 0
        && let Some(entry) = lock(&state.viewers).get(&session_id)
        && let Some(session) = &entry.session
    {
        session.request_keyframe();
    }
    tauri::async_runtime::spawn(async move {
        while let Some(event) = receiver.recv().await {
            let body = InvokeResponseBody::Raw(encode_stream_event(&event));
            if channel.send(body).is_err() {
                *slot.lock().await = Some(receiver);
                return;
            }
        }
    });
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum InputMessage {
    Move { x: u32, y: u32 },
    Button { button: String, pressed: bool },
    Scroll { horizontal: i32, vertical: i32 },
    Key { code: String, pressed: bool },
}

fn button(name: &str) -> Option<PointerButton> {
    Some(match name {
        "left" => PointerButton::Left,
        "right" => PointerButton::Right,
        "middle" => PointerButton::Middle,
        "back" => PointerButton::Back,
        "forward" => PointerButton::Forward,
        _ => return None,
    })
}

pub fn to_input_event(message: &InputMessage) -> Option<InputEvent> {
    Some(match message {
        InputMessage::Move { x, y } => InputEvent::PointerMove { x: *x, y: *y },
        InputMessage::Button {
            button: name,
            pressed,
        } => InputEvent::PointerButton {
            button: button(name)?,
            pressed: *pressed,
        },
        InputMessage::Scroll {
            horizontal,
            vertical,
        } => InputEvent::Scroll {
            horizontal: *horizontal,
            vertical: *vertical,
        },
        InputMessage::Key { code, pressed } => InputEvent::Key {
            usage: usage_from_code(code)?,
            pressed: *pressed,
        },
    })
}

#[tauri::command]
pub fn send_input(
    state: State<'_, AppState>,
    session_id: u64,
    events: Vec<InputMessage>,
) -> Result<(), UiError> {
    let viewers = lock(&state.viewers);
    let session = viewers
        .get(&session_id)
        .and_then(|entry| entry.session.as_ref())
        .ok_or_else(|| UiError::new("unknownSession"))?;
    for event in events.iter().filter_map(to_input_event) {
        session.send_input(event).map_err(UiError::from)?;
    }
    Ok(())
}

#[tauri::command]
pub fn request_keyframe(state: State<'_, AppState>, session_id: u64) -> Result<(), UiError> {
    let viewers = lock(&state.viewers);
    let session = viewers
        .get(&session_id)
        .and_then(|entry| entry.session.as_ref())
        .ok_or_else(|| UiError::new("unknownSession"))?;
    session.request_keyframe();
    Ok(())
}

pub async fn disconnect_session(app: &AppHandle, session_id: u64) {
    let state = app.state::<AppState>();
    let Some(mut entry) = lock(&state.viewers).remove(&session_id) else {
        return;
    };
    if let Some(session) = entry.session.take() {
        let _ = session.close().await;
    }
    finish(app, session_id, "Normal".to_owned()).await;
}

#[tauri::command]
pub async fn disconnect(app: AppHandle, session_id: u64) -> Result<(), UiError> {
    disconnect_session(&app, session_id).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_events_use_the_documented_binary_layout() {
        assert_eq!(
            encode_stream_event(&StreamEvent::Start {
                width: 0x0102,
                height: 0x0304
            }),
            [0, 0, 0, 1, 2, 0, 0, 3, 4]
        );
        assert_eq!(
            encode_stream_event(&StreamEvent::Packet {
                timestamp_ms: 258,
                keyframe: true,
                data: vec![9, 8]
            }),
            [1, 1, 0, 0, 0, 0, 0, 0, 1, 2, 9, 8]
        );
    }

    #[test]
    fn input_messages_parse_from_the_frontend_shape() {
        let messages: Vec<InputMessage> = serde_json::from_str(
            r#"[{"kind":"move","x":5,"y":6},{"kind":"button","button":"right","pressed":true},
                {"kind":"scroll","horizontal":-120,"vertical":240},{"kind":"key","code":"KeyA","pressed":false},
                {"kind":"key","code":"Unidentified","pressed":true},{"kind":"button","button":"sixth","pressed":true}]"#,
        )
        .unwrap();
        let events: Vec<InputEvent> = messages.iter().filter_map(to_input_event).collect();
        assert_eq!(
            events,
            [
                InputEvent::PointerMove { x: 5, y: 6 },
                InputEvent::PointerButton {
                    button: PointerButton::Right,
                    pressed: true
                },
                InputEvent::Scroll {
                    horizontal: -120,
                    vertical: 240
                },
                InputEvent::Key {
                    usage: 0x04,
                    pressed: false
                }
            ]
        );
        assert_eq!(session_from_label("viewer-42"), Some(42));
        assert_eq!(session_from_label("main"), None);
    }
}
