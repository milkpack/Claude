//! [`SharedDoc`]: a [`Document`] mirrored in a Yjs CRDT.
//!
//! Layout of the CRDT (three root maps):
//! - `nodes`: object id → map `{p: parent id ("" = top level), o: order key, <field>: JSON}`. Each
//!   top-level field of the serialized [`Node`] (`kind` with its children taken out, `appearance`,
//!   `name`, `opacity`…) is its own entry, so one person recolouring an object while another moves
//!   it merge instead of one overwriting the other.
//! - `doc`: document field (everything but the layer tree and images) → JSON.
//! - `images`: image key → `{mime, bytes, proxy?}` (the bytes serde leaves out of the JSON).
//!
//! Local edits are found by comparing the edited document with the last synced one. Documents
//! share structure (`Arc<Node>` subtrees), so an unchanged subtree costs one pointer comparison.
//! Remote edits rebuild only the subtrees they touched; the rest of the tree is reused as is.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde_json::{Map as JsonMap, Value};
use vectorcraft_doc::{Document, ImageBlob, Node, NodeId, NodeKind};
use yrs::types::{DeepObservable, Event, PathSegment};
use yrs::undo::Options as UndoOptions;
use yrs::updates::decoder::Decode;
use yrs::{Any, Doc, Map, MapPrelim, MapRef, Origin, Out, ReadTxn, StateVector, Transact, UndoManager, Update};

use crate::order;

/// Origin of transactions made by [`SharedDoc::publish`]: the only ones Undo takes back.
const LOCAL: &str = "vectorcraft-local";
/// Origin of updates applied from the network.
const REMOTE: &str = "vectorcraft-remote";

/// Ids each participant allocates from: `[slot << ID_SHIFT, (slot + 1) << ID_SHIFT)`, slot ≥ 1, all
/// below 2^53 so they survive a trip through a JavaScript number.
const ID_SHIFT: u32 = 32;
const ID_SLOTS: u64 = (1 << (53 - ID_SHIFT)) - 1;

const PARENT: &str = "p";
const ORDER: &str = "o";
/// Document fields kept elsewhere: the tree in `nodes`, images in `images`, and the id counter,
/// which is per participant.
const DOC_SKIP: [&str; 3] = ["layers", "images", "next_id"];

#[derive(Debug, thiserror::Error)]
pub enum CollabError {
    #[error("malformed update: {0}")]
    BadUpdate(String),
    #[error("{0}")]
    Other(String),
}

/// An object as the CRDT has it.
#[derive(Clone, Debug, Default)]
struct RawNode {
    parent: Option<NodeId>,
    order: String,
    /// Serialized fields (JSON) as last written or read.
    fields: BTreeMap<String, String>,
    /// The fields parsed (children left empty), when they parse.
    node: Option<Node>,
}

/// Keys touched since the last [`SharedDoc::materialize`], collected by CRDT observers.
#[derive(Default)]
struct Touched {
    nodes: HashSet<String>,
    doc: HashSet<String>,
    images: HashSet<String>,
    remote: bool,
}

/// The tree as last synced: what the next local edit is compared with.
#[derive(Default)]
struct Tree {
    arcs: HashMap<NodeId, Arc<Node>>,
    parent: HashMap<NodeId, Option<NodeId>>,
    children: HashMap<Option<NodeId>, Vec<NodeId>>,
}

impl Tree {
    fn of(doc: &Document) -> Tree {
        let mut t = Tree::default();
        fn walk(t: &mut Tree, parent: Option<NodeId>, list: &[Arc<Node>]) {
            t.children.insert(parent, list.iter().map(|n| n.id).collect());
            for n in list {
                t.arcs.insert(n.id, n.clone());
                t.parent.insert(n.id, parent);
                if let Some(kids) = container(n) {
                    walk(t, Some(n.id), kids);
                }
            }
        }
        walk(&mut t, None, &doc.layers);
        t
    }
}

/// The child list of a container (envelopes hold their content), `None` for leaves.
fn container(n: &Node) -> Option<&Vec<Arc<Node>>> {
    match &n.kind {
        NodeKind::Envelope { content, .. } => Some(content),
        _ => n.children(),
    }
}

fn container_mut(n: &mut Node) -> Option<&mut Vec<Arc<Node>>> {
    if matches!(n.kind, NodeKind::Envelope { .. }) {
        return match &mut n.kind {
            NodeKind::Envelope { content, .. } => Some(content),
            _ => None,
        };
    }
    n.children_mut()
}

