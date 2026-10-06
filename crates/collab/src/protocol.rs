//! The Yjs sync protocol ("y-protocols": sync steps 1/2, updates, awareness) over any message
//! transport, so VectorCraft talks to the collaboration server, and to any Yjs client on it.
//!
//! [`Connection`] is transport-agnostic: feed it each binary message that arrives
//! ([`Connection::receive`]) and send what it hands back and what [`Connection::outgoing`]
//! collects. The UI owns the socket (a thread with a WebSocket on desktop, the browser's
//! WebSocket on the web).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use yrs::sync::awareness::{Awareness, AwarenessUpdate};
use yrs::sync::time::Clock;
use yrs::sync::{Message, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;

use crate::mirror::{CollabError, SharedDoc};

/// What a participant shows the others: who they are, where their pointer is and what they
/// have selected. Sent as the Yjs awareness state (JSON).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PeerState {
    pub name: String,
    /// sRGB.
    pub color: [u8; 3],
    /// Pointer in document coordinates, while it is over the canvas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<[f64; 2]>,
    /// Selected object ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selection: Vec<u64>,
    /// Active tool id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool: String,
}

/// Re-announce our state this often (ms) so peers know we're still here…
const HEARTBEAT_MS: u64 = 15_000;
/// …and forget peers silent for this long (they lost their connection without saying goodbye).
const PEER_TIMEOUT_MS: u64 = 30_000;

/// Colours for participants, picked by client id: saturated enough to read on art, distinct from
/// the default selection blue.
pub const PEER_COLORS: [[u8; 3]; 10] = [
    [0xe8, 0x57, 0x3f],
    [0x2b, 0xa8, 0x4a],
    [0x8e, 0x44, 0xd8],
    [0xf0, 0x9a, 0x18],
    [0x14, 0x9c, 0xb8],
    [0xd8, 0x3a, 0x8c],
    [0x6a, 0x8f, 0x1a],
    [0x3a, 0x5c, 0xd8],
    [0xb8, 0x6b, 0x2a],
    [0x1a, 0x8f, 0x7a],
];

/// A replica's link to the room: the shared document plus everyone's presence.
pub struct Connection {
    pub doc: SharedDoc,
    awareness: Awareness,
    clock: Arc<dyn Clock>,
    /// The server sent its state: the shared document is complete.
    synced: bool,
    presence_dirty: bool,
    last_announce: u64,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection").field("doc", &self.doc).field("synced", &self.synced).finish()
    }
}

impl Connection {
    /// `clock` gives milliseconds (any epoch, but monotonic enough for heartbeats).
    pub fn new(doc: SharedDoc, clock: impl Clock + 'static) -> Self {
        let clock: Arc<dyn Clock> = Arc::new(clock);
        let c = clock.clone();
        let awareness = Awareness::with_clock(doc.ydoc().clone(), move || c.now());
        Self { doc, awareness, clock, synced: false, presence_dirty: true, last_announce: 0 }
    }

    pub fn client_id(&self) -> u64 {
        self.doc.client_id()
    }

    /// Whether the server's state has arrived (until then the shared document may be partial).
    pub fn is_synced(&self) -> bool {
        self.synced
    }

    /// The colour for this participant.
    pub fn color(&self) -> [u8; 3] {
        PEER_COLORS[(self.client_id() % PEER_COLORS.len() as u64) as usize]
    }

    /// The first messages on a (re)opened connection: our state vector (the server answers with
    /// what we lack, and asks for what it lacks) and our presence.
    pub fn hello(&mut self) -> Vec<Vec<u8>> {
        self.synced = false;
        let sv = yrs::StateVector::decode_v1(&self.doc.state_vector()).unwrap_or_default();
        let mut out = vec![Message::Sync(SyncMessage::SyncStep1(sv)).encode_v1()];
        self.presence_dirty = true;
        out.extend(self.presence_message());
        out
    }

    /// Handle one message from the server → the replies to send back.
    pub fn receive(&mut self, data: &[u8]) -> Result<Vec<Vec<u8>>, CollabError> {
        let msg = Message::decode_v1(data).map_err(|e| CollabError::BadUpdate(e.to_string()))?;
        let mut replies = vec![];
        match msg {
            Message::Sync(SyncMessage::SyncStep1(sv)) => {
                replies.push(Message::Sync(SyncMessage::SyncStep2(self.doc.diff(&sv))).encode_v1());
            }
            Message::Sync(SyncMessage::SyncStep2(update)) => {
                self.doc.apply_update(&update)?;
                self.synced = true;
            }
            Message::Sync(SyncMessage::Update(update)) => self.doc.apply_update(&update)?,
            Message::Awareness(update) => {
                self.awareness.apply_update(update).map_err(|e| CollabError::Other(e.to_string()))?;
            }
            Message::AwarenessQuery => {
                self.presence_dirty = true;
                replies.extend(self.presence_message());
            }
            Message::Auth(Some(reason)) => return Err(CollabError::Other(format!("permission denied: {reason}"))),
            Message::Auth(None) | Message::Custom(..) => {}
        }
        Ok(replies)
    }

    /// Local document updates and presence changes to send, plus the periodic heartbeat. Also
    /// forgets peers that went silent.
    pub fn outgoing(&mut self) -> Vec<Vec<u8>> {
        let mut out: Vec<Vec<u8>> = self.doc.take_outgoing().into_iter().map(|u| Message::Sync(SyncMessage::Update(u)).encode_v1()).collect();
        let now = self.clock.now();
        if now.saturating_sub(self.last_announce) > HEARTBEAT_MS {
            self.presence_dirty = true;
        }
        out.extend(self.presence_message());
        let me = self.awareness.client_id();
        let stale: Vec<_> =
            self.awareness.iter().filter(|(id, s)| *id != me && now.saturating_sub(s.last_updated) > PEER_TIMEOUT_MS).map(|(id, _)| id).collect();
        for id in stale {
            self.awareness.remove_state(id);
        }
        out
    }

    /// Set what the others see of us (sent by the next [`Connection::outgoing`] if it changed).
    pub fn set_presence(&mut self, state: &PeerState) {
        if self.awareness.local_state::<PeerState>().as_ref() == Some(state) {
            return;
        }
        if self.awareness.set_local_state(state).is_ok() {
            self.presence_dirty = true;
        }
    }

    /// Everyone else in the room, by client id.
    pub fn peers(&self) -> Vec<(u64, PeerState)> {
        let me = self.awareness.client_id();
        let mut out: Vec<(u64, PeerState)> = self
            .awareness
            .iter()
            .filter(|(id, _)| *id != me)
            .filter_map(|(id, s)| Some((id.get(), serde_json::from_str(s.data.as_deref()?).ok()?)))
            .collect();
        out.sort_by_key(|(id, _)| *id);
        out
    }

    /// The message that tells the others we left (send it before closing the socket).
    pub fn goodbye(&mut self) -> Option<Vec<u8>> {
        self.awareness.clean_local_state();
        // `update()` skips clients without a state, which now includes us: name ourselves.
        let me = self.awareness.client_id();
        let update = self.awareness.update_with_clients([me]).ok()?;
        Some(Message::Awareness(update).encode_v1())
    }

    fn presence_message(&mut self) -> Option<Vec<u8>> {
        if !self.presence_dirty {
            return None;
        }
        let me = self.awareness.client_id();
        let update: AwarenessUpdate = self.awareness.update_with_clients([me]).ok()?;
        self.presence_dirty = false;
        self.last_announce = self.clock.now();
        Some(Message::Awareness(update).encode_v1())
    }
}
