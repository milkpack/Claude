//! Two sessions co-editing through an in-memory relay that behaves like the collaboration server.

use serde_json::json;
use vectorcraft_collab::{Connection, SharedDoc};
use vectorcraft_doc::NodeId;
use vectorcraft_geom::Point;

use super::*;
use crate::collab::Collab;

/// The server: one replica of the room; whatever a client sends is applied and relayed to the
/// others (the same thing `vectorcraft-collab-server` does over WebSockets).
struct Relay {
    room: Connection,
}

struct Client {
    s: Session,
    c: Collab,
    inbox: Vec<Vec<u8>>,
}

impl Client {
    fn new(id: u64, name: &str) -> Client {
        let mut s = Session::new();
        s.execute("file.new", &json!({"width": 800, "height": 600})).unwrap();
        let uid = s.active().unwrap().uid;
        let c = Collab::new(Connection::new(SharedDoc::with_client_id(id), || 1u64), uid, name);
        Client { s, c, inbox: vec![] }
    }

    fn doc(&self) -> &DocState {
        self.s.active().unwrap()
    }

    fn rect(&mut self, x: f64) -> NodeId {
        let r = self.s.execute("shape.rectangle", &json!({"x": x, "y": 10, "width": 50, "height": 20})).unwrap();
        NodeId(r["id"].as_u64().unwrap())
    }
}

impl Relay {
    fn new() -> Relay {
        Relay { room: Connection::new(SharedDoc::with_client_id(1_000_000), || 1u64) }
    }

    /// Deliver until nobody has anything left to say.
    fn pump(&mut self, clients: &mut [&mut Client]) {
        for _ in 0..20 {
            let mut quiet = true;
            for i in 0..clients.len() {
                let inbox = std::mem::take(&mut clients[i].inbox);
                let mut out = vec![];
                for m in inbox {
                    quiet = false;
                    out.extend(clients[i].c.receive(&m).unwrap());
                }
                let (_, ticked) = clients[i].c.tick(&mut clients[i].s, Some(Point::new(1.0, 2.0)));
                out.extend(ticked);
                for m in out {
                    quiet = false;
                    // The server answers the sender and relays updates and presence to the rest.
                    let replies = self.room.receive(&m).unwrap();
                    clients[i].inbox.extend(replies);
                    let relayed = m.first().is_some_and(|t| *t == 1) || m.get(..2).is_some_and(|h| h == [0, 2] || h == [0, 1]);
                    if relayed {
                        for (j, other) in clients.iter_mut().enumerate() {
                            if j != i {
                                other.inbox.push(m.clone());
                            }
                        }
                    }
                }
            }
            if quiet && clients.iter().all(|c| c.inbox.is_empty()) {
                break;
            }
        }
    }
}

fn connect(relay: &mut Relay, c: &mut Client) {
    let hello = c.c.on_connect();
    for m in hello {
        c.inbox.extend(relay.room.receive(&m).unwrap());
    }
}

fn canon(st: &DocState) -> serde_json::Value {
    let mut v = serde_json::to_value(&*st.doc).unwrap();
    v.as_object_mut().unwrap().remove("next_id");
    v
}

#[test]
fn two_sessions_co_edit_with_commands() {
    let mut relay = Relay::new();
    let mut a = Client::new(1, "Аня");
    let mut b = Client::new(2, "Борис");

    // A opens the room first: the room gets A's document.
    let ra = a.rect(10.0);
    connect(&mut relay, &mut a);
    relay.pump(&mut [&mut a]);
    assert!(a.c.is_joined());

    // B joins: B's untitled document is replaced by the room's.
    connect(&mut relay, &mut b);
    relay.pump(&mut [&mut a, &mut b]);
    assert!(b.c.is_joined());
    assert!(b.doc().doc.node(ra).is_some());
    assert_eq!(canon(a.doc()), canon(b.doc()));

    // Both draw at once; ids don't clash.
    let rb = b.rect(200.0);
    let ra2 = a.rect(400.0);
    relay.pump(&mut [&mut a, &mut b]);
    assert_eq!(canon(a.doc()), canon(b.doc()));
    assert!(a.doc().doc.node(rb).is_some() && b.doc().doc.node(ra2).is_some());

    // B moves A's rectangle; A sees it.
    b.s.execute("select.set", &json!({"ids": [ra.0]})).unwrap();
    b.s.execute("object.move", &json!({"dx": 30, "dy": 0})).unwrap();
    relay.pump(&mut [&mut a, &mut b]);
    assert_eq!(canon(a.doc()), canon(b.doc()));
    let x = a.doc().doc.node(ra).unwrap().geometric_bounds().unwrap().x0;
    assert!((x - 40.0).abs() < 1e-6, "{x}");

    // A's Undo takes back A's last rectangle only, not B's move or B's rectangle.
    a.s.execute("edit.undo", &json!({})).unwrap();
    relay.pump(&mut [&mut a, &mut b]);
    assert_eq!(canon(a.doc()), canon(b.doc()));
    assert!(b.doc().doc.node(ra2).is_none());
    assert!(b.doc().doc.node(rb).is_some());
    let x = b.doc().doc.node(ra).unwrap().geometric_bounds().unwrap().x0;
    assert!((x - 40.0).abs() < 1e-6);
    a.s.execute("edit.redo", &json!({})).unwrap();
    relay.pump(&mut [&mut a, &mut b]);
    assert!(b.doc().doc.node(ra2).is_some());

    // Presence: each sees the other's name, pointer and selection.
    let peers = a.c.peers();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].1.name, "Борис");
    assert_eq!(peers[0].1.cursor, Some([1.0, 2.0]));
    assert_eq!(peers[0].1.selection, vec![ra.0]);
}