/// The node's own fields, serialized, without its id and children.
fn fields_of(n: &Node) -> BTreeMap<String, String> {
    let mut shallow = n.clone();
    if let Some(kids) = container_mut(&mut shallow) {
        kids.clear();
    }
    let mut out = BTreeMap::new();
    if let Ok(Value::Object(m)) = serde_json::to_value(&shallow) {
        for (k, v) in m {
            if k != "id" && k != PARENT && k != ORDER {
                out.insert(k, v.to_string());
            }
        }
    }
    out
}

fn parse_node(id: NodeId, fields: &BTreeMap<String, String>) -> Option<Node> {
    let mut m = JsonMap::new();
    for (k, v) in fields {
        m.insert(k.clone(), serde_json::from_str(v).ok()?);
    }
    m.insert("id".into(), Value::from(id.0));
    match serde_json::from_value::<Node>(Value::Object(m)) {
        Ok(n) => Some(n),
        Err(e) => {
            log::warn!("collab: object {id} doesn't parse ({e}); keeping the last good version");
            None
        }
    }
}

fn id_key(id: NodeId) -> String {
    id.0.to_string()
}

fn parse_id(s: &str) -> Option<NodeId> {
    s.parse().ok().map(NodeId)
}

fn out_str(o: Option<Out>) -> Option<String> {
    match o {
        Some(Out::Any(Any::String(s))) => Some(s.to_string()),
        _ => None,
    }
}

fn out_bytes(o: Option<Out>) -> Option<Arc<Vec<u8>>> {
    match o {
        Some(Out::Any(Any::Buffer(b))) => Some(Arc::new(b.to_vec())),
        _ => None,
    }
}

/// A document shared through a CRDT.
pub struct SharedDoc {
    ydoc: Doc,
    nodes: MapRef,
    meta: MapRef,
    images: MapRef,
    undo: UndoManager,
    touched: Arc<Mutex<Touched>>,
    raw: HashMap<NodeId, RawNode>,
    /// Children per parent as the CRDT orders them: (order key, id).
    kids: HashMap<Option<NodeId>, BTreeSet<(String, NodeId)>>,
    doc_fields: BTreeMap<String, String>,
    image_data: BTreeMap<String, ImageBlob>,
    tree: Tree,
    synced: Option<Arc<Document>>,
    id_range: (u64, u64),
    next_id: u64,
    outbox: Vec<Vec<u8>>,
}

impl std::fmt::Debug for SharedDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedDoc").field("client", &self.client_id()).field("objects", &self.raw.len()).finish()
    }
}

impl Default for SharedDoc {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedDoc {
    /// A new, empty replica with a random client id.
    pub fn new() -> Self {
        Self::with_doc(Doc::new())
    }

    /// A replica with a fixed client id (< 2^53; tests and reproducible runs).
    pub fn with_client_id(id: u64) -> Self {
        Self::with_doc(Doc::with_client_id(id & ((1 << 53) - 1)))
    }

    fn with_doc(ydoc: Doc) -> Self {
        let nodes = ydoc.get_or_insert_map("nodes");
        let meta = ydoc.get_or_insert_map("doc");
        let images = ydoc.get_or_insert_map("images");

        // One undo step per publish: the clock never moves and the capture window never closes,
        // so steps are split only by `UndoManager::reset` (see `SharedDoc::end_step`).
        let options = UndoOptions::<()> {
            capture_timeout_millis: u64::MAX,
            tracked_origins: HashSet::from([Origin::from(LOCAL)]),
            capture_transaction: None,
            timestamp: Arc::new(|| 1u64),
            init_undo_stack: vec![],
            init_redo_stack: vec![],
        };
        let mut undo = UndoManager::with_options(options);
        undo.expand_scope(&ydoc, &nodes);
        undo.expand_scope(&ydoc, &meta);
        undo.expand_scope(&ydoc, &images);

        let touched = Arc::new(Mutex::new(Touched::default()));
        observe(&nodes, "vectorcraft-nodes", touched.clone(), |t| &mut t.nodes);
        observe(&meta, "vectorcraft-doc", touched.clone(), |t| &mut t.doc);
        observe(&images, "vectorcraft-images", touched.clone(), |t| &mut t.images);

        let slot = ydoc.client_id().get() % ID_SLOTS + 1;
        let start = slot << ID_SHIFT;
        Self {
            ydoc,
            nodes,
            meta,
            images,
            undo,
            touched,
            raw: HashMap::new(),
            kids: HashMap::new(),
            doc_fields: BTreeMap::new(),
            image_data: BTreeMap::new(),
            tree: Tree::default(),
            synced: None,
            id_range: (start, start + (1 << ID_SHIFT)),
            next_id: start,
            outbox: vec![],
        }
    }

