use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc as std_mpsc};
use std::time::{Duration, Instant};

use stadesk_codec::{EncoderSettings, I420Frame, Vp9Encoder};
use stadesk_core::{AccessDecision, AccessGate, AccessResponse, PublicIdentity, StaDeskId};
use stadesk_desktop::{DesktopError, InputEvent};
use stadesk_net::{SecureChannel, SecureReceiver, SecureSender};
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::error::SessionError;
use crate::message::{ByeReason, CODEC_VP9, ControllerMessage, HostMessage};
use crate::source::{DesktopProvider, InputSink, bitrate_for};

const ACCESS_TIMEOUT: Duration = Duration::from_secs(30);
const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_ACCESS_ATTEMPTS: u32 = 3;
const CAPTURE_POLL: Duration = Duration::from_millis(100);

pub type Approval = Pin<Box<dyn Future<Output = bool> + Send>>;

pub trait HostDelegate: Send + Sync + 'static {
    fn approve(&self, peer: &PublicIdentity) -> Approval;

    fn session_started(&self, _peer: StaDeskId, _input: Result<(), DesktopError>) {}

    fn session_ended(&self, _peer: StaDeskId, _reason: ByeReason) {}
}

pub struct HostSettings {
    pub gate: Arc<Mutex<AccessGate>>,
    pub provider: Arc<dyn DesktopProvider>,
    pub delegate: Arc<dyn HostDelegate>,
}

enum InputCommand {
    Event(InputEvent),
    Resize(u32, u32),
}

async fn send_message(
    sender: &mut SecureSender,
    message: &HostMessage,
) -> Result<(), SessionError> {
    Ok(sender.send(&message.encode()).await?)
}

async fn receive_message(
    receiver: &mut SecureReceiver,
) -> Result<Option<ControllerMessage>, SessionError> {
    match receiver.receive().await? {
        Some(bytes) => Ok(Some(ControllerMessage::decode(&bytes)?)),
        None => Ok(None),
    }
}

async fn evaluate(
    gate: &Arc<Mutex<AccessGate>>,
    peer: &PublicIdentity,
    request: stadesk_core::AccessRequest,
) -> Result<AccessDecision, SessionError> {
    let gate = Arc::clone(gate);
    let peer = peer.clone();
    tokio::task::spawn_blocking(move || {
        gate.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .evaluate(&peer, &request, Instant::now())
    })
    .await
    .map_err(|_| SessionError::Internal)?
    .map_err(SessionError::from)
}

async fn authorize(
    sender: &mut SecureSender,
    receiver: &mut SecureReceiver,
    peer: &PublicIdentity,
    settings: &HostSettings,
) -> Result<bool, SessionError> {
    for _ in 0..MAX_ACCESS_ATTEMPTS {
        let message = timeout(ACCESS_TIMEOUT, receive_message(receiver))
            .await??
            .ok_or(SessionError::Closed)?;
        let ControllerMessage::AccessRequest(request) = message else {
            return Err(SessionError::Protocol);
        };
        let decision = evaluate(&settings.gate, peer, request).await?;
        match decision {
            AccessDecision::Granted(_) => {
                send_message(
                    sender,
                    &HostMessage::AccessResponse(AccessResponse::Granted),
                )
                .await?;
                return Ok(true);
            }
            AccessDecision::AwaitPrompt => {
                send_message(
                    sender,
                    &HostMessage::AccessResponse(AccessResponse::PendingApproval),
                )
                .await?;
                let approved = timeout(PROMPT_TIMEOUT, settings.delegate.approve(peer))
                    .await
                    .unwrap_or(false);
                let response = if approved {
                    AccessResponse::Granted
                } else {
                    AccessResponse::Denied
                };
                send_message(sender, &HostMessage::AccessResponse(response)).await?;
                return Ok(approved);
            }
            AccessDecision::WrongPassword | AccessDecision::PasswordRequired => {
                send_message(sender, &HostMessage::AccessResponse(decision.response())).await?;
            }
            AccessDecision::LockedOut { .. } | AccessDecision::Denied => {
                send_message(sender, &HostMessage::AccessResponse(decision.response())).await?;
                return Ok(false);
            }
        }
    }
    Ok(false)
}

fn run_input(sink: Box<dyn InputSink>, commands: std_mpsc::Receiver<InputCommand>) {
    let mut sink = sink;
    while let Ok(command) = commands.recv() {
        match command {
            InputCommand::Event(event) => {
                let _ = sink.inject(event);
            }
            InputCommand::Resize(width, height) => sink.resize(width, height),
        }
    }
}

struct CaptureLinks {
    frames: mpsc::Sender<HostMessage>,
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    input_commands: std_mpsc::Receiver<InputCommand>,
    input_sender: std_mpsc::Sender<InputCommand>,
    input_status: tokio::sync::oneshot::Sender<Result<(), DesktopError>>,
}

