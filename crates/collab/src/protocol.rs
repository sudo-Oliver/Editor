//! What editors send each other: one stream per connection, made of frames
//! with a kind byte, a length and a payload.

use std::ops::Range;

use iroh::{
    EndpointId,
    endpoint::{RecvStream, SendStream},
};
use serde::{Deserialize, Serialize};

pub const ALPN: &[u8] = b"editor/collab/1";

/// Larger frames come only from a broken or hostile peer.
pub(crate) const MAX_FRAME: usize = 64 * 1024 * 1024;

const SECRET_LENGTH: usize = 16;

/// What someone may do in a session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    #[default]
    Editor,
    /// Suggests changes and comments, while the text stays as it is.
    Reviewer,
    /// Reads and comments.
    Reader,
}

impl Role {
    pub const ALL: [Role; 3] = [Role::Editor, Role::Reviewer, Role::Reader];

    fn byte(self) -> u8 {
        match self {
            Role::Editor => 0,
            Role::Reviewer => 1,
            Role::Reader => 2,
        }
    }

    fn from_byte(byte: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.byte() == byte)
    }

    pub fn label(self) -> &'static str {
        match self {
            Role::Editor => "Editor",
            Role::Reviewer => "Reviewer",
            Role::Reader => "Reader",
        }
    }
}

/// A comment from a reader, whom the host lets comment without editing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommentIntent {
    Add { path: String, range: Range<usize>, text: String, parent: Option<String> },
    Resolve { id: String, resolved: bool },
}

/// Where someone is in the project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    pub peer: u64,
    pub name: String,
    pub color: u32,
    /// Set by the host, so that nobody can claim more than they have.
    #[serde(default)]
    pub role: Role,
    pub path: String,
    pub anchor: usize,
    pub head: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Hello {
    /// Missing when joining from the local network, where the host decides.
    pub secret: Option<Vec<u8>>,
    pub name: String,
    pub color: u32,
}

#[derive(Debug)]
pub enum Frame {
    Hello(Hello),
    /// The id the host gave the guest, and what the guest may do.
    Welcome(u64, Role),
    Denied,
    /// A state vector, answered with an update holding what it lacks.
    Sync(Vec<u8>),
    Update(Vec<u8>),
    Presence(Presence),
    Left(u64),
    Intent(CommentIntent),
}

impl Frame {
    fn encode(&self) -> Vec<u8> {
        let (kind, payload) = match self {
            Frame::Hello(hello) => (1, serde_json::to_vec(hello).unwrap()),
            Frame::Welcome(peer, role) => (2, peer.to_be_bytes().into_iter().chain([role.byte()]).collect()),
            Frame::Denied => (3, Vec::new()),
            Frame::Sync(state) => (4, state.clone()),
            Frame::Update(update) => (5, update.clone()),
            Frame::Presence(presence) => (6, serde_json::to_vec(presence).unwrap()),
            Frame::Left(peer) => (7, peer.to_be_bytes().to_vec()),
            Frame::Intent(intent) => (8, serde_json::to_vec(intent).unwrap()),
        };
        let mut bytes = Vec::with_capacity(5 + payload.len());
        bytes.push(kind);
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&payload);
        bytes
    }

    fn decode(kind: u8, payload: Vec<u8>) -> Result<Self, String> {
        let peer = |payload: &[u8]| payload.try_into().map(u64::from_be_bytes).map_err(|_| "Invalid peer id".to_string());
        Ok(match kind {
            1 => Frame::Hello(serde_json::from_slice(&payload).map_err(|error| error.to_string())?),
            2 if payload.len() == 9 => Frame::Welcome(peer(&payload[..8])?, Role::from_byte(payload[8]).ok_or("Unknown role")?),
            3 => Frame::Denied,
            4 => Frame::Sync(payload),
            5 => Frame::Update(payload),
            6 => Frame::Presence(serde_json::from_slice(&payload).map_err(|error| error.to_string())?),
            7 => Frame::Left(peer(&payload)?),
            8 => Frame::Intent(serde_json::from_slice(&payload).map_err(|error| error.to_string())?),
            _ => return Err(format!("Unknown message {kind}")),
        })
    }
}

