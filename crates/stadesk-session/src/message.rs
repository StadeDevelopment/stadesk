use stadesk_core::wire::{Reader, Writer};
use stadesk_core::{AccessRequest, AccessResponse, CoreError};
use stadesk_desktop::{InputEvent, PointerButton};

const ACCESS_REQUEST: u8 = 0x01;
const INPUT: u8 = 0x02;
const KEYFRAME_REQUEST: u8 = 0x03;
const CONTROLLER_BYE: u8 = 0x04;

const ACCESS_RESPONSE: u8 = 0x81;
const STREAM_START: u8 = 0x82;
const VIDEO_FRAME: u8 = 0x83;
const HOST_BYE: u8 = 0x84;

const INPUT_POINTER_MOVE: u8 = 1;
const INPUT_POINTER_BUTTON: u8 = 2;
const INPUT_SCROLL: u8 = 3;
const INPUT_KEY: u8 = 4;

const BUTTON_LEFT: u8 = 0;
const BUTTON_RIGHT: u8 = 1;
const BUTTON_MIDDLE: u8 = 2;
const BUTTON_BACK: u8 = 3;
const BUTTON_FORWARD: u8 = 4;

pub const CODEC_VP9: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByeReason {
    Normal,
    AccessDenied,
    HostStopped,
    CaptureFailed,
    ProtocolError,
}

impl ByeReason {
    fn code(self) -> u8 {
        match self {
            Self::Normal => 0,
            Self::AccessDenied => 1,
            Self::HostStopped => 2,
            Self::CaptureFailed => 3,
            Self::ProtocolError => 4,
        }
    }

