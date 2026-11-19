use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use stadesk_core::wire::{Reader, Writer};
use stadesk_core::{
    CHALLENGE_LEN, CoreError, DUAL_SIGNATURE_LEN, PUBLIC_IDENTITY_LEN, PublicIdentity, StaDeskId,
};

pub const SESSION_TOKEN_LEN: usize = 16;
pub const MAX_CONTROL_FRAME_LEN: usize = 16 * 1024;

pub type SessionToken = [u8; SESSION_TOKEN_LEN];

const ADDRESS_V4: u8 = 4;
const ADDRESS_V6: u8 = 6;

const CLIENT_REGISTER: u8 = 1;
const CLIENT_PROOF: u8 = 2;
const CLIENT_CONNECT: u8 = 3;
const CLIENT_RELAY_JOIN: u8 = 4;

const SERVER_CHALLENGE: u8 = 1;
const SERVER_REGISTERED: u8 = 2;
const SERVER_REGISTRATION_REJECTED: u8 = 3;
const SERVER_INCOMING_REQUEST: u8 = 4;
const SERVER_PEER_FOUND: u8 = 5;
const SERVER_PEER_OFFLINE: u8 = 6;
const SERVER_RATE_LIMITED: u8 = 7;
const SERVER_RELAY_READY: u8 = 8;
const SERVER_RELAY_REJECTED: u8 = 9;

