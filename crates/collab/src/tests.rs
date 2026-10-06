#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use serde_json::Value;
use vectorcraft_color::{Color, Paint};
use vectorcraft_doc::appearance::Appearance;
use vectorcraft_doc::{Document, ImageBlob, Node, NodeId};
use vectorcraft_geom::{Affine, Rect, shapes};

use crate::{Connection, PeerState, SharedDoc};

/// One participant: a replica and the document its editor shows.
struct Peer {
    shared: SharedDoc,
    doc: Arc<Document>,
}

impl Peer {
    fn new(client: u64) -> Peer {
        let shared = SharedDoc::with_client_id(client);
        let mut doc = Document::new(400.0, 300.0);
        doc.set_id_range(Some(shared.id_range()), 0);
        Peer { shared, doc: Arc::new(doc) }
    }

    /// Edit like a command does: copy-on-write, then publish as one undo step.
    fn edit(&mut self, f: impl FnOnce(&mut Document)) {
        let mut d = (*self.doc).clone();
        f(&mut d);
        self.doc = Arc::new(d);
        self.shared.publish(&self.doc);
        self.shared.end_step();
    }

    fn pull(&mut self) {
        if let Some(d) = self.shared.materialize(&self.doc) {
            self.doc = d;
        }
    }

    fn layer(&self) -> NodeId {
        self.doc.layers[0].id
    }

    fn add_rect(&mut self, x: f64) -> NodeId {
        let mut id = NodeId(0);
        let layer = self.layer();
        self.edit(|d| {
            id = d.alloc_id();
            d.insert(Some(layer), usize::MAX, rect(id, x)).unwrap();
        });
        id
    }
}

fn rect(id: NodeId, x: f64) -> Node {
    Node::path(id, shapes::rectangle(Rect::new(x, 0.0, x + 10.0, 10.0)), Appearance::default_art())
}

/// Deliver every pending update both ways until both are quiet, then rebuild both documents.
fn sync(a: &mut Peer, b: &mut Peer) {
    loop {
        let (ua, ub) = (a.shared.take_outgoing(), b.shared.take_outgoing());
        if ua.is_empty() && ub.is_empty() {
            break;
        }
        for u in ua {
            b.shared.apply_update(&u).unwrap();
        }
        for u in ub {
            a.shared.apply_update(&u).unwrap();
        }
    }
    a.pull();
    b.pull();
}

/// The document as data, without what differs by design between replicas (the id counter).
fn canon(d: &Document) -> Value {
    let mut v = serde_json::to_value(d).unwrap();
    v.as_object_mut().unwrap().remove("next_id");
    v
}

fn fill(d: &Document, id: NodeId) -> Option<Paint> {
    d.node(id).map(|n| n.appearance.fill_paint())
}

fn set_fill(d: &mut Document, id: NodeId, c: Color) {
    let n = d.node_mut(id).unwrap();
    n.appearance = Appearance::basic(Paint::solid(c), Paint::None, 1.0);
}

fn x_of(d: &Document, id: NodeId) -> f64 {
    d.node(id).unwrap().geometric_bounds().unwrap().x0
}

/// A joined B: B's document is the one A shared.
fn joined() -> (Peer, Peer) {
    let mut a = Peer::new(1);
    let mut b = Peer::new(2);
    let doc = a.doc.clone();
    a.shared.publish(&doc);
    a.shared.end_step();
    sync(&mut a, &mut b);
    (a, b)
}

#[test]
fn join_receives_the_document() {
    let (mut a, mut b) = joined();
    let r = a.add_rect(10.0);
    sync(&mut a, &mut b);
    assert_eq!(canon(&a.doc), canon(&b.doc));
    assert!(b.doc.node(r).is_some());
    assert_eq!(b.doc.layers.len(), 1);
}

#[test]
fn concurrent_recolor_and_move_both_apply() {
    let (mut a, mut b) = joined();
    let r = a.add_rect(10.0);
    sync(&mut a, &mut b);
    // At the same time: A recolours, B moves.
    a.edit(|d| set_fill(d, r, Color::rgb(1.0, 0.0, 0.0)));
    b.edit(|d| d.node_mut(r).unwrap().transform(Affine::translate((100.0, 0.0)), true));
    sync(&mut a, &mut b);
    assert_eq!(canon(&a.doc), canon(&b.doc));
    // The move lives in `kind` (geometry), the colour in `appearance`: neither overwrote the other.
    assert!((x_of(&a.doc, r) - 110.0).abs() < 1e-9);
    assert_eq!(fill(&a.doc, r), Some(Paint::solid(Color::rgb(1.0, 0.0, 0.0))));
}

