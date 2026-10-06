//! Procedural objects through the engine: create (presets, graphs, the selection), every edit
//! command, undo/redo, live slider drags as one undo step, placement kept across regeneration,
//! expand, copy/paste, duplicate, native save/reopen, determinism and caps.

use serde_json::{Value, json};
use vectorcraft_doc::{Node, NodeId, NodeKind, ProcGraph};
use vectorcraft_geom::Rect;

use super::*;

fn session() -> Session {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 800, "height": 600})).unwrap();
    s
}

fn create(s: &mut Session, p: Value) -> NodeId {
    let r = s.execute("procedural.create", &p).unwrap();
    assert_eq!(r["errors"], json!([]), "{r}");
    NodeId(r["id"].as_u64().unwrap())
}

fn node(s: &Session, id: NodeId) -> Node {
    s.doc().unwrap().doc.node(id).unwrap().clone()
}

fn graph(s: &Session, id: NodeId) -> ProcGraph {
    node(s, id).procedural.as_deref().cloned().unwrap()
}

fn kids(s: &Session, id: NodeId) -> usize {
    node(s, id).children().unwrap().len()
}

fn bounds(s: &Session, id: NodeId) -> Rect {
    node(s, id).geometric_bounds().unwrap()
}

fn undo(s: &mut Session) {
    s.execute("edit.undo", &json!({})).unwrap();
}

fn find_kind(g: &ProcGraph, kind: &str) -> u32 {
    g.nodes.iter().find(|n| n.kind == kind).unwrap().id
}

/// The document is well formed and survives the native format and SVG.
fn sane(s: &Session) {
    let st = s.doc().unwrap();
    assert!(st.interaction.is_none());
    vectorcraft_testkit::invariants::check_document(&st.doc).unwrap();
    vectorcraft_testkit::invariants::check_native_roundtrip(&st.doc).unwrap();
    vectorcraft_testkit::invariants::check_svg_roundtrip(&st.doc).unwrap();
}

#[test]
fn every_preset_makes_an_object_on_the_artboard() {
    let mut s = session();
    let list = s.execute("procedural.presets", &json!({})).unwrap();
    let ids: Vec<String> = list.as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap().to_string()).collect();
    assert!(ids.len() >= 8);
    for p in &ids {
        let id = create(&mut s, json!({"preset": p}));
        let n = node(&s, id);
        assert!(n.procedural.is_some() && matches!(n.kind, NodeKind::Group { .. }));
        assert!(kids(&s, id) > 0, "{p}");
        // Centred on the artboard's centre by default.
        assert!(bounds(&s, id).center().distance(vectorcraft_geom::Point::new(400.0, 300.0)) < 60.0, "{p}");
        assert_eq!(s.doc().unwrap().selection.objects, vec![id]);
    }
    sane(&s);
    let c = s.execute("procedural.catalogue", &json!({})).unwrap();
    assert!(c["kinds"].as_array().unwrap().len() >= 35);
    assert!(s.execute("procedural.create", &json!({"preset": "nope"})).is_err());
}

#[test]
fn create_at_a_point_undo_and_redo() {
    let mut s = session();
    let id = create(&mut s, json!({"preset": "jitterSquares", "x": 100, "y": 150, "name": "Tiles"}));
    assert_eq!(node(&s, id).name.as_deref(), Some("Tiles"));
    assert_eq!(kids(&s, id), 100);
    assert!(bounds(&s, id).center().distance(vectorcraft_geom::Point::new(100.0, 150.0)) < 10.0);
    undo(&mut s);
    assert!(s.doc().unwrap().doc.node(id).is_none());
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(kids(&s, id), 100);
    assert!(s.execute("procedural.create", &json!({"x": 1, "y": f64::MAX})).is_err());
    assert!(s.execute("procedural.create", &json!({"x": 1})).is_err());
}