#[test]
fn remote_edits_wait_for_a_drag_to_end() {
    let mut relay = Relay::new();
    let mut a = Client::new(1, "A");
    let mut b = Client::new(2, "B");
    let r = a.rect(10.0);
    connect(&mut relay, &mut a);
    relay.pump(&mut [&mut a]);
    connect(&mut relay, &mut b);
    relay.pump(&mut [&mut a, &mut b]);

    // A is mid-gesture (an interaction is open) when B's rectangle arrives.
    let before = a.doc().doc.clone();
    a.s.active_mut().unwrap().interaction = Some(crate::Interaction {
        label: "Move".into(),
        doc: before.clone(),
        selection: Default::default(),
        preview: None,
        active_layer: None,
        isolation: None,
        perspective_again: None,
    });
    let rb = b.rect(300.0);
    relay.pump(&mut [&mut a, &mut b]);
    assert!(a.doc().doc.node(rb).is_none(), "applied mid-drag");
    a.s.active_mut().unwrap().interaction = None;
    relay.pump(&mut [&mut a, &mut b]);
    assert!(a.doc().doc.node(rb).is_some() && a.doc().doc.node(r).is_some());
    assert_eq!(canon(a.doc()), canon(b.doc()));
}

#[test]
fn leaving_restores_local_undo() {
    let mut relay = Relay::new();
    let mut a = Client::new(1, "A");
    connect(&mut relay, &mut a);
    relay.pump(&mut [&mut a]);
    assert!(a.doc().shared_history.is_some());
    a.c.leave(&mut a.s);
    assert!(a.doc().shared_history.is_none());
    let r = a.rect(0.0);
    a.s.execute("edit.undo", &json!({})).unwrap();
    assert!(a.doc().doc.node(r).is_none());
}

#[test]
fn procedural_objects_converge() {
    let mut relay = Relay::new();
    let mut a = Client::new(1, "A");
    let mut b = Client::new(2, "B");
    connect(&mut relay, &mut a);
    relay.pump(&mut [&mut a]);
    connect(&mut relay, &mut b);
    relay.pump(&mut [&mut a, &mut b]);

    // A makes a procedural object; B gets the graph and the generated art.
    let id = NodeId(a.s.execute("procedural.create", &json!({"preset": "jitterSquares", "x": 200, "y": 200})).unwrap()["id"].as_u64().unwrap());
    relay.pump(&mut [&mut a, &mut b]);
    assert_eq!(canon(a.doc()), canon(b.doc()));
    assert!(b.doc().doc.node(id).is_some_and(|n| n.procedural.is_some()));

    // B edits a parameter (regenerating the art with ids from B's range); A follows.
    let g = b.doc().doc.node(id).unwrap().procedural.clone().unwrap();
    let grid = g.nodes.iter().find(|n| n.kind == "instance.grid").unwrap().id;
    b.s.execute("procedural.setParam", &json!({"id": id.0, "node": grid, "param": "columns", "value": 4})).unwrap();
    relay.pump(&mut [&mut a, &mut b]);
    assert_eq!(canon(a.doc()), canon(b.doc()));
    assert_eq!(a.doc().doc.node(id).unwrap().children().unwrap().len(), 40);

    // Both edit at once: A reseeds while B moves the object; they still agree.
    a.s.execute("procedural.reseed", &json!({"id": id.0})).unwrap();
    b.s.execute("select.set", &json!({"ids": [id.0]})).unwrap();
    b.s.execute("object.move", &json!({"dx": 25, "dy": 0})).unwrap();
    relay.pump(&mut [&mut a, &mut b]);
    assert_eq!(canon(a.doc()), canon(b.doc()));
    // Ids stay unique on both sides.
    for c in [&a, &b] {
        let mut seen = std::collections::BTreeSet::new();
        for l in &c.doc().doc.layers {
            l.walk(&mut |n| assert!(seen.insert(n.id), "duplicate id {:?}", n.id));
        }
    }
}
