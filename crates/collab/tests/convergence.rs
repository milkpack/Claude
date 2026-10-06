//! Property-based convergence tests: three participants edit one shared document at random, their
//! updates arrive late, batched and in different orders, and in the end every replica must hold
//! the same, well-formed document, the one the CRDT state itself describes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use proptest::prelude::*;
use serde_json::Value;
use vectorcraft_collab::SharedDoc;
use vectorcraft_color::{Color, Paint};
use vectorcraft_doc::appearance::Appearance;
use vectorcraft_doc::{Document, Node, NodeId, NodeKind};
use vectorcraft_geom::{Affine, Rect, shapes};
use yrs::updates::decoder::Decode;
use yrs::{Map, Transact, Update};

const PEERS: usize = 3;

// ------------------------------------------------------------------------------------ harness

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

    /// Edit like a command does: copy-on-write, publish, close the undo step.
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
}

fn rect(id: NodeId, x: f64) -> Node {
    Node::path(id, shapes::rectangle(Rect::new(x, 0.0, x + 10.0, 10.0)), Appearance::default_art())
}

/// The document as data, without what differs by design between replicas (the id counter).
fn canon(d: &Document) -> Value {
    let mut v = serde_json::to_value(d).unwrap();
    v.as_object_mut().unwrap().remove("next_id");
    v
}

/// A replica that has only ever seen `state` (a late joiner), materialized.
fn fresh(state: &[u8]) -> Arc<Document> {
    let mut s = SharedDoc::with_client_id(1000);
    s.apply_update(state).unwrap();
    s.materialize(&Arc::new(Document::new(10.0, 10.0))).unwrap()
}

/// How many objects the CRDT state holds (entries of its `nodes` map).
fn crdt_objects(state: &[u8]) -> HashSet<NodeId> {
    let doc = yrs::Doc::new();
    let nodes = doc.get_or_insert_map("nodes");
    doc.transact_mut().apply_update(Update::decode_v1(state).unwrap()).unwrap();
    let txn = doc.transact();
    nodes.keys(&txn).map(|k| NodeId(k.parse().unwrap())).collect()
}

/// Every id in the tree, failing on a duplicate.
fn tree_ids(d: &Document) -> HashSet<NodeId> {
    let mut ids = HashSet::new();
    d.walk(|n| assert!(ids.insert(n.id), "object {} appears twice", n.id));
    ids
}

/// Containers an object can go into (layers and groups), in paint order.
fn containers(d: &Document) -> Vec<NodeId> {
    let mut out = vec![];
    d.walk(|n| {
        if matches!(n.kind, NodeKind::Layer { .. } | NodeKind::Group { .. }) {
            out.push(n.id);
        }
    });
    out
}

/// Every object below the layers.
fn objects(d: &Document) -> Vec<NodeId> {
    let layers: HashSet<NodeId> = d.layers.iter().map(|l| l.id).collect();
    let mut out = vec![];
    d.walk(|n| {
        if !layers.contains(&n.id) {
            out.push(n.id);
        }
    });
    out
}

fn pick(v: &[NodeId], i: u16) -> Option<NodeId> {
    (!v.is_empty()).then(|| v[usize::from(i) % v.len()])
}

