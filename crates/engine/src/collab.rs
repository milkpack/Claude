//! Sharing an open document with a room: real-time co-editing through [`vectorcraft_collab`].
//!
//! The frontend owns the socket and calls [`Collab::receive`] for each message that arrives and
//! [`Collab::tick`] once per frame, sending what both hand back. Each tick:
//! 1. publishes what the commands since the last tick changed (a drag in progress is shared live,
//!    and becomes one undo step when it ends);
//! 2. carries out the Undo/Redo the commands queued ([`crate::SharedHistory`]): they take back
//!    only this participant's own steps;
//! 3. brings in the others' changes, unless a drag or typing is in progress (they arrive the
//!    moment it ends, so a gesture never fights a remote edit);
//! 4. tells the others where our pointer is and what we have selected.
//!
//! Commands, tools, the MCP server and the control channel need no changes: they edit the
//! document as always.

use std::sync::Arc;

use vectorcraft_collab::{Connection, PeerState};
use vectorcraft_doc::{Document, NodeId};
use vectorcraft_geom::Point;

use crate::{DocState, Session, SharedHistory};

pub use vectorcraft_collab::{Clock, PEER_COLORS, PeerState as Peer, SharedDoc};

/// What a tick did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TickReport {
    /// The document changed from outside (others' edits, or Undo/Redo): redraw.
    pub changed: bool,
    /// The shared document is no longer open (closed): leave the room.
    pub closed: bool,
}

/// One document shared with one room.
#[derive(Debug)]
pub struct Collab {
    pub conn: Connection,
    /// The shared document ([`DocState::uid`]).
    pub doc_uid: u64,
    /// Shown to the others.
    pub name: String,
    /// The first sync is done: the room's document is (or became) this one.
    joined: bool,
    /// Publishes since the last [`SharedDoc::end_step`]: an undo step is still open.
    step_open: bool,
}

impl Collab {
    pub fn new(conn: Connection, doc_uid: u64, name: impl Into<String>) -> Self {
        Self { conn, doc_uid, name: name.into(), joined: false, step_open: false }
    }

    /// Whether the room's document arrived and is the one being edited.
    pub fn is_joined(&self) -> bool {
        self.joined
    }

    /// The first messages on a newly opened (or reopened) socket.
    pub fn on_connect(&mut self) -> Vec<Vec<u8>> {
        self.conn.hello()
    }

    /// Handle one message from the server → replies to send.
    pub fn receive(&mut self, msg: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        self.conn.receive(msg).map_err(|e| e.to_string())
    }

    /// Everyone else in the room.
    pub fn peers(&self) -> Vec<(u64, PeerState)> {
        self.conn.peers()
    }

    /// Our colour (the others see our pointer and selection in it).
    pub fn color(&self) -> [u8; 3] {
        self.conn.color()
    }

    /// Sync the shared document with the room; `pointer` is ours in document coordinates while it
    /// is over the canvas. → what happened, and the messages to send.
    pub fn tick(&mut self, s: &mut Session, pointer: Option<Point>) -> (TickReport, Vec<Vec<u8>>) {
        let tool = s.tool_id().to_string();
        let mut report = TickReport::default();
        let Some(st) = s.document_mut(self.doc_uid) else {
            report.closed = true;
            return (report, self.conn.goodbye().into_iter().collect());
        };
        if !self.joined {
            if !self.conn.is_synced() {
                return (report, self.conn.outgoing());
            }
            self.join(st);
            report.changed = true;
        }

        // 1. Our edits.
        let shared = self.conn.doc.synced().cloned();
        if shared.as_ref().is_none_or(|d| !Arc::ptr_eq(d, &st.doc)) {
            self.step_open |= self.conn.doc.publish(&st.doc);
        }
        if st.interaction.is_none() && self.step_open {
            self.conn.doc.end_step();
            self.step_open = false;
        }

        // 2. Undo/Redo.
        let requests = st.shared_history.as_mut().map(|h| std::mem::take(&mut h.requests)).unwrap_or_default();
        for r in requests {
            match r {
                crate::HistoryRequest::Undo => self.conn.doc.undo(),
                crate::HistoryRequest::Redo => self.conn.doc.redo(),
            };
        }

        // 3. Theirs.
        if st.interaction.is_none()
            && self.conn.doc.has_remote_changes()
            && let Some(doc) = self.conn.doc.materialize(&st.doc)
        {
            adopt(st, doc);
            report.changed = true;
        }

        // The snapshots aren't used while shared (Undo goes through the room).
        st.history.undo.clear();
        st.history.redo.clear();
        if let Some(h) = &mut st.shared_history {
            h.can_undo = self.conn.doc.can_undo() || self.step_open;
            h.can_redo = self.conn.doc.can_redo();
        }

        // 4. Presence.
        let state = PeerState {
            name: self.name.clone(),
            color: self.conn.color(),
            cursor: pointer.filter(|p| p.x.is_finite() && p.y.is_finite()).map(|p| [p.x, p.y]),
            selection: st.selection.objects.iter().map(|id| id.0).collect(),
            tool,
        };
        self.conn.set_presence(&state);
        (report, self.conn.outgoing())
    }

    /// Stop sharing: the document stays open with its own Undo history from here on. → the
    /// message telling the others we left.
    pub fn leave(&mut self, s: &mut Session) -> Option<Vec<u8>> {
        if let Some(st) = s.document_mut(self.doc_uid) {
            st.shared_history = None;
            let mut doc = (*st.doc).clone();
            doc.set_id_range(None, 0);
            st.doc = Arc::new(doc);
        }
        self.conn.goodbye()
    }

    /// The first sync: share our document with an empty room, or take the room's.
    fn join(&mut self, st: &mut DocState) {
        self.joined = true;
        st.shared_history = Some(SharedHistory::default());
        if self.conn.doc.is_empty() {
            let mut doc: Document = (*st.doc).clone();
            doc.set_id_range(Some(self.conn.doc.id_range()), 0);
            st.doc = Arc::new(doc);
            self.conn.doc.publish(&st.doc);
            self.conn.doc.end_step();
        } else {
            // Whatever was in progress belongs to the document being replaced.
            st.interaction = None;
            if let Some(doc) = self.conn.doc.materialize(&st.doc) {
                adopt(st, doc);
            }
        }
        st.history.undo.clear();
        st.history.redo.clear();
    }
}

/// Show `doc` (from the room) in `st`, keeping what still applies of the selection and context.
fn adopt(st: &mut DocState, doc: Arc<Document>) {
    st.doc = doc;
    st.selection.prune(&st.doc);
    let exists = |id: Option<NodeId>, d: &Document| id.is_some_and(|i| d.node(i).is_some());
    if !exists(st.active_layer, &st.doc) {
        st.active_layer = st.doc.default_layer();
    }
    if st.isolation.is_some() && !exists(st.isolation, &st.doc) {
        st.isolation = None;
    }
    if st.mask_view.is_some() && !exists(st.mask_view, &st.doc) {
        st.mask_view = None;
    }
    st.revision += 1;
}