#[test]
fn objects_drawn_at_once_keep_distinct_ids() {
    let (mut a, mut b) = joined();
    let ra = a.add_rect(10.0);
    let rb = b.add_rect(50.0);
    assert_ne!(ra, rb);
    sync(&mut a, &mut b);
    assert_eq!(canon(&a.doc), canon(&b.doc));
    assert!(a.doc.node(ra).is_some() && a.doc.node(rb).is_some());
    // Both land in the layer, in the same order on both sides.
    assert_eq!(a.doc.layers[0].children().unwrap().len(), 2);
}

#[test]
fn delete_wins_over_concurrent_edit() {
    let (mut a, mut b) = joined();
    let r = a.add_rect(10.0);
    sync(&mut a, &mut b);
    a.edit(|d| {
        d.remove(r).unwrap();
    });
    b.edit(|d| set_fill(d, r, Color::rgb(0.0, 1.0, 0.0)));
    sync(&mut a, &mut b);
    assert_eq!(canon(&a.doc), canon(&b.doc));
    assert!(a.doc.node(r).is_none());
}

#[test]
fn undo_takes_back_only_your_own_changes() {
    let (mut a, mut b) = joined();
    let r = a.add_rect(10.0);
    sync(&mut a, &mut b);
    a.edit(|d| set_fill(d, r, Color::rgb(1.0, 0.0, 0.0)));
    sync(&mut a, &mut b);
    let rb = b.add_rect(200.0);
    b.edit(|d| d.node_mut(r).unwrap().transform(Affine::translate((5.0, 0.0)), true));
    sync(&mut a, &mut b);

    // A undoes its recolour: B's rectangle and B's move stay.
    assert!(a.shared.undo());
    sync(&mut a, &mut b);
    assert_eq!(canon(&a.doc), canon(&b.doc));
    assert_ne!(fill(&a.doc, r), Some(Paint::solid(Color::rgb(1.0, 0.0, 0.0))));
    assert!(a.doc.node(rb).is_some());
    assert!((x_of(&a.doc, r) - 15.0).abs() < 1e-9);

    // Redo brings it back; undoing twice more removes A's rectangle (B's move goes with it).
    assert!(a.shared.redo());
    sync(&mut a, &mut b);
    assert_eq!(fill(&b.doc, r), Some(Paint::solid(Color::rgb(1.0, 0.0, 0.0))));
    assert!(a.shared.undo());
    assert!(a.shared.undo());
    sync(&mut a, &mut b);
    assert_eq!(canon(&a.doc), canon(&b.doc));
    assert!(b.doc.node(r).is_none());
    assert!(b.doc.node(rb).is_some());
}

#[test]
fn concurrent_reorders_converge_without_duplicates() {
    let (mut a, mut b) = joined();
    let ids: Vec<NodeId> = (0..5).map(|i| a.add_rect(i as f64 * 20.0)).collect();
    sync(&mut a, &mut b);
    let layer = a.layer();
    a.edit(|d| d.move_node(ids[0], Some(layer), usize::MAX).unwrap());
    b.edit(|d| d.move_node(ids[4], Some(layer), 0).unwrap());
    sync(&mut a, &mut b);
    assert_eq!(canon(&a.doc), canon(&b.doc));
    let order: Vec<NodeId> = a.doc.layers[0].children().unwrap().iter().map(|n| n.id).collect();
    assert_eq!(order, vec![ids[4], ids[1], ids[2], ids[3], ids[0]]);
}