    pub fn client_id(&self) -> u64 {
        self.ydoc.client_id().get()
    }

    /// The underlying Yjs document (presence shares it).
    pub fn ydoc(&self) -> &Doc {
        &self.ydoc
    }

    /// The ids this replica allocates new objects from.
    pub fn id_range(&self) -> (u64, u64) {
        self.id_range
    }

    /// Whether the CRDT holds a document yet (false until someone published one).
    pub fn is_empty(&self) -> bool {
        let txn = self.ydoc.transact();
        self.nodes.len(&txn) == 0 && self.meta.len(&txn) == 0
    }

    /// The document as last published or materialized.
    pub fn synced(&self) -> Option<&Arc<Document>> {
        self.synced.as_ref()
    }

    /// Whether remote changes wait for [`SharedDoc::materialize`].
    pub fn has_remote_changes(&self) -> bool {
        self.touched.lock().map(|t| t.remote).unwrap_or(false)
    }

    /// Encoded updates (Yjs v1) made locally since the last call, for the other participants.
    pub fn take_outgoing(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.outbox)
    }

    pub fn state_vector(&self) -> Vec<u8> {
        use yrs::updates::encoder::Encode;
        self.ydoc.transact().state_vector().encode_v1()
    }

    /// Everything the holder of `state_vector` lacks (Yjs v1 update).
    pub fn diff(&self, state_vector: &StateVector) -> Vec<u8> {
        self.ydoc.transact().encode_diff_v1(state_vector)
    }

    /// The whole state as one update (to save it, or seed a server).
    pub fn encode_state(&self) -> Vec<u8> {
        self.diff(&StateVector::default())
    }

    /// Apply an update from another participant. The document changes on the next
    /// [`SharedDoc::materialize`].
    pub fn apply_update(&mut self, update: &[u8]) -> Result<(), CollabError> {
        let update = Update::decode_v1(update).map_err(|e| CollabError::BadUpdate(e.to_string()))?;
        let mut txn = self.ydoc.transact_mut_with(REMOTE);
        txn.apply_update(update).map_err(|e| CollabError::BadUpdate(e.to_string()))?;
        drop(txn);
        if let Ok(mut t) = self.touched.lock() {
            t.remote = true;
        }
        Ok(())
    }

    // ---------------------------------------------------------------- local → CRDT

    /// Write what changed between the last synced document and `doc` into the CRDT. Returns
    /// whether anything was written. Successive publishes form one undo step until
    /// [`SharedDoc::end_step`].
    pub fn publish(&mut self, doc: &Arc<Document>) -> bool {
        if self.synced.as_ref().is_some_and(|s| Arc::ptr_eq(s, doc)) {
            return false;
        }
        if self.id_range_contains(doc.peek_next_id()) {
            self.next_id = self.next_id.max(doc.peek_next_id());
        }
        let before = self.ydoc.transact().state_vector();
        let tree = Tree::of(doc);
        let wrote;
        {
            let ydoc = self.ydoc.clone();
            let mut txn = ydoc.transact_mut_with(LOCAL);
            let mut w = false;
            w |= self.publish_fields(&mut txn, doc);
            w |= self.publish_images(&mut txn, doc);
            w |= self.publish_tree(&mut txn, &tree);
            wrote = w;
        }
        self.tree = tree;
        self.synced = Some(doc.clone());
        if wrote {
            let update = self.ydoc.transact().encode_diff_v1(&before);
            self.outbox.push(update);
        }
        wrote
    }

    /// Close the current undo step: the next publish starts a new one.
    pub fn end_step(&mut self) {
        self.undo.reset();
    }