/// A tiny deterministic generator for delivery choices (the seed comes from proptest).
fn next(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

// ------------------------------------------------------------------------------------ model

#[derive(Clone, Debug)]
enum Op {
    /// A rectangle into container `.0` at index `.1`.
    AddRect(u16, u16),
    /// A group (holding one rectangle) into container `.0` at index `.1`.
    AddGroup(u16, u16),
    Delete(u16),
    /// Object `.0` into container `.1` at index `.2`.
    Move(u16, u16, u16),
    /// Object or layer `.0` to index `.1` among its siblings.
    Reorder(u16, u16),
    Recolor(u16, u8),
    Translate(u16, i8),
    Rename(u16, u8),
    Title(u8),
    /// A document field serialized only when set (absent from the CRDT otherwise).
    Template(bool),
    AddLayer,
    RemoveLayer(u16),
    Undo,
    Redo,
}

#[derive(Clone, Debug)]
enum Step {
    Edit(usize, Op),
    /// Deliver up to `n` queued updates from one peer to another, picked at random from the queue.
    Deliver {
        from: usize,
        to: usize,
        seed: u64,
        n: usize,
    },
    /// Deliver everything queued from one peer to another as one merged update.
    DeliverMerged {
        from: usize,
        to: usize,
    },
    Materialize(usize),
}

fn op() -> impl Strategy<Value = Op> {
    let i = any::<u16>;
    prop_oneof![
        5 => (i(), i()).prop_map(|(c, k)| Op::AddRect(c, k)),
        2 => (i(), i()).prop_map(|(c, k)| Op::AddGroup(c, k)),
        2 => i().prop_map(Op::Delete),
        3 => (i(), i(), i()).prop_map(|(n, c, k)| Op::Move(n, c, k)),
        2 => (i(), i()).prop_map(|(n, k)| Op::Reorder(n, k)),
        2 => (i(), any::<u8>()).prop_map(|(n, c)| Op::Recolor(n, c)),
        2 => (i(), any::<i8>()).prop_map(|(n, d)| Op::Translate(n, d)),
        1 => (i(), any::<u8>()).prop_map(|(n, k)| Op::Rename(n, k)),
        1 => any::<u8>().prop_map(Op::Title),
        1 => any::<bool>().prop_map(Op::Template),
        1 => Just(Op::AddLayer),
        1 => i().prop_map(Op::RemoveLayer),
        2 => Just(Op::Undo),
        1 => Just(Op::Redo),
    ]
}

fn step() -> impl Strategy<Value = Step> {
    let pair = (0..PEERS, 1..PEERS).prop_map(|(from, d)| (from, (from + d) % PEERS));
    prop_oneof![
        6 => (0..PEERS, op()).prop_map(|(p, o)| Step::Edit(p, o)),
        3 => (pair.clone(), any::<u64>(), 1..4usize).prop_map(|((from, to), seed, n)| Step::Deliver { from, to, seed: seed | 1, n }),
        1 => pair.prop_map(|(from, to)| Step::DeliverMerged { from, to }),
        2 => (0..PEERS).prop_map(Step::Materialize),
    ]
}

/// Apply one local edit to a document, the way the editor's commands would (invalid picks are
/// no-ops, as a disabled command would be).
fn mutate(d: &mut Document, op: &Op) {
    match *op {
        Op::AddRect(c, k) => {
            let Some(c) = pick(&containers(d), c) else { return };
            let len = d.children(Some(c)).map_or(0, Vec::len);
            let id = d.alloc_id();
            d.insert(Some(c), usize::from(k) % (len + 1), rect(id, f64::from(k % 300))).unwrap();
        }
        Op::AddGroup(c, k) => {
            let Some(c) = pick(&containers(d), c) else { return };
            let len = d.children(Some(c)).map_or(0, Vec::len);
            let (g, r) = (d.alloc_id(), d.alloc_id());
            let group = Node::group(g, vec![Arc::new(rect(r, f64::from(k % 300)))]);
            d.insert(Some(c), usize::from(k) % (len + 1), group).unwrap();
        }
        Op::Delete(n) => {
            if let Some(n) = pick(&objects(d), n) {
                d.remove(n).unwrap();
            }
        }
        Op::Move(n, c, k) => {
            let (Some(n), Some(c)) = (pick(&objects(d), n), pick(&containers(d), c)) else { return };
            // Like `Document::move_node`, which refuses this: not into itself or its descendants.
            if d.ancestry(c).unwrap().contains(&n) {
                return;
            }
            let len = d.children(Some(c)).map_or(0, Vec::len);
            d.move_node(n, Some(c), usize::from(k) % (len + 1)).unwrap();
        }
        Op::Reorder(n, k) => {
            let mut all = d.layers.iter().map(|l| l.id).collect::<Vec<_>>();
            all.extend(objects(d));
            let Some(n) = pick(&all, n) else { return };
            let parent = d.parent_of(n);
            let len = d.children(parent).map_or(0, Vec::len);
            d.move_node(n, parent, usize::from(k) % len.max(1)).unwrap();
        }
        Op::Recolor(n, c) => {
            let Some(n) = pick(&objects(d), n) else { return };
            let color = Color::rgb(f32::from(c) / 255.0, 0.5, 1.0 - f32::from(c) / 255.0);
            d.node_mut(n).unwrap().appearance = Appearance::basic(Paint::solid(color), Paint::None, 1.0);
        }
        Op::Translate(n, dx) => {
            let Some(n) = pick(&objects(d), n) else { return };
            d.node_mut(n).unwrap().transform(Affine::translate((f64::from(dx), 1.0)), true);
        }
        Op::Rename(n, k) => {
            let Some(n) = pick(&objects(d), n) else { return };
            d.node_mut(n).unwrap().name = (k % 4 != 0).then(|| format!("name {k}"));
        }
        Op::Title(k) => d.title = format!("Title {k}"),
        Op::Template(t) => d.template = t,
        Op::AddLayer => {
            d.add_layer(None);
        }
        Op::RemoveLayer(l) => {
            let layers: Vec<NodeId> = d.layers.iter().map(|l| l.id).collect();
            if layers.len() > 1
                && let Some(l) = pick(&layers, l)
            {
                d.remove(l).unwrap();
            }
        }
        Op::Undo | Op::Redo => {}
    }
}

/// Three participants sharing one document, with per-link queues of updates in flight.
struct Room {
    peers: Vec<Peer>,
    /// `queues[from][to]`: updates `from` sent that `to` hasn't received yet.
    queues: Vec<Vec<Vec<Vec<u8>>>>,
}

impl Room {
    /// Peer 0 shares a small document, the others join it.
    fn new() -> Room {
        let mut peers: Vec<Peer> = (0..PEERS).map(|i| Peer::new(i as u64 + 1)).collect();
        let first = &mut peers[0];
        let mut d = (*first.doc).clone();
        let layer = d.layers[0].id;
        let g = d.alloc_id();
        d.insert(Some(layer), usize::MAX, Node::group(g, vec![])).unwrap();
        for x in 0..3 {
            let r = d.alloc_id();
            d.insert(Some(layer), usize::MAX, rect(r, f64::from(x) * 20.0)).unwrap();
        }
        let r = d.alloc_id();
        d.insert(Some(g), usize::MAX, rect(r, 100.0)).unwrap();
        d.add_layer(None);
        first.doc = Arc::new(d);
        first.shared.publish(&first.doc);
        first.shared.end_step();
        let state = first.shared.encode_state();
        first.shared.take_outgoing();
        for p in &mut peers[1..] {
            p.shared.apply_update(&state).unwrap();
            p.pull();
        }
        Room { peers, queues: vec![vec![vec![]; PEERS]; PEERS] }
    }

    fn send(&mut self, from: usize) {
        for u in self.peers[from].shared.take_outgoing() {
            for to in 0..PEERS {
                if to != from {
                    self.queues[from][to].push(u.clone());
                }
            }
        }
    }

    fn run(&mut self, step: &Step) {
        match step {
            Step::Edit(p, op) => {
                let peer = &mut self.peers[*p];
                match op {
                    Op::Undo => {
                        peer.shared.undo();
                        peer.pull();
                    }
                    Op::Redo => {
                        peer.shared.redo();
                        peer.pull();
                    }
                    op => peer.edit(|d| mutate(d, op)),
                }
                self.send(*p);
            }
            Step::Deliver { from, to, seed, n } => {
                let mut seed = *seed;
                for _ in 0..*n {
                    let q = &mut self.queues[*from][*to];
                    if q.is_empty() {
                        break;
                    }
                    let u = q.remove(next(&mut seed) as usize % q.len());
                    self.peers[*to].shared.apply_update(&u).unwrap();
                }
            }
            Step::DeliverMerged { from, to } => {
                let q = std::mem::take(&mut self.queues[*from][*to]);
                if !q.is_empty() {
                    let merged = yrs::merge_updates_v1(q.iter().map(Vec::as_slice)).unwrap();
                    self.peers[*to].shared.apply_update(&merged).unwrap();
                }
            }
            Step::Materialize(p) => self.peers[*p].pull(),
        }
    }

    /// Deliver everything still in flight (newest first, to vary the order), then materialize,
    /// until nobody has anything more to send (materializing may repair an object, see
    /// `SharedDoc::materialize`).
    fn settle(&mut self) {
        for round in 0.. {
            assert!(round < 4, "still sending after {round} rounds");
            for from in 0..PEERS {
                self.send(from);
            }
            if round > 0 && self.queues.iter().flatten().all(Vec::is_empty) {
                break;
            }
            for from in 0..PEERS {
                for to in 0..PEERS {
                    while let Some(u) = self.queues[from][to].pop() {
                        self.peers[to].shared.apply_update(&u).unwrap();
                    }
                }
            }
            for p in &mut self.peers {
                p.pull();
            }
        }
    }

    /// The convergence properties.
    fn check(&self) {
        let reference = &self.peers[0].doc;
        for (i, p) in self.peers.iter().enumerate() {
            let doc = &p.doc;
            assert_same(doc, reference, &format!("peer {i} vs peer 0"));
            // Well-formed: unique ids, layers (only) at the top.
            let ids = tree_ids(doc);
            for l in &doc.layers {
                assert!(matches!(l.kind, NodeKind::Layer { .. }), "top-level object {} isn't a layer", l.id);
            }
            // A late joiner gets the same document; nothing the CRDT holds is missing from it.
            let state = p.shared.encode_state();
            assert_same(&fresh(&state), reference, &format!("replica from peer {i}'s state vs peer 0"));
            let held = crdt_objects(&state);
            let missing: Vec<_> = held.difference(&ids).collect();
            assert!(missing.is_empty(), "objects in the CRDT but not in the tree: {missing:?}");
            // The only object the tree may have beyond the CRDT: a layer made up for objects left
            // without one, until someone's next edit publishes it.
            let extra: Vec<_> = ids.difference(&held).collect();
            assert!(
                extra.len() <= 1 && extra.iter().all(|id| doc.layers.iter().any(|l| l.id == **id)),
                "objects in the tree but not in the CRDT: {extra:?}"
            );
        }
    }
}

/// Fail with the two trees (not pages of JSON) when two documents differ.
fn assert_same(a: &Document, b: &Document, what: &str) {
    let (ca, cb) = (canon(a), canon(b));
    if ca == cb {
        return;
    }
    let (Value::Object(ma), Value::Object(mb)) = (&ca, &cb) else { panic!("{what}: not objects") };
    let fields: Vec<&String> = ma.keys().chain(mb.keys()).filter(|k| ma.get(*k) != mb.get(*k)).collect();
    panic!("{what}: documents differ in {fields:?}\n--- left:\n{}--- right:\n{}", outline(a), outline(b));
}

/// The tree as indented lines: id, kind, name, bounds.
fn outline(d: &Document) -> String {
    fn go(n: &Node, depth: usize, out: &mut String) {
        let kind = serde_json::to_value(&n.kind).ok().and_then(|v| v.get("type").and_then(|t| t.as_str().map(str::to_string))).unwrap_or_default();
        let x = n.geometric_bounds().map(|b| b.x0);
        out.push_str(&format!("{}{} {kind} {:?} {x:?}\n", "  ".repeat(depth), n.id, n.name));
        for c in n.children().into_iter().flatten() {
            go(c, depth + 1, out);
        }
    }
    let mut s = format!("title {:?} template {}\n", d.title, d.template);
    for l in &d.layers {
        go(l, 0, &mut s);
    }
    s
}

/// 128 cases, or `CONVERGENCE_CASES` to search longer.
fn cases() -> u32 {
    std::env::var("CONVERGENCE_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(128)
}

fn play(steps: &[Step]) {
    let mut room = Room::new();
    for s in steps {
        room.run(s);
    }
    room.settle();
    room.check();
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(), max_shrink_iters: 4096, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn random_sessions_converge(steps in proptest::collection::vec(step(), 1..100)) {
        play(&steps);
    }
}

// ------------------------------------------------------------------------------------ cost

/// A remote recolour of one object in a 5,000-object document rebuilds only its path.
#[test]
fn materialize_rebuilds_only_what_changed() {
    let mut a = Peer::new(1);
    let mut b = Peer::new(2);
    let mut target = NodeId(0);
    a.edit(|d| {
        let layer = d.layers[0].id;
        for gi in 0..50 {
            let g = d.alloc_id();
            let kids: Vec<Arc<Node>> = (0..99)
                .map(|ri| {
                    let id = d.alloc_id();
                    if gi == 17 && ri == 42 {
                        target = id;
                    }
                    Arc::new(rect(id, f64::from(ri)))
                })
                .collect();
            d.insert(Some(layer), usize::MAX, Node::group(g, kids)).unwrap();
        }
    });
    assert!(a.doc.node_count() > 5000);
    for u in a.shared.take_outgoing() {
        b.shared.apply_update(&u).unwrap();
    }
    b.pull();
    assert_eq!(canon(&a.doc), canon(&b.doc));

    a.edit(|d| d.node_mut(target).unwrap().appearance = Appearance::basic(Paint::solid(Color::BLACK), Paint::None, 1.0));
    let updates = a.shared.take_outgoing();
    assert_eq!(updates.len(), 1);
    assert!(updates[0].len() < 1000, "a recolour sent {} bytes", updates[0].len());
    let before = b.doc.clone();
    b.shared.apply_update(&updates[0]).unwrap();
    let t = Instant::now();
    b.pull();
    let took = t.elapsed();
    assert!(took < Duration::from_millis(500), "materialize took {took:?}");
    assert_eq!(canon(&a.doc), canon(&b.doc));

    // Every group but the target's is the same allocation; inside it, every other rectangle too.
    let old = before.layers[0].children().unwrap();
    let new = b.doc.layers[0].children().unwrap();
    assert_eq!(old.len(), new.len());
    for (o, n) in old.iter().zip(new) {
        let has_target = n.children().unwrap().iter().any(|r| r.id == target);
        assert_eq!(Arc::ptr_eq(o, n), !has_target, "group {}", n.id);
        for (or, nr) in o.children().unwrap().iter().zip(n.children().unwrap()) {
            assert_eq!(Arc::ptr_eq(or, nr), nr.id != target, "object {}", nr.id);
        }
    }
    for l in &before.layers[1..] {
        assert!(b.doc.layers.iter().any(|n| Arc::ptr_eq(l, n)));
    }
}

// ------------------------------------------------------------------------------------ regressions
// Cases the property found (shrunk), kept as plain tests. Each names the bug it caught.

/// Peer 2's updates reach the others newest first. yrs integrated the later update with a skip
/// over the gap and then never retried the blocks that waited on the gap: the move was lost on
/// two replicas for good.
#[test]
fn updates_from_one_peer_out_of_order() {
    play(&[
        Step::Edit(2, Op::AddRect(0, 1454)),
        Step::Edit(0, Op::AddRect(0, 0)),
        Step::Edit(2, Op::Move(15509, 0, 804)),
        Step::Edit(2, Op::Move(692, 7361, 0)),
    ]);
}

/// The same, plainly: three edits applied in reverse order.
#[test]
fn three_edits_applied_in_reverse() {
    let mut room = Room::new();
    for op in [Op::AddRect(0, 0), Op::Recolor(0, 9), Op::Move(0, 1, 0)] {
        room.run(&Step::Edit(1, op));
    }
    let updates = std::mem::take(&mut room.queues[1][0]);
    assert_eq!(updates.len(), 3);
    for u in updates.iter().rev() {
        room.peers[0].shared.apply_update(u).unwrap();
    }
    room.peers[0].pull();
    assert_same(&room.peers[0].doc, &room.peers[1].doc, "after reverse delivery");
}

/// A batch with a hole (an update in the middle was delivered alone, earlier) must wait for
/// nothing: the hole is already there.
#[test]
fn batch_with_a_hole_already_filled() {
    play(&[
        Step::Edit(2, Op::AddRect(0, 0)),
        Step::Edit(2, Op::AddRect(0, 0)),
        Step::Edit(2, Op::AddRect(0, 0)),
        Step::Deliver { from: 2, to: 0, seed: 851701157991481239, n: 1 },
        Step::Edit(0, Op::AddRect(0, 0)),
        Step::DeliverMerged { from: 2, to: 0 },
    ]);
}

/// Peer 0's edit depends on peer 1's objects, which arrive after it and out of order: the
/// waiting edit must be retried once they are in (it stayed pending in yrs forever, and peer 2
/// never showed it).
#[test]
fn dependency_on_another_peer_arrives_late() {
    play(&[
        Step::Edit(1, Op::AddRect(0, 0)),
        Step::Edit(0, Op::RemoveLayer(0)),
        Step::Edit(2, Op::Move(15516, 18752, 0)),
        Step::Edit(1, Op::AddRect(21, 0)),
        Step::Edit(0, Op::Delete(0)),
        Step::Deliver { from: 1, to: 0, seed: 1, n: 2 },
        Step::Materialize(0),
        Step::Edit(0, Op::AddRect(0, 0)),
        Step::Edit(0, Op::AddLayer),
    ]);
}

/// Undo right after sharing emptied the room for everyone (the share was an undo step).
#[test]
fn undo_right_after_sharing_does_nothing() {
    let mut room = Room::new();
    assert!(!room.peers[0].shared.can_undo());
    let before = canon(&room.peers[0].doc);
    room.run(&Step::Edit(0, Op::Undo));
    room.settle();
    assert_eq!(canon(&room.peers[0].doc), before);
    room.check();
}

/// Every layer gone (two removed at once) while objects were added to one: the layer made up
/// for them had each replica's own next id.
#[test]
fn no_layer_left() {
    play(&[
        Step::Edit(0, Op::RemoveLayer(509)),
        Step::Edit(0, Op::AddRect(0, 0)),
        Step::Edit(0, Op::AddRect(0, 0)),
        Step::Edit(1, Op::RemoveLayer(11570)),
    ]);
}

/// Peer 2 adds into a layer peer 0 deleted (delivered, not yet materialized): the layer comes
/// back, but without a parent or stacking order, so it sorted differently on every replica.
#[test]
fn layer_brought_back_by_an_edit_gets_its_place() {
    let steps = [
        Step::Edit(0, Op::RemoveLayer(10062)),
        Step::Deliver { from: 0, to: 2, seed: 1, n: 1 },
        Step::Edit(0, Op::AddRect(0, 0)),
        Step::Edit(2, Op::AddGroup(594, 0)),
    ];
    play(&steps);
    // Peer 2 wrote the layer back whole: no replica has to repair it.
    let mut room = Room::new();
    let layer = room.peers[0].doc.layers[0].id;
    for s in &steps {
        room.run(s);
    }
    let doc = yrs::Doc::new();
    let nodes = doc.get_or_insert_map("nodes");
    doc.transact_mut().apply_update(Update::decode_v1(&room.peers[2].shared.encode_state()).unwrap()).unwrap();
    let txn = doc.transact();
    let Some(yrs::Out::YMap(entry)) = nodes.get(&txn, &layer.0.to_string()) else { panic!("layer {layer} not written back") };
    assert!(entry.get(&txn, "p").is_some() && entry.get(&txn, "o").is_some(), "layer {layer} written back without its place");
}

/// The same with more traffic: the layer brought back was also listed under its old place, so
/// it appeared twice.
#[test]
fn object_brought_back_is_listed_once() {
    play(&[
        Step::Edit(2, Op::AddRect(12284, 0)),
        Step::Edit(2, Op::Move(843, 29399, 0)),
        Step::Edit(2, Op::AddRect(25553, 0)),
        Step::Edit(2, Op::AddRect(0, 7701)),
        Step::Edit(2, Op::Move(12362, 0, 3572)),
        Step::Edit(2, Op::Move(14034, 19321, 0)),
        Step::Edit(0, Op::AddRect(0, 0)),
        Step::Edit(0, Op::AddRect(0, 0)),
        Step::Deliver { from: 2, to: 1, seed: 1, n: 1 },
        Step::Deliver { from: 2, to: 1, seed: 1, n: 2 },
        Step::Edit(2, Op::AddRect(0, 0)),
        Step::Edit(2, Op::RemoveLayer(10919)),
        Step::Edit(1, Op::AddGroup(0, 0)),
        Step::Edit(2, Op::AddRect(0, 0)),
        Step::Deliver { from: 2, to: 1, seed: 6037870598050489699, n: 2 },
        Step::Edit(1, Op::AddRect(48343, 0)),
    ]);
}

/// A document field set back to its default (here by Undo) leaves the CRDT; replicas kept the
/// value they had instead of the default.
#[test]
fn document_field_back_to_default() {
    play(&[Step::Edit(0, Op::Template(true)), Step::Edit(0, Op::Undo)]);
    play(&[Step::Edit(0, Op::Template(true)), Step::Edit(0, Op::Template(false))]);
}

/// Peer 1 deletes a layer while peer 0 moves an object in it, then undoes: the object comes back
/// without its geometry (Yjs doesn't redo an entry someone else overwrote), and only replicas
/// that still had the old version showed it.
#[test]
fn undo_delete_after_concurrent_edit() {
    play(&[
        Step::Edit(1, Op::RemoveLayer(42854)),
        Step::Edit(0, Op::Translate(0, 0)),
        Step::Deliver { from: 0, to: 1, seed: 1, n: 1 },
        Step::Edit(1, Op::Undo),
    ]);
}

/// The same with the stacking order: the layer came back without one.
#[test]
fn undo_delete_after_concurrent_reorder() {
    play(&[
        Step::Edit(1, Op::AddRect(0, 0)),
        Step::Edit(1, Op::RemoveLayer(24455)),
        Step::Edit(0, Op::AddRect(0, 0)),
        Step::Edit(0, Op::Reorder(59064, 7157)),
        Step::Edit(1, Op::RemoveLayer(0)),
        Step::DeliverMerged { from: 0, to: 1 },
        Step::Edit(1, Op::Undo),
    ]);
}

/// Recolour, Undo, then someone deletes the object: yrs's garbage collection freed the object's
/// map but left the entries Undo keeps pointing at it, and encoding the state read freed memory
/// (a crash, about one run in two).
#[test]
fn delete_after_undone_recolor_is_memory_safe() {
    play(&[
        Step::Edit(2, Op::AddGroup(30571, 45838)),
        Step::Edit(1, Op::Recolor(23769, 186)),
        Step::Edit(1, Op::Undo),
        Step::Edit(1, Op::Recolor(28932, 18)),
        Step::Edit(0, Op::AddLayer),
        Step::Deliver { from: 0, to: 2, seed: 9531254962752373961, n: 2 },
        Step::Edit(2, Op::Delete(32297)),
    ]);
    // The same, directly: every object recoloured and the recolour undone, then deleted remotely.
    for victim in 0..4 {
        let mut room = Room::new();
        room.run(&Step::Edit(1, Op::Recolor(victim, 1)));
        room.run(&Step::Edit(1, Op::Undo));
        room.run(&Step::Edit(1, Op::Recolor(victim + 1, 2)));
        room.run(&Step::Edit(0, Op::Delete(victim)));
        room.settle();
        for p in &room.peers {
            let _ = fresh(&p.shared.encode_state());
        }
        room.check();
    }
}