#[test]
fn the_selection_becomes_source_art_in_place() {
    let mut s = session();
    let a = NodeId(s.execute("shape.rectangle", &json!({"x": 100, "y": 100, "width": 50, "height": 20})).unwrap()["id"].as_u64().unwrap());
    let b = NodeId(s.execute("shape.ellipse", &json!({"x": 200, "y": 100, "width": 40, "height": 40})).unwrap()["id"].as_u64().unwrap());
    s.execute("select.set", &json!({"ids": [a.0, b.0]})).unwrap();
    let id = create(&mut s, json!({}));
    let d = &s.doc().unwrap().doc;
    assert!(d.node(a).is_none() && d.node(b).is_none(), "the art moved into the graph");
    let g = graph(&s, id);
    assert_eq!(g.nodes[0].kind, "source.art");
    assert_eq!(g.nodes[0].art.len(), 2);
    // The object shows the art exactly where it was.
    let r = bounds(&s, id);
    assert!((r.x0 - 100.0).abs() < 1e-6 && (r.y0 - 100.0).abs() < 1e-6 && (r.x1 - 240.0).abs() < 1e-6 && (r.y1 - 140.0).abs() < 1e-6, "{r:?}");
    // Feed it into a radial repeat.
    let src = g.nodes[0].id;
    let out = find_kind(&g, "output");
    let rad = s
        .execute("procedural.addNode", &json!({"kind": "instance.radial", "params": {"count": 6, "radius": 80}, "position": [200, 0]}))
        .unwrap()["node"]
        .as_u64()
        .unwrap();
    s.execute("procedural.connect", &json!({"from": src, "to": rad})).unwrap();
    s.execute("procedural.connect", &json!({"from": rad, "to": out})).unwrap();
    assert_eq!(kids(&s, id), 6);
    // Each copy is the two objects as a group, with ids of their own.
    let n = node(&s, id);
    let mut ids = std::collections::BTreeSet::new();
    n.walk(&mut |c| assert!(ids.insert(c.id), "duplicate id {:?}", c.id));
    assert_eq!(ids.len(), 1 + 6 * 3);
    sane(&s);
    // Undo puts the original objects back.
    for _ in 0..4 {
        undo(&mut s);
    }
    assert!(s.doc().unwrap().doc.node(a).is_some() && s.doc().unwrap().doc.node(id).is_none());
}

#[test]
fn graph_editing_commands() {
    let mut s = session();
    let id = create(&mut s, json!({"preset": "flower"}));
    let g = graph(&s, id);
    let out = find_kind(&g, "output");
    let before = bounds(&s, id);
    // A transform node between the last node and the output.
    let last = g.nodes.iter().find(|n| n.id == out).unwrap().inputs[0].unwrap().node;
    let r = s.execute("procedural.addNode", &json!({"kind": "modify.transform", "params": {"x": 100}, "name": "Shift"})).unwrap();
    let t = r["node"].as_u64().unwrap() as u32;
    assert_eq!(graph(&s, id).node(t).unwrap().name.as_deref(), Some("Shift"));
    s.execute("procedural.connect", &json!({"from": last, "to": t})).unwrap();
    s.execute("procedural.connect", &json!({"from": t, "to": out, "port": 0})).unwrap();
    assert!((bounds(&s, id).x0 - before.x0 - 100.0).abs() < 1e-6);
    // Params: set, clamp, reset, refuse.
    s.execute("procedural.setParam", &json!({"node": t, "param": "x", "value": 50})).unwrap();
    assert!((bounds(&s, id).x0 - before.x0 - 50.0).abs() < 1e-6);
    s.execute("procedural.setParam", &json!({"node": t, "params": {"x": 1e12, "y": 0}})).unwrap();
    assert_eq!(graph(&s, id).node(t).unwrap().params["x"], json!(100000.0));
    s.execute("procedural.setParam", &json!({"node": t, "param": "x", "value": null})).unwrap();
    assert!(!graph(&s, id).node(t).unwrap().params.contains_key("x"));
    let rev = s.doc().unwrap().revision;
    for bad in [
        json!({"node": t, "param": "x", "value": "far"}),
        json!({"node": t, "param": "nope", "value": 1}),
        json!({"node": 999, "param": "x", "value": 1}),
        json!({"node": t}),
        json!({"node": t, "param": "pivot", "value": "elsewhere"}),
    ] {
        assert!(s.execute("procedural.setParam", &bad).is_err(), "{bad}");
    }
    assert_eq!(s.doc().unwrap().revision, rev, "refused edits change nothing");
    // Wires: cycles and bad ports are refused.
    assert!(s.execute("procedural.connect", &json!({"from": out, "to": t})).is_err());
    assert!(s.execute("procedural.connect", &json!({"from": t, "to": t})).is_err());
    assert!(s.execute("procedural.connect", &json!({"from": last, "to": t, "port": 5})).is_err());
    assert!(s.execute("procedural.connect", &json!({"from": 999, "to": t})).is_err());
    // Bypass, rename, move in the editor.
    s.execute("procedural.setNode", &json!({"node": t, "bypass": true, "position": [10, 20], "name": null})).unwrap();
    let n = graph(&s, id).node(t).unwrap().clone();
    assert!(n.bypass && n.position == [10.0, 20.0] && n.name.is_none());
    assert!((bounds(&s, id).x0 - before.x0).abs() < 1e-6);
    assert!(s.execute("procedural.setNode", &json!({"node": t, "position": [f64::NAN, 1]})).is_err());
    // Disconnect the output: no art, and a warning-free empty result.
    s.execute("procedural.disconnect", &json!({"to": out})).unwrap();
    assert_eq!(kids(&s, id), 0);
    s.execute("procedural.setOutput", &json!({"node": t})).unwrap();
    assert!(kids(&s, id) > 0);
    s.execute("procedural.removeNode", &json!({"node": t})).unwrap();
    let g = graph(&s, id);
    assert!(g.node(t).is_none() && g.output.is_none());
    assert!(s.execute("procedural.removeNode", &json!({"node": t})).is_err());
    // Seeds.
    let r = s.execute("procedural.setSeed", &json!({"seed": 42})).unwrap();
    assert_eq!(r["seed"], json!(42));
    let r = s.execute("procedural.reseed", &json!({})).unwrap();
    assert_ne!(r["seed"], json!(42));
    assert!(r["seed"].as_u64().unwrap() < 1 << 53);
    assert!(s.execute("procedural.setSeed", &json!({"seed": -1})).is_err());
    // Unknown kinds are refused when added…
    assert!(s.execute("procedural.addNode", &json!({"kind": "generate.teapot"})).is_err());
    assert!(s.execute("procedural.addNode", &json!({"kind": "generate.star", "params": {"points": "many"}})).is_err());
    // …but a graph that has one reports it per node.
    let mut g = graph(&s, id);
    g.nodes.push(vectorcraft_doc::ProcNode { id: 500, kind: "generate.teapot".into(), ..Default::default() });
    let r = s.execute("procedural.setGraph", &json!({"graph": g})).unwrap();
    let errs = r["errors"].as_array().unwrap();
    assert!(errs.iter().any(|e| e["node"] == json!(500) && e["level"] == json!("error")), "{errs:?}");
    assert_eq!(s.execute("procedural.errors", &json!({})).unwrap()["errors"], r["errors"]);
    let got = s.execute("procedural.get", &json!({"id": id.0})).unwrap();
    assert_eq!(got["graph"]["nodes"].as_array().unwrap().len(), g.nodes.len());
    sane(&s);
}