    fn publish_fields(&mut self, txn: &mut yrs::TransactionMut, doc: &Document) -> bool {
        let mut shallow = doc.clone();
        shallow.layers.clear();
        shallow.images.clear();
        let Ok(Value::Object(m)) = serde_json::to_value(&shallow) else { return false };
        let mut wrote = false;
        let mut seen = HashSet::new();
        for (k, v) in m {
            if DOC_SKIP.contains(&k.as_str()) {
                continue;
            }
            let s = v.to_string();
            seen.insert(k.clone());
            if self.doc_fields.get(&k) != Some(&s) {
                self.meta.insert(txn, k.as_str(), s.as_str());
                self.doc_fields.insert(k, s);
                wrote = true;
            }
        }
        let gone: Vec<String> = self.doc_fields.keys().filter(|k| !seen.contains(*k)).cloned().collect();
        for k in gone {
            self.meta.remove(txn, &k);
            self.doc_fields.remove(&k);
            wrote = true;
        }
        wrote
    }

    fn publish_images(&mut self, txn: &mut yrs::TransactionMut, doc: &Document) -> bool {
        let mut wrote = false;
        for (k, blob) in &doc.images {
            let same = self.image_data.get(k).is_some_and(|old| {
                old.mime == blob.mime
                    && Arc::ptr_eq(&old.bytes, &blob.bytes)
                    && match (&old.proxy, &blob.proxy) {
                        (None, None) => true,
                        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                        _ => false,
                    }
            });
            if same {
                continue;
            }
            let mut entries: Vec<(&str, Any)> = vec![("mime", Any::from(blob.mime.clone())), ("bytes", Any::Buffer(blob.bytes.as_slice().into()))];
            if let Some(p) = &blob.proxy {
                entries.push(("proxy", Any::Buffer(p.as_slice().into())));
            }
            self.images.insert(txn, k.as_str(), MapPrelim::from_iter(entries));
            self.image_data.insert(k.clone(), blob.clone());
            wrote = true;
        }
        let gone: Vec<String> = self.image_data.keys().filter(|k| !doc.images.contains_key(*k)).cloned().collect();
        for k in gone {
            self.images.remove(txn, &k);
            self.image_data.remove(&k);
            wrote = true;
        }
        wrote
    }

    fn publish_tree(&mut self, txn: &mut yrs::TransactionMut, tree: &Tree) -> bool {
        let mut wrote = false;
        // Objects that are new or whose own fields changed.
        for (id, arc) in &tree.arcs {
            if self.tree.arcs.get(id).is_some_and(|old| Arc::ptr_eq(old, arc)) {
                continue;
            }
            let fields = fields_of(arc);
            let key = id_key(*id);
            match self.nodes.get(txn, &key) {
                Some(Out::YMap(m)) => {
                    let raw = self.raw.entry(*id).or_default();
                    for (k, v) in &fields {
                        if raw.fields.get(k) != Some(v) {
                            m.insert(txn, k.as_str(), v.as_str());
                            wrote = true;
                        }
                    }
                    for k in raw.fields.keys().filter(|k| !fields.contains_key(*k)) {
                        m.remove(txn, k);
                        wrote = true;
                    }
                }
                _ => {
                    let entries: Vec<(&str, Any)> = fields.iter().map(|(k, v)| (k.as_str(), Any::from(v.clone()))).collect();
                    self.nodes.insert(txn, key.as_str(), MapPrelim::from_iter(entries));
                    // Parent and order follow below.
                    self.raw.insert(*id, RawNode { parent: None, order: String::new(), ..RawNode::default() });
                    wrote = true;
                }
            }
            if let Some(raw) = self.raw.get_mut(id) {
                raw.node = Some(strip(arc));
                raw.fields = fields;
            }
        }
        // Parents and stacking order.
        for (parent, list) in &tree.children {
            if self.tree.children.get(parent) == Some(list) && list.iter().all(|id| self.raw.get(id).is_some_and(|r| r.parent == *parent)) {
                continue;
            }
            let old: Vec<Option<&str>> =
                list.iter().map(|id| self.raw.get(id).filter(|r| r.parent == *parent && !r.order.is_empty()).map(|r| r.order.as_str())).collect();
            let keys = order::assign(&old);
            for (id, key) in list.iter().zip(keys) {
                let Some(raw) = self.raw.get(id) else { continue };
                let (old_parent, old_order) = (raw.parent, raw.order.clone());
                if old_parent == *parent && old_order == key {
                    continue;
                }
                if let Some(Out::YMap(m)) = self.nodes.get(txn, &id_key(*id)) {
                    if old_parent != *parent || old_order.is_empty() {
                        m.insert(txn, PARENT, parent.map(id_key).unwrap_or_default());
                    }
                    m.insert(txn, ORDER, key.as_str());
                    wrote = true;
                }
                if let Some(set) = self.kids.get_mut(&old_parent) {
                    set.remove(&(old_order, *id));
                }
                self.kids.entry(*parent).or_default().insert((key.clone(), *id));
                if let Some(raw) = self.raw.get_mut(id) {
                    raw.parent = *parent;
                    raw.order = key;
                }
            }
        }
        // Deleted objects.
        let gone: Vec<NodeId> = self.tree.arcs.keys().filter(|id| !tree.arcs.contains_key(id)).copied().collect();
        for id in gone {
            self.nodes.remove(txn, &id_key(id));
            if let Some(raw) = self.raw.remove(&id)
                && let Some(set) = self.kids.get_mut(&raw.parent)
            {
                set.remove(&(raw.order, id));
            }
            wrote = true;
        }
        wrote
    }