#[test]
fn crossed_moves_into_each_other_lose_nothing() {
    let (mut a, mut b) = joined();
    let layer = a.layer();
    let (mut g1, mut g2) = (NodeId(0), NodeId(0));
    a.edit(|d| {
        g1 = d.alloc_id();
        g2 = d.alloc_id();
        d.insert(Some(layer), usize::MAX, Node::group(g1, vec![])).unwrap();
        d.insert(Some(layer), usize::MAX, Node::group(g2, vec![])).unwrap();
    });
    let r = a.add_rect(0.0);
    a.edit(|d| d.move_node(r, Some(g1), 0).unwrap());
    sync(&mut a, &mut b);
    // A puts g1 into g2 while B puts g2 into g1: a cycle once merged.
    a.edit(|d| d.move_node(g1, Some(g2), 0).unwrap());
    b.edit(|d| d.move_node(g2, Some(g1), 0).unwrap());
    sync(&mut a, &mut b);
    assert_eq!(canon(&a.doc), canon(&b.doc));
    for id in [g1, g2, r] {
        assert!(a.doc.node(id).is_some(), "{id} lost");
    }
}

#[test]
fn unchanged_subtrees_are_reused() {
    let (mut a, mut b) = joined();
    let layer = a.layer();
    let mut g = NodeId(0);
    a.edit(|d| {
        g = d.alloc_id();
        d.insert(Some(layer), usize::MAX, Node::group(g, vec![])).unwrap();
    });
    let r1 = a.add_rect(0.0);
    a.edit(|d| d.move_node(r1, Some(g), 0).unwrap());
    let r2 = a.add_rect(40.0);
    sync(&mut a, &mut b);
    let group_before = b.doc.layers[0].children().unwrap().iter().find(|n| n.id == g).unwrap().clone();
    a.edit(|d| set_fill(d, r2, Color::BLACK));
    sync(&mut a, &mut b);
    let group_after = b.doc.layers[0].children().unwrap().iter().find(|n| n.id == g).unwrap().clone();
    assert!(Arc::ptr_eq(&group_before, &group_after), "untouched group was rebuilt");
}

#[test]
fn document_settings_and_images_sync() {
    let (mut a, mut b) = joined();
    a.edit(|d| {
        d.title = "Poster".into();
        d.images.insert("img1".into(), ImageBlob { mime: "image/png".into(), bytes: Arc::new(vec![1, 2, 3]), proxy: None });
    });
    sync(&mut a, &mut b);
    assert_eq!(b.doc.title, "Poster");
    assert_eq!(b.doc.images.get("img1").map(|i| i.bytes.as_slice()), Some(&[1u8, 2, 3][..]));
    a.edit(|d| {
        d.images.clear();
    });
    sync(&mut a, &mut b);
    assert!(b.doc.images.is_empty());
}

#[test]
fn late_joiner_gets_full_state_through_the_protocol() {
    let (mut a, _) = joined();
    let r = a.add_rect(10.0);
    // A server would hold this; a newcomer syncs from it with step 1 / step 2.
    let mut server = SharedDoc::with_client_id(99);
    server.apply_update(&a.shared.encode_state()).unwrap();
    let mut c = Connection::new(SharedDoc::with_client_id(3), || 1000u64);
    c.set_presence(&PeerState { name: "C".into(), ..PeerState::default() });
    let hello = c.hello();
    assert_eq!(hello.len(), 2, "sync step 1 + presence");
    // Answer step 1 the way the server does.
    let mut server_conn = Connection::new(server, || 1000u64);
    let mut replies = vec![];
    for m in &hello {
        replies.extend(server_conn.receive(m).unwrap());
    }
    for m in replies {
        c.receive(&m).unwrap();
    }
    assert!(c.is_synced());
    assert_eq!(server_conn.peers(), vec![(3, PeerState { name: "C".into(), ..PeerState::default() })]);
    let mut doc = Arc::new(Document::new(10.0, 10.0));
    doc = c.doc.materialize(&doc).unwrap();
    assert!(doc.node(r).is_some());
    assert_eq!(canon(&doc), canon(&a.doc));
}

#[test]
fn garbage_updates_are_errors_not_crashes() {
    let mut s = SharedDoc::with_client_id(5);
    assert!(s.apply_update(&[0xff, 0x00, 0x13]).is_err());
    let mut c = Connection::new(SharedDoc::with_client_id(6), || 0u64);
    assert!(c.receive(&[]).is_err());
    assert!(c.receive(&[0, 9, 9, 9]).is_err());
}