fn run_capture(provider: Arc<dyn DesktopProvider>, links: CaptureLinks) -> ByeReason {
    let CaptureLinks {
        frames,
        stop,
        keyframe,
        input_commands,
        input_sender,
        input_status,
    } = links;
    let opened = match provider.open() {
        Ok(opened) => opened,
        Err(error) => {
            let _ = input_status.send(Err(error));
            return ByeReason::CaptureFailed;
        }
    };
    let mut source = opened.frames;
    let input_thread = match opened.input {
        Ok(sink) => {
            let _ = input_status.send(Ok(()));
            std::thread::Builder::new()
                .name("stadesk-input".to_owned())
                .spawn(move || run_input(sink, input_commands))
                .ok()
        }
        Err(error) => {
            let _ = input_status.send(Err(error));
            None
        }
    };
    let started = Instant::now();
    let mut encoder: Option<(Vp9Encoder, u32, u32)> = None;
    let reason = loop {
        if stop.load(Ordering::Acquire) {
            break ByeReason::Normal;
        }
        let frame = match source.next_frame(CAPTURE_POLL) {
            Ok(Some(frame)) => frame,
            Ok(None) => continue,
            Err(_) => break ByeReason::CaptureFailed,
        };
        let size_changed = encoder
            .as_ref()
            .is_none_or(|(_, width, height)| (*width, *height) != (frame.width, frame.height));
        if size_changed {
            let settings = EncoderSettings::new(
                frame.width,
                frame.height,
                bitrate_for(frame.width, frame.height),
            );
            let Ok(created) = Vp9Encoder::new(settings) else {
                break ByeReason::CaptureFailed;
            };
            encoder = Some((created, frame.width, frame.height));
            let _ = input_sender.send(InputCommand::Resize(frame.width, frame.height));
            let start = HostMessage::StreamStart {
                codec: CODEC_VP9,
                width: frame.width,
                height: frame.height,
            };
            if frames.blocking_send(start).is_err() {
                break ByeReason::Normal;
            }
            keyframe.store(true, Ordering::Release);
        }
        let Some((active, _, _)) = encoder.as_mut() else {
            break ByeReason::CaptureFailed;
        };
        let Ok(image) =
            I420Frame::from_bgra(frame.width, frame.height, &frame.bgra, frame.stride())
        else {
            break ByeReason::CaptureFailed;
        };
        let force_keyframe = keyframe.swap(false, Ordering::AcqRel);
        let timestamp = started.elapsed().as_millis() as i64;
        let Ok(packets) = active.encode(&image, timestamp, force_keyframe) else {
            break ByeReason::CaptureFailed;
        };
        let mut delivered = true;
        for packet in packets {
            let message = HostMessage::VideoFrame {
                timestamp_ms: packet.timestamp_ms,
                keyframe: packet.keyframe,
                data: packet.data,
            };
            if frames.blocking_send(message).is_err() {
                delivered = false;
                break;
            }
        }
        if !delivered {
            break ByeReason::Normal;
        }
    };
    drop(input_sender);
    drop(source);
    if let Some(thread) = input_thread {
        let _ = thread.join();
    }
    reason
}

pub async fn serve(
    channel: SecureChannel,
    settings: &HostSettings,
) -> Result<ByeReason, SessionError> {
    let peer = channel.peer().clone();
    let peer_id = peer.id();
    let (mut sender, mut receiver) = channel.split();
    if !authorize(&mut sender, &mut receiver, &peer, settings).await? {
        let _ = send_message(&mut sender, &HostMessage::Bye(ByeReason::AccessDenied)).await;
        let _ = sender.close().await;
        return Ok(ByeReason::AccessDenied);
    }

    let (frame_sender, mut frame_receiver) = mpsc::channel(1);
    let stop = Arc::new(AtomicBool::new(false));
    let keyframe = Arc::new(AtomicBool::new(true));
    let (input_sender, input_commands) = std_mpsc::channel();
    let (status_sender, status_receiver) = tokio::sync::oneshot::channel();
    let links = CaptureLinks {
        frames: frame_sender,
        stop: Arc::clone(&stop),
        keyframe: Arc::clone(&keyframe),
        input_commands,
        input_sender: input_sender.clone(),
        input_status: status_sender,
    };
    let provider = Arc::clone(&settings.provider);
    let capture = tokio::task::spawn_blocking(move || run_capture(provider, links));
    let input_status = status_receiver.await.unwrap_or(Err(DesktopError::Stopped(
        "the capture thread ended before opening the desktop".to_owned(),
    )));
    settings.delegate.session_started(peer_id, input_status);

    let outgoing = async {
        while let Some(message) = frame_receiver.recv().await {
            send_message(&mut sender, &message).await?;
        }
        Ok::<(), SessionError>(())
    };
    let incoming = async {
        loop {
            match receive_message(&mut receiver).await? {
                Some(ControllerMessage::Input(event)) => {
                    let _ = input_sender.send(InputCommand::Event(event));
                }
                Some(ControllerMessage::KeyframeRequest) => {
                    keyframe.store(true, Ordering::Release);
                }
                Some(ControllerMessage::Bye) | None => return Ok::<(), SessionError>(()),
                Some(ControllerMessage::AccessRequest(_)) => return Err(SessionError::Protocol),
            }
        }
    };
    let outcome = tokio::select! {
        result = outgoing => result,
        result = incoming => result,
    };
    stop.store(true, Ordering::Release);
    drop(input_sender);
    drop(frame_receiver);
    let capture_reason = capture.await.unwrap_or(ByeReason::CaptureFailed);
    let reason = match (&outcome, capture_reason) {
        (Err(SessionError::Protocol), _) => ByeReason::ProtocolError,
        (_, ByeReason::CaptureFailed) => ByeReason::CaptureFailed,
        _ => ByeReason::Normal,
    };
    let _ = send_message(&mut sender, &HostMessage::Bye(reason)).await;
    let _ = sender.close().await;
    settings.delegate.session_ended(peer_id, reason);
    match outcome {
        Ok(()) | Err(SessionError::Net(_)) => Ok(reason),
        Err(error) => Err(error),
    }
}
