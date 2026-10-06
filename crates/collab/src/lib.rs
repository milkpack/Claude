//! Real-time collaborative editing for VectorCraft.
//!
//! Several people edit one document at once, as in Figma: every change appears for everyone
//! within a frame, nobody locks anything, and edits made at the same time merge.
//!
//! - [`SharedDoc`] mirrors a [`vectorcraft_doc::Document`] in a Yjs CRDT (via `yrs`): local edits
//!   are published as fine-grained changes (per object, per field), remote ones rebuild only the
//!   subtrees they touch. Undo takes back only your own changes.
//! - [`Connection`] speaks the standard Yjs sync and awareness protocol over any transport, so the
//!   server (`apps/vectorcraft-collab-server`) is a plain Yjs relay with storage, and presence
//!   (names, pointers, selections) travels as Yjs awareness.
//!
//! Object ids: each participant allocates from its own range ([`SharedDoc::id_range`], see
//! `Document::set_id_range`), so objects drawn at the same moment on two machines never collide.
#![forbid(unsafe_code)]

mod mirror;
pub mod order;
mod protocol;

pub use mirror::{CollabError, SharedDoc};
pub use protocol::{Connection, PEER_COLORS, PeerState};
pub use yrs::sync::time::Clock;

#[cfg(test)]
mod tests;
