//! Shared editing of a project between editors, without a server. Every
//! editor keeps a full copy as a CRDT, and updates merge in any order.

mod model;
mod protocol;
mod review;
mod session;
mod track;

pub use model::{Author, Comment, Project, RemoteChanges, Suggestion, SuggestionKind, TrackedOp};
pub use protocol::{CommentIntent, Presence, Role};
pub use review::{ReviewFile, ReviewKind};
pub use session::{Event, Identity, LanBrowser, LanSession, Session};
pub use track::{Tracked, track};

/// A CRDT client id that no other editor of a project is likely to have.
pub fn random_client_id() -> u64 {
    let bytes = iroh::SecretKey::generate().to_bytes();
    u64::from_le_bytes(bytes[..8].try_into().unwrap()) & ((1 << 53) - 1)
}