const ROLE_CONTROLLER: u8 = 1;
const ROLE_HOST: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelayRole {
    Controller,
    Host,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMessage {
    Register {
        identity: Box<PublicIdentity>,
    },
    Proof {
        signature: Vec<u8>,
    },
    Connect {
        target: StaDeskId,
    },
    RelayJoin {
        token: SessionToken,
        role: RelayRole,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMessage {
    Challenge {
        challenge: [u8; CHALLENGE_LEN],
    },
    Registered {
        observed: SocketAddr,
    },
    RegistrationRejected,
    IncomingRequest {
        token: SessionToken,
        controller: SocketAddr,
    },
    PeerFound {
        token: SessionToken,
        host: SocketAddr,
    },
    PeerOffline,
    RateLimited,
    RelayReady,
    RelayRejected,
}

fn write_address(writer: &mut Writer, address: &SocketAddr) {
    match address.ip() {
        IpAddr::V4(ip) => writer.u8(ADDRESS_V4).bytes(&ip.octets()),
        IpAddr::V6(ip) => writer.u8(ADDRESS_V6).bytes(&ip.octets()),
    };
    writer.u16(address.port());
}

fn read_address(reader: &mut Reader<'_>) -> Result<SocketAddr, CoreError> {
    let ip = match reader.u8()? {
        ADDRESS_V4 => IpAddr::V4(Ipv4Addr::from(reader.array::<4>()?)),
        ADDRESS_V6 => IpAddr::V6(Ipv6Addr::from(reader.array::<16>()?)),
        _ => return Err(CoreError::Malformed),
    };
    Ok(SocketAddr::new(ip, reader.u16()?))
}

fn read_id(reader: &mut Reader<'_>) -> Result<StaDeskId, CoreError> {
    let length = usize::from(reader.u8()?);
    let text = std::str::from_utf8(reader.slice(length)?).map_err(|_| CoreError::Malformed)?;
    StaDeskId::parse(text)
}

impl ClientMessage {
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::with_capacity(PUBLIC_IDENTITY_LEN + DUAL_SIGNATURE_LEN);
        match self {
            Self::Register { identity } => {
                writer.u8(CLIENT_REGISTER).bytes(identity.encode());
            }
            Self::Proof { signature } => {
                writer.u8(CLIENT_PROOF).bytes(signature);
            }
            Self::Connect { target } => {
                let text = target.to_string();
                writer
                    .u8(CLIENT_CONNECT)
                    .u8(text.len() as u8)
                    .bytes(text.as_bytes());
            }
            Self::RelayJoin { token, role } => {
                let role = match role {
                    RelayRole::Controller => ROLE_CONTROLLER,
                    RelayRole::Host => ROLE_HOST,
                };
                writer.u8(CLIENT_RELAY_JOIN).bytes(token).u8(role);
            }
        }
        writer.finish()
    }

    pub fn decode(input: &[u8]) -> Result<Self, CoreError> {
        let mut reader = Reader::new(input);
        let message = match reader.u8()? {
            CLIENT_REGISTER => Self::Register {
                identity: Box::new(PublicIdentity::decode(reader.slice(PUBLIC_IDENTITY_LEN)?)?),
            },
            CLIENT_PROOF => Self::Proof {
                signature: reader.slice(DUAL_SIGNATURE_LEN)?.to_vec(),
            },
            CLIENT_CONNECT => Self::Connect {
                target: read_id(&mut reader)?,
            },
            CLIENT_RELAY_JOIN => Self::RelayJoin {
                token: reader.array::<SESSION_TOKEN_LEN>()?,
                role: match reader.u8()? {
                    ROLE_CONTROLLER => RelayRole::Controller,
                    ROLE_HOST => RelayRole::Host,
                    _ => return Err(CoreError::Malformed),
                },
            },
            _ => return Err(CoreError::Malformed),
        };
        reader.finish()?;
        Ok(message)
    }
}

impl ServerMessage {
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::with_capacity(64);
        match self {
            Self::Challenge { challenge } => {
                writer.u8(SERVER_CHALLENGE).bytes(challenge);
            }
            Self::Registered { observed } => {
                writer.u8(SERVER_REGISTERED);
                write_address(&mut writer, observed);
            }
            Self::RegistrationRejected => {
                writer.u8(SERVER_REGISTRATION_REJECTED);
            }
            Self::IncomingRequest { token, controller } => {
                writer.u8(SERVER_INCOMING_REQUEST).bytes(token);
                write_address(&mut writer, controller);
            }
            Self::PeerFound { token, host } => {
                writer.u8(SERVER_PEER_FOUND).bytes(token);
                write_address(&mut writer, host);
            }
            Self::PeerOffline => {
                writer.u8(SERVER_PEER_OFFLINE);
            }
            Self::RateLimited => {
                writer.u8(SERVER_RATE_LIMITED);
            }
            Self::RelayReady => {
                writer.u8(SERVER_RELAY_READY);
            }
            Self::RelayRejected => {
                writer.u8(SERVER_RELAY_REJECTED);
            }
        }
        writer.finish()
    }

    pub fn decode(input: &[u8]) -> Result<Self, CoreError> {
        let mut reader = Reader::new(input);
        let message = match reader.u8()? {
            SERVER_CHALLENGE => Self::Challenge {
                challenge: reader.array::<CHALLENGE_LEN>()?,
            },
            SERVER_REGISTERED => Self::Registered {
                observed: read_address(&mut reader)?,
            },
            SERVER_REGISTRATION_REJECTED => Self::RegistrationRejected,
            SERVER_INCOMING_REQUEST => Self::IncomingRequest {
                token: reader.array::<SESSION_TOKEN_LEN>()?,
                controller: read_address(&mut reader)?,
            },
            SERVER_PEER_FOUND => Self::PeerFound {
                token: reader.array::<SESSION_TOKEN_LEN>()?,
                host: read_address(&mut reader)?,
            },
            SERVER_PEER_OFFLINE => Self::PeerOffline,
            SERVER_RATE_LIMITED => Self::RateLimited,
            SERVER_RELAY_READY => Self::RelayReady,
            SERVER_RELAY_REJECTED => Self::RelayRejected,
            _ => return Err(CoreError::Malformed),
        };
        reader.finish()?;
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stadesk_core::Identity;

    #[test]
    fn client_messages_round_trip() {
        let identity = Identity::generate().unwrap();
        for message in [
            ClientMessage::Register {
                identity: Box::new(identity.public().clone()),
            },
            ClientMessage::Proof {
                signature: vec![7u8; DUAL_SIGNATURE_LEN],
            },
            ClientMessage::Connect {
                target: identity.id(),
            },
            ClientMessage::RelayJoin {
                token: [3u8; SESSION_TOKEN_LEN],
                role: RelayRole::Controller,
            },
            ClientMessage::RelayJoin {
                token: [4u8; SESSION_TOKEN_LEN],
                role: RelayRole::Host,
            },
        ] {
            assert_eq!(ClientMessage::decode(&message.encode()).unwrap(), message);
        }
    }

    #[test]
    fn server_messages_round_trip_with_both_address_families() {
        let v4: SocketAddr = "203.0.113.7:21300".parse().unwrap();
        let v6: SocketAddr = "[2001:db8::1]:443".parse().unwrap();
        for message in [
            ServerMessage::Challenge {
                challenge: [1u8; CHALLENGE_LEN],
            },
            ServerMessage::Registered { observed: v4 },
            ServerMessage::Registered { observed: v6 },
            ServerMessage::RegistrationRejected,
            ServerMessage::IncomingRequest {
                token: [2u8; SESSION_TOKEN_LEN],
                controller: v6,
            },
            ServerMessage::PeerFound {
                token: [5u8; SESSION_TOKEN_LEN],
                host: v4,
            },
            ServerMessage::PeerOffline,
            ServerMessage::RateLimited,
            ServerMessage::RelayReady,
            ServerMessage::RelayRejected,
        ] {
            assert_eq!(ServerMessage::decode(&message.encode()).unwrap(), message);
        }
    }

    #[test]
    fn malformed_messages_are_rejected() {
        assert!(ClientMessage::decode(&[]).is_err());
        assert!(ClientMessage::decode(&[99]).is_err());
        assert!(ClientMessage::decode(&[CLIENT_PROOF, 1, 2]).is_err());
        assert!(ClientMessage::decode(&[CLIENT_CONNECT, 3, b'A', b'B', b'C']).is_err());
        let mut join = ClientMessage::RelayJoin {
            token: [0u8; SESSION_TOKEN_LEN],
            role: RelayRole::Host,
        }
        .encode();
        *join.last_mut().unwrap() = 9;
        assert!(ClientMessage::decode(&join).is_err());
        assert!(ServerMessage::decode(&[SERVER_REGISTERED, 5, 0, 0]).is_err());
        assert!(ServerMessage::decode(&[SERVER_PEER_OFFLINE, 0]).is_err());
        assert!(ServerMessage::decode(&[42]).is_err());
    }
}
