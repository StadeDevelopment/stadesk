use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use stadesk_codec::{I420Frame, Vp9Decoder};
use stadesk_core::{AccessRequest, AccessResponse, PublicIdentity};
use stadesk_desktop::InputEvent;
use stadesk_net::{SecureChannel, SecureReceiver, SecureSender};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::error::SessionError;
use crate::message::{ByeReason, CODEC_VP9, ControllerMessage, HostMessage};

const ACCESS_TIMEOUT: Duration = Duration::from_secs(90);
const DECODER_THREADS: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    pub sequence: u64,
    pub image: Arc<I420Frame>,
}

enum DecodeCommand {
    Start,
    Packet(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    Start {
        width: u32,
        height: u32,
    },
    Packet {
        timestamp_ms: i64,
        keyframe: bool,
        data: Vec<u8>,
    },
}

pub enum Delivery {
    Decoded,
    Packets(mpsc::UnboundedSender<StreamEvent>),
}

enum StreamTarget {
    Decoder(std_mpsc::Sender<DecodeCommand>),
    Packets(mpsc::UnboundedSender<StreamEvent>),
}

impl StreamTarget {
    fn start(&self, width: u32, height: u32) {
        match self {
            Self::Decoder(decoder) => {
                let _ = decoder.send(DecodeCommand::Start);
            }
            Self::Packets(packets) => {
                let _ = packets.send(StreamEvent::Start { width, height });
            }
        }
    }

    fn packet(&self, timestamp_ms: i64, keyframe: bool, data: Vec<u8>) {
        match self {
            Self::Decoder(decoder) => {
                let _ = decoder.send(DecodeCommand::Packet(data));
            }
            Self::Packets(packets) => {
                let _ = packets.send(StreamEvent::Packet {
                    timestamp_ms,
                    keyframe,
                    data,
                });
            }
        }
    }
}

async fn send_message(
    sender: &mut SecureSender,
    message: &ControllerMessage,
) -> Result<(), SessionError> {
    Ok(sender.send(&message.encode()?).await?)
}

async fn receive_message(
    receiver: &mut SecureReceiver,
) -> Result<Option<HostMessage>, SessionError> {
    match receiver.receive().await? {
        Some(bytes) => Ok(Some(HostMessage::decode(&bytes)?)),
        None => Ok(None),
    }
}

fn run_decoder(
    commands: std_mpsc::Receiver<DecodeCommand>,
    frames: watch::Sender<Option<VideoFrame>>,
    control: mpsc::UnboundedSender<ControllerMessage>,
) {
    let mut decoder: Option<Vp9Decoder> = None;
    let mut sequence = 0u64;
    let mut waiting_for_keyframe = false;
    while let Ok(command) = commands.recv() {
        match command {
            DecodeCommand::Start => {
                decoder = Vp9Decoder::new(DECODER_THREADS).ok();
                waiting_for_keyframe = false;
            }
            DecodeCommand::Packet(data) => {
                let Some(active) = decoder.as_mut() else {
                    continue;
                };
                match active.decode(&data) {
                    Ok(images) => {
                        waiting_for_keyframe = false;
                        for image in images {
                            sequence += 1;
                            frames.send_replace(Some(VideoFrame {
                                sequence,
                                image: Arc::new(image),
                            }));
                        }
                    }
                    Err(_) => {
                        if !waiting_for_keyframe {
                            waiting_for_keyframe = true;
                            let _ = control.send(ControllerMessage::KeyframeRequest);
                        }
                        decoder = Vp9Decoder::new(DECODER_THREADS).ok();
                    }
                }
            }
        }
    }
}

pub struct ViewerSession {
    peer: PublicIdentity,
    control: mpsc::UnboundedSender<ControllerMessage>,
    frames: watch::Receiver<Option<VideoFrame>>,
    finished: JoinHandle<Result<ByeReason, SessionError>>,
}

impl ViewerSession {
    pub async fn start(
        channel: SecureChannel,
        password: Option<&str>,
        on_pending: impl FnOnce(),
    ) -> Result<Self, SessionError> {
        Self::start_with(channel, password, on_pending, Delivery::Decoded).await
    }