#[test]
fn targets_the_selection_or_its_ancestor() {
    let mut s = session();
    let id = create(&mut s, json!({"preset": "flower"}));
    let child = node(&s, id).children().unwrap()[3].id;
    s.execute("select.set", &json!({"ids": [child.0]})).unwrap();
    assert_eq!(s.execute("procedural.get", &json!({})).unwrap()["id"], json!(id.0));
    s.execute("select.set", &json!({"ids": []})).unwrap();
    assert!(s.execute("procedural.get", &json!({})).is_err());
    let r = NodeId(s.execute("shape.rectangle", &json!({"x": 1, "y": 1, "width": 5, "height": 5})).unwrap()["id"].as_u64().unwrap());
    assert!(s.execute("procedural.get", &json!({"id": r.0})).is_err());
    assert!(s.execute("procedural.get", &json!({"id": 123456})).is_err());
}

#[test]
fn a_slider_drag_is_one_undo_step() {
    let mut s = session();
    let id = create(&mut s, json!({"preset": "spiralCircles"}));
    let g = graph(&s, id);
    let rep = find_kind(&g, "instance.linear");
    let steps = s.doc().unwrap().history.undo.len();
    s.begin_interaction("Count").unwrap();
    for n in [10, 20, 30, 40] {
        s.preview("procedural.setParam", &json!({"id": id.0, "node": rep, "param": "count", "value": n})).unwrap();
        assert_eq!(kids(&s, id), n);
    }
    s.commit_interaction().unwrap();
    assert_eq!(s.doc().unwrap().history.undo.len(), steps + 1);
    assert_eq!(kids(&s, id), 40);
    undo(&mut s);
    assert_eq!(kids(&s, id), 72);
    // A cancelled drag leaves nothing behind.
    s.begin_interaction("Count").unwrap();
    s.preview("procedural.setParam", &json!({"id": id.0, "node": rep, "param": "count", "value": 3})).unwrap();
    s.cancel_interaction().unwrap();
    assert_eq!(kids(&s, id), 72);
}

