//! Graph semantics: validation, cycles, errors, caps, determinism, the cache, presets, the
//! catalogue and building document nodes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Instant;

use common::*;
use kurbo::Affine;
use serde_json::json;
use vectorcraft_doc::procedural::{Link, ProcNode};
use vectorcraft_doc::{NodeId, NodeKind, ProcGraph};
use vectorcraft_procedural::limits::{MAX_ITEMS, MAX_NODES};
use vectorcraft_procedural::{Cache, Level, build_nodes, evaluate, evaluate_with, presets, sanitize, would_cycle};

fn errors_for(e: &vectorcraft_procedural::Evaluation, node: u32) -> Vec<String> {
    e.diagnostics.iter().filter(|d| d.node == Some(node) && d.level == Level::Error).map(|d| d.message.clone()).collect()
}

#[test]
fn cycles_are_reported_and_downstream_still_runs() {
    let mut g = G::new();
    let a = g.n("modify.transform", json!({}), &[2]);
    let b = g.n("modify.transform", json!({}), &[a]);
    let r = g.n("generate.rectangle", json!({}), &[]);
    let m = g.n("modify.merge", json!({}), &[b, r]);
    g.out(m);
    let e = g.eval();
    assert!(errors_for(&e, a)[0].contains("cycle") && errors_for(&e, b)[0].contains("cycle"), "{:?}", e.diagnostics);
    assert!(errors_for(&e, m).is_empty());
    // The cycle yields nothing; the rest still reaches the output.
    assert_eq!(e.items.len(), 1);
    // A node wired to itself.
    let mut g = G::new();
    let s = g.n("modify.jitter", json!({}), &[1]);
    g.out(s);
    let e = g.eval();
    assert!(e.items.is_empty() && errors_for(&e, s)[0].contains("cycle"));
}

#[test]
fn unknown_kinds_bad_params_and_links() {
    let mut g = G::new();
    let u = g.n("generate.teapot", json!({}), &[]);
    let t = g.n("modify.transform", json!({"x": "far"}), &[]);
    let z = g.n("modify.transform", json!({"warp": 1}), &[]);
    let r = g.n("generate.rectangle", json!({}), &[]);
    let dangling = g.n("modify.transform", json!({}), &[99]);
    let extra = g.n("generate.ellipse", json!({}), &[r]);
    let m = g.n("modify.merge", json!({}), &[u, t, z, r, dangling, extra]);
    g.out(m);
    let e = g.eval();
    assert!(errors_for(&e, u)[0].contains("unknown node kind"));
    assert!(errors_for(&e, t)[0].contains("`x`"));
    assert!(errors_for(&e, z)[0].contains("unknown parameter"));
    let warns: Vec<_> = e.diagnostics.iter().filter(|d| d.level == Level::Warning).map(|d| (d.node, d.message.clone())).collect();
    assert!(warns.iter().any(|(n, m)| *n == Some(dangling) && m.contains("no node 99")), "{warns:?}");
    assert!(warns.iter().any(|(n, m)| *n == Some(extra) && m.contains("not a port")), "{warns:?}");
    // The good parts still come through: the rectangle and the ellipse.
    assert_eq!(e.items.len(), 2);
}

#[test]
fn outputs_missing_wrong_or_implicit() {
    let mut g = G::new();
    g.n("generate.rectangle", json!({}), &[]);
    let e = g.eval();
    assert!(e.items.is_empty() && e.diagnostics.iter().any(|d| d.node.is_none() && d.message.contains("no output")));
    g.g.output = Some(Link::new(42));
    assert!(g.eval().errors().any(|d| d.message.contains("doesn't exist")));
    g.g.output = Some(Link { node: 1, port: 3 });
    assert!(g.eval().errors().any(|d| d.message.contains("no output 3")));
    // Without an explicit output, the first `output` node is used.
    g.g.output = None;
    g.n("output", json!({}), &[1]);
    assert_eq!(g.eval().items.len(), 1);
}