    fn id_range_contains(&self, id: u64) -> bool {
        (self.id_range.0..self.id_range.1).contains(&id)
    }

    // ---------------------------------------------------------------- CRDT → local

    /// Rebuild the document from the CRDT after remote changes (or an undo). `current` is the
    /// document being edited: unchanged subtrees and local-only state (Puppet Warp pins) carry
    /// over from it. `None` when nothing changed since the last sync.
    pub fn materialize(&mut self, current: &Arc<Document>) -> Option<Arc<Document>> {
        let touched = match self.touched.lock() {
            Ok(mut t) => std::mem::take(&mut *t),
            Err(_) => return None,
        };
        let first = self.synced.is_none();
        if !first && touched.nodes.is_empty() && touched.doc.is_empty() && touched.images.is_empty() {
            return None;
        }
        let txn = self.ydoc.transact();

        // Document fields.
        let doc_keys: Vec<String> = if first { self.meta.keys(&txn).map(str::to_string).collect() } else { touched.doc.into_iter().collect() };
        let mut fields_changed = first;
        for k in doc_keys {
            match out_str(self.meta.get(&txn, &k)) {
                Some(v) if self.doc_fields.get(&k) != Some(&v) => {
                    self.doc_fields.insert(k, v);
                    fields_changed = true;
                }
                Some(_) => {}
                None => fields_changed |= self.doc_fields.remove(&k).is_some(),
            }
        }

        // Images.
        let image_keys: Vec<String> = if first { self.images.keys(&txn).map(str::to_string).collect() } else { touched.images.into_iter().collect() };
        let mut images_changed = first;
        for k in image_keys {
            match self.images.get(&txn, &k) {
                Some(Out::YMap(m)) => {
                    let mime = out_str(m.get(&txn, "mime")).unwrap_or_default();
                    let Some(bytes) = out_bytes(m.get(&txn, "bytes")) else { continue };
                    let proxy = out_bytes(m.get(&txn, "proxy"));
                    self.image_data.insert(k, ImageBlob { mime, bytes, proxy });
                    images_changed = true;
                }
                _ => images_changed |= self.image_data.remove(&k).is_some(),
            }
        }

        // Objects.
        let node_keys: Vec<String> = if first { self.nodes.keys(&txn).map(str::to_string).collect() } else { touched.nodes.into_iter().collect() };
        let mut dirty: HashSet<NodeId> = HashSet::new();
        for k in node_keys {
            let Some(id) = parse_id(&k) else { continue };
            let Some(Out::YMap(m)) = self.nodes.get(&txn, &k) else {
                if let Some(raw) = self.raw.remove(&id) {
                    if let Some(set) = self.kids.get_mut(&raw.parent) {
                        set.remove(&(raw.order, id));
                    }
                    dirty.insert(id);
                }
                continue;
            };
            let mut parent = None;
            let mut order = String::new();
            let mut fields = BTreeMap::new();
            for (key, value) in m.iter(&txn) {
                let Out::Any(Any::String(s)) = value else { continue };
                match key {
                    PARENT => parent = parse_id(&s),
                    ORDER => order = s.to_string(),
                    _ => {
                        fields.insert(key.to_string(), s.to_string());
                    }
                }
            }
            let raw = self.raw.entry(id).or_default();
            if raw.fields != fields || raw.node.is_none() {
                if let Some(n) = parse_node(id, &fields) {
                    raw.node = Some(n);
                }
                raw.fields = fields;
                dirty.insert(id);
            }
            if raw.parent != parent || raw.order != order {
                if let Some(set) = self.kids.get_mut(&raw.parent) {
                    set.remove(&(raw.order.clone(), id));
                }
                dirty.insert(id);
                raw.parent = parent;
                raw.order = order.clone();
                self.kids.entry(parent).or_default().insert((order, id));
            }
        }
        drop(txn);

        let layers = self.build_layers(&dirty);
        let mut doc = if fields_changed { self.document_from_fields(current) } else { (**current).clone() };
        doc.layers = layers;
        doc.images = if images_changed { self.image_data.clone() } else { current.images.clone() };
        doc.puppet = current.puppet.clone();
        doc.set_id_range(Some(self.id_range), self.next_id);
        self.next_id = self.next_id.max(doc.peek_next_id());
        let doc = Arc::new(doc);
        self.tree = Tree::of(&doc);
        self.synced = Some(doc.clone());
        Some(doc)
    }