pub async fn write_frame(send: &mut SendStream, frame: &Frame) -> Result<(), String> {
    send.write_all(&frame.encode()).await.map_err(|error| error.to_string())
}

pub async fn read_frame(recv: &mut RecvStream) -> Result<Frame, String> {
    let mut header = [0u8; 5];
    recv.read_exact(&mut header).await.map_err(|error| error.to_string())?;
    let length = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
    if length > MAX_FRAME {
        return Err("Message too large".to_string());
    }
    let mut payload = vec![0u8; length];
    recv.read_exact(&mut payload).await.map_err(|error| error.to_string())?;
    Frame::decode(header[0], payload)
}

/// An invitation: who hosts, and the secret that lets a guest in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub host: EndpointId,
    pub secret: [u8; SECRET_LENGTH],
}

impl Ticket {
    pub fn new(host: EndpointId) -> Self {
        // A fresh key comes from the system's secure random source.
        let random = iroh::SecretKey::generate().to_bytes();
        Self {
            host,
            secret: random[..SECRET_LENGTH].try_into().unwrap(),
        }
    }

    pub fn encode(&self) -> String {
        let mut bytes = self.host.as_bytes().to_vec();
        bytes.extend_from_slice(&self.secret);
        data_encoding::BASE32_NOPAD.encode(&bytes).to_lowercase()
    }

    /// Accepts the code with any case, spaces or dashes, as people paste it.
    pub fn decode(code: &str) -> Result<Self, String> {
        let invalid = || "This is not a valid invite code".to_string();
        let cleaned: String = code.chars().filter(|ch| ch.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
        let bytes = data_encoding::BASE32_NOPAD.decode(cleaned.as_bytes()).map_err(|_| invalid())?;
        if bytes.len() != 32 + SECRET_LENGTH {
            return Err(invalid());
        }
        let host = EndpointId::from_bytes(bytes[..32].try_into().unwrap()).map_err(|_| invalid())?;
        Ok(Self {
            host,
            secret: bytes[32..].try_into().unwrap(),
        })
    }
}

/// Compares secrets in constant time, so that timing tells nothing.
pub fn same_secret(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |difference, (x, y)| difference | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_survives_how_people_paste_it() {
        let ticket = Ticket::new(iroh::SecretKey::generate().public());
        let code = ticket.encode();
        assert_eq!(Ticket::decode(&code).unwrap(), ticket);
        let messy = format!("  {}-{} ", code[..10].to_uppercase(), &code[10..]);
        assert_eq!(Ticket::decode(&messy).unwrap(), ticket);
        assert!(Ticket::decode("abc").is_err());
    }

    #[test]
    fn frames_round_trip() {
        let frame = Frame::Presence(Presence {
            peer: 3,
            name: "Ada".into(),
            color: 0xff0000,
            role: Role::Reviewer,
            path: "main.tex".into(),
            anchor: 1,
            head: 4,
        });
        let bytes = frame.encode();
        let Frame::Presence(presence) = Frame::decode(bytes[0], bytes[5..].to_vec()).unwrap() else {
            panic!("wrong kind");
        };
        assert_eq!(presence.name, "Ada");
        assert!(same_secret(b"abc", b"abc"));
        assert!(!same_secret(b"abc", b"abd"));
    }

    #[test]
    fn roles_travel_with_the_welcome() {
        for role in Role::ALL {
            let bytes = Frame::Welcome(7, role).encode();
            let Frame::Welcome(7, decoded) = Frame::decode(bytes[0], bytes[5..].to_vec()).unwrap() else {
                panic!("wrong frame");
            };
            assert_eq!(decoded, role);
        }
    }
}