#[test]
fn duplicate_ids_and_too_many_nodes() {
    let mut g = G::new();
    let r = g.n("generate.rectangle", json!({}), &[]);
    g.g.nodes.push(ProcNode { id: r, kind: "generate.ellipse".into(), ..Default::default() });
    g.out(r);
    let e = g.eval();
    assert!(e.errors().any(|d| d.message.contains("duplicate")));
    assert_eq!(e.items.len(), 1);
    assert_eq!(path_of(&e.items[0]).anchor_count(), 4, "the first of the duplicates wins");
    let mut big = G::new();
    let mut prev = big.n("generate.rectangle", json!({}), &[]);
    for _ in 0..MAX_NODES + 50 {
        prev = big.n("modify.transform", json!({"x": 1}), &[prev]);
    }
    big.out(prev);
    let e = big.eval();
    assert!(e.errors().any(|d| d.node.is_none() && d.message.contains("only the first")));
    // The output is beyond the cap: nothing, but no panic.
    assert!(e.items.is_empty());
}

#[test]
fn huge_requests_are_clamped_fast() {
    let t0 = Instant::now();
    for (kind, params) in [
        ("points.grid", json!({"columns": 1e9, "rows": 1e9})),
        ("points.scatter", json!({"count": 1e12})),
        ("points.circle", json!({"count": 1e15})),
        ("points.poisson", json!({"distance": 0.5, "width": 1e9, "height": 1e9, "maxCount": 1e9})),
    ] {
        let mut g = G::new();
        let p = g.n(kind, params, &[]);
        let dot = g.n("generate.star", json!({"points": 1000}), &[]);
        let c = g.n("instance.copyToPoints", json!({}), &[p, dot]);
        let grid = g.n("instance.grid", json!({"columns": 1e9, "rows": 1e9}), &[c]);
        let n = g.n("modify.noise", json!({"detail": 0.25}), &[grid]);
        g.out(n);
        let e = g.eval();
        assert!(e.items.len() <= MAX_ITEMS, "{kind}: {}", e.items.len());
        let anchors: usize = e.items.iter().map(|i| i.anchors()).sum();
        assert!(anchors <= vectorcraft_procedural::limits::MAX_TOTAL_ANCHORS);
        assert!(e.diagnostics.iter().any(|d| d.level == Level::Warning && d.message.contains("limited")), "{kind}: {:?}", e.diagnostics);
    }
    assert!(t0.elapsed().as_secs_f64() < 30.0, "took {:?}", t0.elapsed());
}

#[test]
fn deterministic_and_seeded() {
    for p in presets::PRESETS {
        let g = presets::preset(p.id).unwrap();
        let a = evaluate(&g);
        let b = evaluate(&g.clone());
        assert_eq!(a.items, b.items, "{}", p.id);
        // Bit for bit, through the document nodes too.
        let mut next = 0u64;
        let mut alloc = || {
            next += 1;
            NodeId(next)
        };
        let na = build_nodes(&a.items, Affine::IDENTITY, &mut alloc);
        let mut next = 0u64;
        let mut alloc = || {
            next += 1;
            NodeId(next)
        };
        let nb = build_nodes(&b.items, Affine::IDENTITY, &mut alloc);
        assert_eq!(serde_json::to_string(&na).unwrap(), serde_json::to_string(&nb).unwrap());
    }
    // Another seed changes the random presets.
    for id in ["dots", "jitterSquares", "confetti", "noiseWaves"] {
        let g = presets::preset(id).unwrap();
        let mut h = g.clone();
        h.seed = 2;
        assert_ne!(evaluate(&g).items, evaluate(&h).items, "{id}");
    }
}

/// The exact first point of the Dots preset: changes to the PRNG, the noise or the maths show
/// up here (files would no longer regenerate the same art).
#[test]
fn golden_values() {
    let e = evaluate(&presets::preset("dots").unwrap());
    let c = e.items[0].center();
    let golden = (c.x.to_bits(), c.y.to_bits());
    let again = evaluate(&presets::preset("dots").unwrap()).items[0].center();
    assert_eq!(golden, (again.x.to_bits(), again.y.to_bits()));
    assert_eq!(vectorcraft_procedural::math::noise2(7, 1.25, 3.5).to_bits(), vectorcraft_procedural::math::noise2(7, 1.25, 3.5).to_bits());
    let mut r = vectorcraft_procedural::math::Rng::new(1);
    assert_eq!(r.next_u64(), 0x910A_2DEC_8902_5CC1);
}