    fn document_from_fields(&self, current: &Document) -> Document {
        let mut m = JsonMap::new();
        for (k, v) in &self.doc_fields {
            if let Ok(v) = serde_json::from_str(v) {
                m.insert(k.clone(), v);
            }
        }
        m.insert("layers".into(), Value::Array(vec![]));
        m.insert("next_id".into(), Value::from(1));
        // Fields this version requires but the CRDT lacks (an empty room) come from `current`.
        let mut defaults = current.clone();
        defaults.layers.clear();
        defaults.images.clear();
        if let Ok(Value::Object(cur)) = serde_json::to_value(&defaults) {
            for (k, v) in cur {
                m.entry(k).or_insert(v);
            }
        }
        match serde_json::from_value::<Document>(Value::Object(m)) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("collab: the shared document settings don't parse ({e}); keeping the local ones");
                current.clone()
            }
        }
    }

    /// The layer tree from the CRDT, reusing the last synced subtrees nothing in `dirty` touched.
    /// Objects whose parent is gone, isn't a container, or forms a cycle (concurrent moves) go to
    /// the end of the first layer, the same way on every replica.
    fn build_layers(&self, dirty: &HashSet<NodeId>) -> Vec<Arc<Node>> {
        let is_layer = |id: &NodeId| self.raw.get(id).and_then(|r| r.node.as_ref()).is_some_and(|n| matches!(n.kind, NodeKind::Layer { .. }));
        // Top-level objects that aren't layers move into the first layer.
        let (top, strays): (Vec<NodeId>, Vec<NodeId>) = self.sorted_kids(None).into_iter().partition(is_layer);
        let mut placed: HashSet<NodeId> = HashSet::new();
        for id in top.iter().chain(&strays) {
            self.claim(*id, &mut placed);
        }
        let mut orphans = strays;
        let mut rest: Vec<NodeId> = self.raw.keys().filter(|id| !placed.contains(id)).copied().collect();
        rest.sort();
        for id in rest {
            if placed.contains(&id) {
                continue;
            }
            // Climb to the topmost unplaced ancestor (a cycle stops where it repeats).
            let mut root = id;
            let mut seen = HashSet::from([id]);
            while let Some(p) = self.raw.get(&root).and_then(|r| r.parent) {
                if placed.contains(&p) || !self.raw.contains_key(&p) || !seen.insert(p) {
                    break;
                }
                root = p;
            }
            self.claim(root, &mut placed);
            orphans.push(root);
        }
        let mut built: Vec<Arc<Node>> = vec![];
        for id in top {
            if let Some(n) = self.build(id, None, dirty, &mut HashSet::new()) {
                built.push(n);
            }
        }
        let mut loose: Vec<Arc<Node>> = vec![];
        for id in orphans {
            // Built as a root: its recorded parent doesn't hold it.
            let Some(n) = self.build(id, Some(id), dirty, &mut HashSet::new()) else { continue };
            if matches!(n.kind, NodeKind::Layer { .. }) {
                built.push(n);
            } else {
                loose.push(n);
            }
        }
        if !loose.is_empty() {
            if built.is_empty() {
                // No layer at all: give the strays one.
                built.push(Arc::new(Node::layer(NodeId(self.next_id), "Layer 1", vectorcraft_doc::LayerColor::Preset(0))));
            }
            if let Some(first) = built.first_mut()
                && let Some(kids) = container_mut(Arc::make_mut(first))
            {
                kids.extend(loose);
            }
        }
        built
    }

    /// Mark `root` and everything the CRDT puts inside it as placed.
    fn claim(&self, root: NodeId, placed: &mut HashSet<NodeId>) {
        let mut stack = vec![root];
        placed.insert(root);
        while let Some(n) = stack.pop() {
            if !self.is_container(n) {
                continue;
            }
            for k in self.sorted_kids(Some(n)) {
                if placed.insert(k) {
                    stack.push(k);
                }
            }
        }
    }

    fn is_container(&self, id: NodeId) -> bool {
        self.raw.get(&id).and_then(|r| r.node.as_ref()).is_some_and(|n| container(n).is_some())
    }

    fn sorted_kids(&self, parent: Option<NodeId>) -> Vec<NodeId> {
        self.kids.get(&parent).map(|s| s.iter().map(|(_, id)| *id).filter(|id| self.raw.contains_key(id)).collect()).unwrap_or_default()
    }

    /// `id`'s subtree. `moved` names an object built away from its recorded parent (a repaired
    /// orphan): it is rebuilt rather than reused, as its old copy may sit elsewhere.
    fn build(&self, id: NodeId, moved: Option<NodeId>, dirty: &HashSet<NodeId>, visiting: &mut HashSet<NodeId>) -> Option<Arc<Node>> {
        if !visiting.insert(id) {
            return None;
        }
        let raw = self.raw.get(&id)?;
        let shallow = raw.node.as_ref()?;
        let kids: Vec<Arc<Node>> = if container(shallow).is_some() {
            self.sorted_kids(Some(id))
                .into_iter()
                .filter(|k| self.raw.get(k).is_some_and(|r| r.parent == Some(id)))
                .filter_map(|k| self.build(k, None, dirty, visiting))
                .collect()
        } else {
            vec![]
        };
        if !dirty.contains(&id)
            && moved.is_none()
            && let Some(old) = self.tree.arcs.get(&id)
        {
            let same = match container(old) {
                Some(old_kids) => old_kids.len() == kids.len() && old_kids.iter().zip(&kids).all(|(a, b)| Arc::ptr_eq(a, b)),
                None => kids.is_empty(),
            };
            if same {
                return Some(old.clone());
            }
        }
        let mut n = shallow.clone();
        if let Some(slot) = container_mut(&mut n) {
            *slot = kids;
        }
        Some(Arc::new(n))
    }

    // ---------------------------------------------------------------- undo

    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }

    /// Take back this participant's last step (others' edits stay). The document changes on the
    /// next [`SharedDoc::materialize`].
    pub fn undo(&mut self) -> bool {
        self.history(true)
    }

    pub fn redo(&mut self) -> bool {
        self.history(false)
    }

    fn history(&mut self, undo: bool) -> bool {
        self.undo.reset();
        let before = self.ydoc.transact().state_vector();
        let done = if undo { self.undo.undo_blocking() } else { self.undo.redo_blocking() };
        if done {
            let update = self.ydoc.transact().encode_diff_v1(&before);
            self.outbox.push(update);
            if let Ok(mut t) = self.touched.lock() {
                t.remote = true;
            }
        }
        done
    }
}

/// The node without its children (what the CRDT stores of it).
fn strip(n: &Node) -> Node {
    let mut s = n.clone();
    if let Some(kids) = container_mut(&mut s) {
        kids.clear();
    }
    s
}

/// Record the top-level keys of `map` that any transaction touches.
fn observe(map: &MapRef, key: &'static str, touched: Arc<Mutex<Touched>>, pick: fn(&mut Touched) -> &mut HashSet<String>) {
    map.observe_deep(key, move |txn, events| {
        // `publish` keeps its caches itself; everything else (remote updates, undo) is re-read.
        if txn.origin() == Some(&Origin::from(LOCAL)) {
            return;
        }
        let Ok(mut t) = touched.lock() else { return };
        for e in events.iter() {
            match e.path().front() {
                Some(PathSegment::Key(k)) => {
                    pick(&mut t).insert(k.to_string());
                }
                Some(PathSegment::Index(_)) => {}
                None => {
                    if let Event::Map(m) = e {
                        for k in m.keys(txn).keys() {
                            pick(&mut t).insert(k.to_string());
                        }
                    }
                }
            }
        }
    });
}