    pub async fn start_with(
        channel: SecureChannel,
        password: Option<&str>,
        on_pending: impl FnOnce(),
        delivery: Delivery,
    ) -> Result<Self, SessionError> {
        let peer = channel.peer().clone();
        let (mut sender, mut receiver) = channel.split();
        let request = match password {
            Some(password) => AccessRequest::with_password(password),
            None => AccessRequest::without_password(),
        };
        send_message(&mut sender, &ControllerMessage::AccessRequest(request)).await?;
        let mut on_pending = Some(on_pending);
        loop {
            let message = timeout(ACCESS_TIMEOUT, receive_message(&mut receiver))
                .await??
                .ok_or(SessionError::Closed)?;
            match message {
                HostMessage::AccessResponse(AccessResponse::Granted) => break,
                HostMessage::AccessResponse(AccessResponse::PendingApproval) => {
                    if let Some(notify) = on_pending.take() {
                        notify();
                    }
                }
                HostMessage::AccessResponse(other) => {
                    let _ = sender.close().await;
                    return Err(SessionError::Access(other));
                }
                HostMessage::Bye(reason) => return Err(SessionError::Ended(reason)),
                _ => return Err(SessionError::Protocol),
            }
        }

        let (control, mut control_receiver) = mpsc::unbounded_channel();
        let (frame_sender, frames) = watch::channel(None);
        let target = match delivery {
            Delivery::Decoded => {
                let (decode_sender, decode_commands) = std_mpsc::channel();
                let decoder_control = control.clone();
                std::thread::Builder::new()
                    .name("stadesk-decoder".to_owned())
                    .spawn(move || run_decoder(decode_commands, frame_sender, decoder_control))
                    .map_err(|_| SessionError::Internal)?;
                StreamTarget::Decoder(decode_sender)
            }
            Delivery::Packets(packets) => StreamTarget::Packets(packets),
        };

        let finished = tokio::spawn(async move {
            let outgoing = async {
                while let Some(message) = control_receiver.recv().await {
                    let closing = message == ControllerMessage::Bye;
                    send_message(&mut sender, &message).await?;
                    if closing {
                        return Ok::<Option<ByeReason>, SessionError>(Some(ByeReason::Normal));
                    }
                }
                Ok(Some(ByeReason::Normal))
            };
            let incoming = async {
                loop {
                    match receive_message(&mut receiver).await? {
                        Some(HostMessage::StreamStart {
                            codec,
                            width,
                            height,
                        }) => {
                            if codec != CODEC_VP9 {
                                return Err(SessionError::Protocol);
                            }
                            target.start(width, height);
                        }
                        Some(HostMessage::VideoFrame {
                            timestamp_ms,
                            keyframe,
                            data,
                        }) => target.packet(timestamp_ms, keyframe, data),
                        Some(HostMessage::Bye(reason)) => return Ok(Some(reason)),
                        None => return Ok(None),
                        Some(HostMessage::AccessResponse(_)) => return Err(SessionError::Protocol),
                    }
                }
            };
            let outcome = tokio::select! {
                result = outgoing => result,
                result = incoming => result,
            };
            drop(target);
            let _ = sender.close().await;
            match outcome {
                Ok(Some(reason)) => Ok(reason),
                Ok(None) => Err(SessionError::Closed),
                Err(error) => Err(error),
            }
        });

        Ok(Self {
            peer,
            control,
            frames,
            finished,
        })
    }

    pub fn peer(&self) -> &PublicIdentity {
        &self.peer
    }

    pub fn frames(&self) -> watch::Receiver<Option<VideoFrame>> {
        self.frames.clone()
    }

    pub fn send_input(&self, event: InputEvent) -> Result<(), SessionError> {
        self.control
            .send(ControllerMessage::Input(event))
            .map_err(|_| SessionError::Closed)
    }

    pub fn request_keyframe(&self) {
        let _ = self.control.send(ControllerMessage::KeyframeRequest);
    }

    pub fn is_finished(&self) -> bool {
        self.finished.is_finished()
    }

    pub async fn wait(self) -> Result<ByeReason, SessionError> {
        self.finished.await.map_err(|_| SessionError::Internal)?
    }

    pub async fn close(self) -> Result<ByeReason, SessionError> {
        let _ = self.control.send(ControllerMessage::Bye);
        self.wait().await
    }
}