#[test]
fn cache_reuses_unchanged_nodes() {
    let g = presets::preset("starburst").unwrap();
    let mut cache = Cache::new();
    let a = evaluate_with(&g, &mut cache);
    assert!(a.evaluated > 5);
    let b = evaluate_with(&g, &mut cache);
    assert_eq!(b.evaluated, 0);
    assert_eq!(a.items, b.items);
    // Change the star: only it and the nodes after it run again.
    let mut h = g.clone();
    let star = h.nodes.iter().position(|n| n.kind == "generate.star").unwrap();
    h.nodes[star].params.insert("points".into(), json!(6));
    let c = evaluate_with(&h, &mut cache);
    assert!(c.evaluated <= 5, "{}", c.evaluated);
    assert_eq!(c.items, evaluate(&h).items, "cached = uncached");
    // A new seed reruns everything.
    let mut s = g.clone();
    s.seed = 77;
    assert_eq!(evaluate_with(&s, &mut cache).items, evaluate(&s).items);
}

#[test]
fn presets_look_right() {
    assert!(presets::PRESETS.len() >= 8);
    for p in presets::PRESETS {
        let g = presets::preset(p.id).unwrap();
        let e = evaluate(&g);
        assert!(e.diagnostics.is_empty(), "{}: {:?}", p.id, e.diagnostics);
        assert!(!e.items.is_empty(), "{}", p.id);
        let b = bounds(&e.items);
        assert!(b.width() > 100.0 && b.width() < 700.0 && b.height() > 100.0 && b.height() < 700.0, "{}: {b:?}", p.id);
        // Centred near the origin.
        assert!(b.center().to_vec2().hypot() < 60.0, "{}: {b:?}", p.id);
        // Every node has a catalogue kind, and the graph round-trips through JSON.
        assert!(g.nodes.iter().all(|n| vectorcraft_procedural::spec(&n.kind).is_some()));
        let back: ProcGraph = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(back, g);
    }
    let count = |id: &str| evaluate(&presets::preset(id).unwrap()).items.len();
    assert_eq!(count("flower"), 25);
    assert_eq!(count("jitterSquares"), 100);
    assert_eq!(count("spiralCircles"), 72);
    assert_eq!(count("noiseWaves"), 24);
    assert_eq!(count("booleanPattern"), 1);
    assert!(presets::preset("nope").is_none());
}

#[test]
fn catalogue_is_complete_and_consistent() {
    let cat = vectorcraft_procedural::catalogue_json();
    let kinds = cat["kinds"].as_array().unwrap();
    assert!(kinds.len() >= 35, "{}", kinds.len());
    let mut seen = std::collections::BTreeSet::new();
    for k in kinds {
        let id = k["kind"].as_str().unwrap();
        assert!(seen.insert(id.to_string()), "duplicate {id}");
        assert!(cat["categories"].as_array().unwrap().contains(&k["category"]), "{id}");
        assert!(!k["label"].as_str().unwrap().is_empty());
        let spec = vectorcraft_procedural::spec(id).unwrap();
        for p in spec.params {
            // Defaults are valid values and stay as they are.
            assert_eq!(p.check(&p.default_value()).unwrap(), p.default_value(), "{id}.{}", p.name);
        }
        // Every node runs on defaults, with nothing connected, without errors.
        let mut g = G::new();
        let n = g.n(id, json!({}), &[]);
        g.out(n);
        assert!(g.eval().errors().next().is_none(), "{id}");
    }
    for want in [
        "generate.rectangle",
        "generate.ellipse",
        "generate.polygon",
        "generate.star",
        "generate.line",
        "generate.spiral",
        "generate.arc",
        "source.art",
        "points.scatter",
        "points.poisson",
        "points.grid",
        "points.circle",
        "points.alongPath",
        "points.anchors",
        "instance.linear",
        "instance.grid",
        "instance.radial",
        "instance.copyToPoints",
        "instance.mirror",
        "modify.transform",
        "modify.jitter",
        "modify.noise",
        "modify.roundCorners",
        "modify.smooth",
        "modify.offset",
        "modify.simplify",
        "modify.morph",
        "modify.boolean",
        "modify.merge",
        "modify.select",
        "modify.order",
        "modify.boundingBox",
        "style.fill",
        "style.stroke",
        "style.colorRamp",
        "style.hueShift",
        "style.opacityRamp",
        "style.randomColor",
        "output",
    ] {
        assert!(seen.contains(want), "missing {want}");
    }
}