#[test]
fn moves_rotations_and_scales_survive_regeneration() {
    let mut s = session();
    let id = create(&mut s, json!({"preset": "jitterSquares", "x": 300, "y": 300}));
    let jit = find_kind(&graph(&s, id), "modify.jitter");
    s.execute("select.set", &json!({"ids": [id.0]})).unwrap();
    s.execute("object.move", &json!({"dx": 120, "dy": -40})).unwrap();
    let moved = bounds(&s, id);
    s.execute("procedural.setParam", &json!({"node": jit, "param": "move", "value": 0})).unwrap();
    s.execute("procedural.setParam", &json!({"node": jit, "param": "rotate", "value": 0})).unwrap();
    let c = bounds(&s, id).center();
    assert!(c.distance(vectorcraft_geom::Point::new(420.0, 260.0)) < 1.0, "{c:?} (was {moved:?})");
    // Rotate 90°: a wide grid becomes tall, and stays so after an edit.
    let grid = find_kind(&graph(&s, id), "instance.grid");
    s.execute("procedural.setParam", &json!({"node": grid, "param": "rows", "value": 2})).unwrap();
    let wide = bounds(&s, id);
    assert!(wide.width() > wide.height() * 3.0);
    s.execute("object.rotate", &json!({"angle": 90})).unwrap();
    s.execute("procedural.setParam", &json!({"node": grid, "param": "rows", "value": 3})).unwrap();
    let tall = bounds(&s, id);
    assert!(tall.height() > tall.width() * 2.0, "{tall:?}");
    assert!(tall.center().distance(c) < 20.0, "{tall:?}");
    sane(&s);
}

#[test]
fn expand_detaches_the_art() {
    let mut s = session();
    let id = create(&mut s, json!({"preset": "dots"}));
    let before = node(&s, id).children().unwrap().clone();
    s.execute("procedural.expand", &json!({})).unwrap();
    let n = node(&s, id);
    assert!(n.procedural.is_none());
    assert_eq!(n.children().unwrap(), &before);
    assert!(s.execute("procedural.get", &json!({"id": id.0})).is_err());
    undo(&mut s);
    assert!(node(&s, id).procedural.is_some());
}

#[test]
fn copy_paste_duplicate_and_independent_edits() {
    let mut s = session();
    let id = create(&mut s, json!({"preset": "hexTiles"}));
    s.execute("edit.copy", &json!({})).unwrap();
    s.execute("edit.paste", &json!({})).unwrap();
    let pasted = s.doc().unwrap().selection.objects[0];
    assert_ne!(pasted, id);
    assert_eq!(graph(&s, pasted).nodes, graph(&s, id).nodes);
    s.execute("select.set", &json!({"ids": [id.0]})).unwrap();
    s.execute("edit.duplicate", &json!({"dx": 10, "dy": 10})).unwrap();
    let dup = s.doc().unwrap().selection.objects[0];
    assert!(dup != id && dup != pasted);
    // Editing one copy leaves the others alone.
    let grid = find_kind(&graph(&s, dup), "instance.grid");
    s.execute("procedural.setParam", &json!({"id": dup.0, "node": grid, "param": "columns", "value": 2})).unwrap();
    assert_eq!(kids(&s, dup), 18);
    assert_eq!(kids(&s, id), 108);
    assert_eq!(kids(&s, pasted), 108);
    // The duplicate kept its offset through the regeneration.
    let (bd, bo) = (bounds(&s, dup), bounds(&s, id));
    assert!((bd.center().y - bo.center().y - 10.0).abs() < 1.0, "{bd:?} {bo:?}");
    // Ids are unique across the document.
    let mut seen = std::collections::BTreeSet::new();
    for l in &s.doc().unwrap().doc.layers {
        l.walk(&mut |n| assert!(seen.insert(n.id), "duplicate id {:?}", n.id));
    }
    sane(&s);
}