    fn from_code(code: u8) -> Result<Self, CoreError> {
        Ok(match code {
            0 => Self::Normal,
            1 => Self::AccessDenied,
            2 => Self::HostStopped,
            3 => Self::CaptureFailed,
            4 => Self::ProtocolError,
            _ => return Err(CoreError::Malformed),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerMessage {
    AccessRequest(AccessRequest),
    Input(InputEvent),
    KeyframeRequest,
    Bye,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostMessage {
    AccessResponse(AccessResponse),
    StreamStart {
        codec: u8,
        width: u32,
        height: u32,
    },
    VideoFrame {
        timestamp_ms: i64,
        keyframe: bool,
        data: Vec<u8>,
    },
    Bye(ByeReason),
}

fn write_i32(writer: &mut Writer, value: i32) {
    writer.u32(value as u32);
}

fn read_i32(reader: &mut Reader<'_>) -> Result<i32, CoreError> {
    Ok(reader.u32()? as i32)
}

fn button_code(button: PointerButton) -> u8 {
    match button {
        PointerButton::Left => BUTTON_LEFT,
        PointerButton::Right => BUTTON_RIGHT,
        PointerButton::Middle => BUTTON_MIDDLE,
        PointerButton::Back => BUTTON_BACK,
        PointerButton::Forward => BUTTON_FORWARD,
    }
}

fn button_from_code(code: u8) -> Result<PointerButton, CoreError> {
    Ok(match code {
        BUTTON_LEFT => PointerButton::Left,
        BUTTON_RIGHT => PointerButton::Right,
        BUTTON_MIDDLE => PointerButton::Middle,
        BUTTON_BACK => PointerButton::Back,
        BUTTON_FORWARD => PointerButton::Forward,
        _ => return Err(CoreError::Malformed),
    })
}

fn read_bool(reader: &mut Reader<'_>) -> Result<bool, CoreError> {
    match reader.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(CoreError::Malformed),
    }
}

fn write_input(writer: &mut Writer, event: &InputEvent) {
    match *event {
        InputEvent::PointerMove { x, y } => {
            writer.u8(INPUT_POINTER_MOVE).u32(x).u32(y);
        }
        InputEvent::PointerButton { button, pressed } => {
            writer
                .u8(INPUT_POINTER_BUTTON)
                .u8(button_code(button))
                .u8(u8::from(pressed));
        }
        InputEvent::Scroll {
            horizontal,
            vertical,
        } => {
            writer.u8(INPUT_SCROLL);
            write_i32(writer, horizontal);
            write_i32(writer, vertical);
        }
        InputEvent::Key { usage, pressed } => {
            writer.u8(INPUT_KEY).u16(usage).u8(u8::from(pressed));
        }
    }
}

fn read_input(reader: &mut Reader<'_>) -> Result<InputEvent, CoreError> {
    Ok(match reader.u8()? {
        INPUT_POINTER_MOVE => InputEvent::PointerMove {
            x: reader.u32()?,
            y: reader.u32()?,
        },
        INPUT_POINTER_BUTTON => InputEvent::PointerButton {
            button: button_from_code(reader.u8()?)?,
            pressed: read_bool(reader)?,
        },
        INPUT_SCROLL => InputEvent::Scroll {
            horizontal: read_i32(reader)?,
            vertical: read_i32(reader)?,
        },
        INPUT_KEY => InputEvent::Key {
            usage: reader.u16()?,
            pressed: read_bool(reader)?,
        },
        _ => return Err(CoreError::Malformed),
    })
}

impl ControllerMessage {
    pub fn encode(&self) -> Result<Vec<u8>, CoreError> {
        let mut writer = Writer::with_capacity(32);
        match self {
            Self::AccessRequest(request) => {
                writer.u8(ACCESS_REQUEST).bytes(&request.encode()?);
            }
            Self::Input(event) => {
                writer.u8(INPUT);
                write_input(&mut writer, event);
            }
            Self::KeyframeRequest => {
                writer.u8(KEYFRAME_REQUEST);
            }
            Self::Bye => {
                writer.u8(CONTROLLER_BYE);
            }
        }
        Ok(writer.finish())
    }

    pub fn decode(input: &[u8]) -> Result<Self, CoreError> {
        let mut reader = Reader::new(input);
        let message = match reader.u8()? {
            ACCESS_REQUEST => Self::AccessRequest(AccessRequest::decode(reader.rest())?),
            INPUT => Self::Input(read_input(&mut reader)?),
            KEYFRAME_REQUEST => Self::KeyframeRequest,
            CONTROLLER_BYE => Self::Bye,
            _ => return Err(CoreError::Malformed),
        };
        reader.finish()?;
        Ok(message)
    }
}

impl HostMessage {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::AccessResponse(response) => {
                let mut writer = Writer::with_capacity(8);
                writer.u8(ACCESS_RESPONSE).bytes(&response.encode());
                writer.finish()
            }
            Self::StreamStart {
                codec,
                width,
                height,
            } => {
                let mut writer = Writer::with_capacity(10);
                writer.u8(STREAM_START).u8(*codec).u32(*width).u32(*height);
                writer.finish()
            }
            Self::VideoFrame {
                timestamp_ms,
                keyframe,
                data,
            } => {
                let mut writer = Writer::with_capacity(10 + data.len());
                writer
                    .u8(VIDEO_FRAME)
                    .bytes(&timestamp_ms.to_be_bytes())
                    .u8(u8::from(*keyframe))
                    .bytes(data);
                writer.finish()
            }
            Self::Bye(reason) => {
                let mut writer = Writer::with_capacity(2);
                writer.u8(HOST_BYE).u8(reason.code());
                writer.finish()
            }
        }
    }

    pub fn decode(input: &[u8]) -> Result<Self, CoreError> {
        let mut reader = Reader::new(input);
        let message = match reader.u8()? {
            ACCESS_RESPONSE => Self::AccessResponse(AccessResponse::decode(reader.rest())?),
            STREAM_START => Self::StreamStart {
                codec: reader.u8()?,
                width: reader.u32()?,
                height: reader.u32()?,
            },
            VIDEO_FRAME => Self::VideoFrame {
                timestamp_ms: i64::from_be_bytes(reader.array::<8>()?),
                keyframe: read_bool(&mut reader)?,
                data: reader.rest().to_vec(),
            },
            HOST_BYE => Self::Bye(ByeReason::from_code(reader.u8()?)?),
            _ => return Err(CoreError::Malformed),
        };
        reader.finish()?;
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_messages_round_trip() {
        let messages = [
            ControllerMessage::AccessRequest(AccessRequest::without_password()),
            ControllerMessage::AccessRequest(AccessRequest::with_password("şifre 1234")),
            ControllerMessage::Input(InputEvent::PointerMove { x: 1919, y: 0 }),
            ControllerMessage::Input(InputEvent::PointerButton {
                button: PointerButton::Forward,
                pressed: true,
            }),
            ControllerMessage::Input(InputEvent::Scroll {
                horizontal: -120,
                vertical: 360,
            }),
            ControllerMessage::Input(InputEvent::Key {
                usage: 0xe7,
                pressed: false,
            }),
            ControllerMessage::KeyframeRequest,
            ControllerMessage::Bye,
        ];
        for message in messages {
            let encoded = message.encode().unwrap();
            assert_eq!(ControllerMessage::decode(&encoded).unwrap(), message);
        }
    }

    #[test]
    fn host_messages_round_trip() {
        let messages = [
            HostMessage::AccessResponse(AccessResponse::LockedOut {
                retry_after_seconds: 30,
            }),
            HostMessage::StreamStart {
                codec: CODEC_VP9,
                width: 2940,
                height: 1912,
            },
            HostMessage::VideoFrame {
                timestamp_ms: -5,
                keyframe: true,
                data: vec![1, 2, 3],
            },
            HostMessage::VideoFrame {
                timestamp_ms: i64::MAX,
                keyframe: false,
                data: Vec::new(),
            },
            HostMessage::Bye(ByeReason::CaptureFailed),
        ];
        for message in messages {
            assert_eq!(HostMessage::decode(&message.encode()).unwrap(), message);
        }
    }

    #[test]
    fn malformed_messages_are_rejected() {
        assert!(ControllerMessage::decode(&[]).is_err());
        assert!(ControllerMessage::decode(&[0x7f]).is_err());
        assert!(ControllerMessage::decode(&[INPUT, 9]).is_err());
        assert!(ControllerMessage::decode(&[INPUT, INPUT_POINTER_BUTTON, 7, 1]).is_err());
        assert!(ControllerMessage::decode(&[INPUT, INPUT_KEY, 0, 4, 2]).is_err());
        assert!(ControllerMessage::decode(&[KEYFRAME_REQUEST, 0]).is_err());
        assert!(HostMessage::decode(&[STREAM_START, 1, 0, 0]).is_err());
        assert!(HostMessage::decode(&[VIDEO_FRAME, 0, 0, 0, 0, 0, 0, 0, 0, 3]).is_err());
        assert!(HostMessage::decode(&[HOST_BYE, 9]).is_err());
    }
}