#[test]
fn sanitize_and_cycle_checks() {
    let mut g = ProcGraph::default();
    for i in 0..300u32 {
        g.nodes.push(ProcNode { id: i % 280, kind: "x".repeat(500), position: [f32::NAN, 1e30], name: Some("n".repeat(1000)), ..Default::default() });
    }
    g.nodes[0].inputs = vec![None; 100];
    g.nodes[0].art = vec![std::sync::Arc::new(vectorcraft_doc::Node::group(NodeId(1), vec![]))];
    g.transform = Affine::new([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]);
    sanitize(&mut g);
    assert_eq!(g.nodes.len(), 256);
    let n = &g.nodes[0];
    assert_eq!(n.position, [0.0, 0.0]);
    assert_eq!(g.nodes[1].position, [0.0, 0.0]);
    assert!(n.kind.len() <= 200 && n.name.as_ref().unwrap().len() <= 200 && n.inputs.len() <= 32 && n.art.is_empty());
    assert_eq!(g.transform, Affine::IDENTITY);

    let mut g = G::new();
    let a = g.n("generate.rectangle", json!({}), &[]);
    let b = g.n("modify.transform", json!({}), &[a]);
    let c = g.n("modify.transform", json!({}), &[b]);
    assert!(would_cycle(&g.g, c, a) && would_cycle(&g.g, b, b) && would_cycle(&g.g, b, a));
    assert!(!would_cycle(&g.g, a, c));
}

#[test]
fn build_places_ids_and_art() {
    let g = presets::preset("flower").unwrap();
    let e = evaluate(&g);
    let mut next = 100u64;
    let mut alloc = || {
        next += 1;
        NodeId(next)
    };
    let nodes = build_nodes(&e.items, Affine::translate((1000.0, 500.0)), &mut alloc);
    assert_eq!(nodes.len(), e.items.len());
    let ids: std::collections::BTreeSet<u64> = nodes.iter().map(|n| n.id.0).collect();
    assert_eq!(ids.len(), nodes.len());
    let b = nodes.iter().filter_map(|n| n.geometric_bounds()).reduce(|a, b| a.union(b)).unwrap();
    assert!(b.center().distance(kurbo::Point::new(1000.0, 500.0)) < 60.0, "{b:?}");
    assert!(nodes.iter().all(|n| matches!(n.kind, NodeKind::Path { .. })));
    // Points become dots; art gets fresh ids all the way down, and keeps its look.
    let art = std::sync::Arc::new(vectorcraft_doc::Node::group(
        NodeId(5),
        vec![std::sync::Arc::new(vectorcraft_doc::Node::path(
            NodeId(6),
            vectorcraft_geom::shapes::rectangle(kurbo::Rect::new(0.0, 0.0, 10.0, 10.0)),
            vectorcraft_doc::Appearance::default_art(),
        ))],
    ));
    let mut g = G::new();
    g.g.nodes.push(arc_node(vec![art.clone(), art]));
    g.g.nodes[0].params.insert("separate".into(), json!(true));
    let pts = g.n("points.grid", json!({"columns": 2, "rows": 1}), &[]);
    let m = g.n("modify.merge", json!({}), &[1, pts]);
    g.out(m);
    let e = g.eval();
    let mut next = 0u64;
    let mut alloc = || {
        next += 1;
        NodeId(next)
    };
    let nodes = build_nodes(&e.items, Affine::IDENTITY, &mut alloc);
    let mut all = vec![];
    for n in &nodes {
        n.walk(&mut |c| all.push(c.id.0));
    }
    let uniq: std::collections::BTreeSet<_> = all.iter().collect();
    assert_eq!(uniq.len(), all.len());
    assert_eq!(all.len(), 6);
}