#[test]
fn native_save_and_reopen() {
    let mut s = session();
    let id = create(&mut s, json!({"preset": "booleanPattern", "x": 250, "y": 200}));
    let src = s.execute("shape.rectangle", &json!({"x": 10, "y": 10, "width": 30, "height": 30})).unwrap()["id"].as_u64().unwrap();
    s.execute("select.set", &json!({"ids": [src]})).unwrap();
    let art = create(&mut s, json!({}));
    let saved = vectorcraft_format::base64_decode(s.execute("document.save", &json!({})).unwrap()["dataBase64"].as_str().unwrap()).unwrap();
    let before_g = graph(&s, id);
    let before_n = node(&s, id);
    let before_art = node(&s, art);
    s.execute("document.open", &json!({"name": "p.vectorcraft", "dataBase64": vectorcraft_format::base64_encode(&saved)})).unwrap();
    assert_eq!(graph(&s, id), before_g);
    assert_eq!(node(&s, id), before_n);
    assert_eq!(node(&s, art), before_art);
    // The reopened graph still edits, and regenerates the same art from the same graph.
    s.execute("procedural.setSeed", &json!({"id": id.0, "seed": before_g.seed})).unwrap();
    assert_eq!(node(&s, id).children(), before_n.children());
    let fill = find_kind(&graph(&s, id), "style.fill");
    s.execute("procedural.setParam", &json!({"id": id.0, "node": fill, "param": "color", "value": "#ff0000"})).unwrap();
    // Files without the field (older ones) load as before: a plain group stays plain.
    let plain = Node::group(NodeId(9), vec![]);
    let v = serde_json::to_value(&plain).unwrap();
    assert!(v.get("procedural").is_none());
    let back: Node = serde_json::from_value(v).unwrap();
    assert!(back.procedural.is_none());
    sane(&s);
}

#[test]
fn same_seed_same_art_other_seed_other_art() {
    let make = |seed: u64| {
        let mut s = session();
        let id = create(&mut s, json!({"preset": "confetti", "seed": seed}));
        serde_json::to_string(node(&s, id).children().unwrap()).unwrap()
    };
    assert_eq!(make(5), make(5));
    assert_ne!(make(5), make(6));
}

#[test]
fn huge_graphs_are_capped_quickly() {
    let mut s = session();
    let id = create(&mut s, json!({"preset": "dots"}));
    let g = graph(&s, id);
    let pts = find_kind(&g, "points.scatter");
    let t = std::time::Instant::now();
    s.execute("procedural.setParam", &json!({"node": pts, "param": "count", "value": 1e9})).unwrap();
    assert_eq!(kids(&s, id), vectorcraft_procedural::limits::MAX_ITEMS);
    // The value is clamped to the param's range as it is stored.
    assert_eq!(graph(&s, id).node(pts).unwrap().params["count"], json!(20_000));
    // Copies far off the canvas are left out instead of breaking the file.
    let mut g = graph(&s, id);
    g.nodes.push(vectorcraft_doc::ProcNode {
        id: 900,
        kind: "modify.transform".into(),
        params: json!({"x": 100000, "scaleX": 1000, "scaleY": 1000, "pivot": "origin"}).as_object().cloned().unwrap(),
        inputs: vec![g.output.and_then(|o| g.node(o.node)).and_then(|n| n.inputs[0])],
        ..Default::default()
    });
    g.nodes.push(vectorcraft_doc::ProcNode {
        id: 901,
        kind: "modify.transform".into(),
        params: json!({"scaleX": 1000, "scaleY": 1000, "pivot": "origin"}).as_object().cloned().unwrap(),
        inputs: vec![Some(vectorcraft_doc::procedural::Link::new(900))],
        ..Default::default()
    });
    g.output = Some(vectorcraft_doc::procedural::Link::new(901));
    let r = s.execute("procedural.setGraph", &json!({"graph": g})).unwrap();
    assert!(r["errors"].to_string().contains("outside the canvas"), "{r}");
    assert!(t.elapsed().as_secs_f64() < 60.0, "{:?}", t.elapsed());
    sane(&s);
}

#[test]
fn garbage_graphs_are_refused_or_reported() {
    let mut s = session();
    let id = create(&mut s, json!({"preset": "flower"}));
    for g in [json!(5), json!({"nodes": 5}), json!({"nodes": [{"id": -1}]}), json!({"seed": "x"})] {
        assert!(s.execute("procedural.setGraph", &json!({"id": id.0, "graph": g})).is_err(), "{g}");
    }
    // Structurally valid but meaningless graphs are stored and report their problems.
    let g = json!({"nodes": [{"id": 1, "kind": "modify.merge", "inputs": [{"node": 1}, {"node": 7}]}], "output": {"node": 1}});
    let r = s.execute("procedural.setGraph", &json!({"id": id.0, "graph": g})).unwrap();
    let text = r["errors"].to_string();
    assert!(text.contains("cycle") && text.contains("no node 7"), "{text}");
    assert_eq!(kids(&s, id), 0);
    sane(&s);
}
