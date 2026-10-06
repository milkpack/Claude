//! Every node kind on its own: counts, bounds, positions, determinism.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;

use common::*;
use kurbo::{ParamCurve, Point, Rect};
use serde_json::json;
use vectorcraft_color::{Color, Paint};
use vectorcraft_doc::{Appearance, Node, NodeId};
use vectorcraft_geom::shapes;
use vectorcraft_procedural::{Geom, Item, math};

fn solid(it: &Item) -> Color {
    match &it.style.fill {
        Some(Paint::Solid { color, .. }) => *color,
        f => panic!("not solid: {f:?}"),
    }
}

fn stroke_color(it: &Item) -> Color {
    match &it.style.stroke {
        Some(Paint::Solid { color, .. }) => *color,
        f => panic!("not solid: {f:?}"),
    }
}

// ---------- generate ----------

#[test]
fn rectangle_and_rounded() {
    let r = one("generate.rectangle", json!({"width": 80, "height": 40}));
    assert_eq!(r.len(), 1);
    assert_eq!(path_of(&r[0]).anchor_count(), 4);
    assert_rect(bounds(&r), -40.0, -20.0, 40.0, 20.0, 1e-9);
    let r = one("generate.rectangle", json!({"width": 80, "height": 40, "radius": 10}));
    assert_eq!(path_of(&r[0]).anchor_count(), 8);
    assert_rect(bounds(&r), -40.0, -20.0, 40.0, 20.0, 1e-9);
    // Defaults: a 100 pt square, white fill and black stroke.
    let r = one("generate.rectangle", json!({}));
    assert_rect(bounds(&r), -50.0, -50.0, 50.0, 50.0, 1e-9);
    assert_eq!(solid(&r[0]), Color::WHITE);
}

#[test]
fn ellipse_polygon_star() {
    let e = one("generate.ellipse", json!({"width": 60, "height": 20}));
    assert_rect(bounds(&e), -30.0, -10.0, 30.0, 10.0, 1e-9);
    let p = one("generate.polygon", json!({"sides": 6, "radius": 50}));
    let pd = path_of(&p[0]);
    assert_eq!(pd.anchor_count(), 6);
    // First corner straight up.
    let a0 = pd.subpaths[0].anchors[0].p;
    assert!(close(a0.x, 0.0, 1e-9) && close(a0.y, -50.0, 1e-9), "{a0:?}");
    for (_, _, a) in pd.anchors() {
        assert!(close(a.p.to_vec2().hypot(), 50.0, 1e-9));
    }
    let s = one("generate.star", json!({"points": 7, "radius": 40, "innerRatio": 0.25}));
    let pd = path_of(&s[0]);
    assert_eq!(pd.anchor_count(), 14);
    for (_, i, a) in pd.anchors() {
        let want = if i % 2 == 0 { 40.0 } else { 10.0 };
        assert!(close(a.p.to_vec2().hypot(), want, 1e-9));
    }
    // Clamped counts.
    assert_eq!(path_of(&one("generate.polygon", json!({"sides": 1e9}))[0]).anchor_count(), 1000);
    assert_eq!(path_of(&one("generate.star", json!({"points": -5}))[0]).anchor_count(), 4);
}

#[test]
fn line_spiral_arc() {
    let l = one("generate.line", json!({"x1": 1, "y1": 2, "x2": 11, "y2": 2}));
    let pd = path_of(&l[0]);
    assert!(!pd.subpaths[0].closed);
    assert_eq!(pd.subpaths[0].anchors.iter().map(|a| a.p).collect::<Vec<_>>(), vec![Point::new(1.0, 2.0), Point::new(11.0, 2.0)]);
    assert_eq!(l[0].style.fill, Some(Paint::None));

    let s = one("generate.spiral", json!({"turns": 3, "innerRadius": 10, "outerRadius": 100}));
    let pd = path_of(&s[0]);
    assert_eq!(pd.anchor_count(), 25);
    let first = pd.subpaths[0].anchors[0].p;
    let last = pd.subpaths[0].anchors.last().unwrap().p;
    assert!(close(first.to_vec2().hypot(), 10.0, 1e-9) && close(last.to_vec2().hypot(), 100.0, 1e-9));
    let b = bounds(&s);
    assert!(b.width() > 180.0 && b.width() < 205.0, "{b:?}");
    // Its curve stays close to the true spiral: radius grows with the angle.
    let mid = pd.subpaths[0].segment(4).eval(0.5);
    let ang = std::f64::consts::TAU * 4.5 / 8.0;
    let want = 10.0 + 90.0 * ang / (std::f64::consts::TAU * 3.0);
    assert!(close(mid.to_vec2().hypot(), want, 0.5), "{} vs {want}", mid.to_vec2().hypot());
    let e = one("generate.spiral", json!({"growth": "exponential", "innerRadius": 5, "outerRadius": 80, "clockwise": true}));
    let last = path_of(&e[0]).subpaths[0].anchors.last().unwrap().p;
    assert!(close(last.to_vec2().hypot(), 80.0, 1e-6));

    let a = one("generate.arc", json!({"radius": 50, "startAngle": 0, "sweep": 90}));
    let pd = path_of(&a[0]);
    let sp = &pd.subpaths[0];
    assert!(close(sp.anchors[0].p.x, 50.0, 1e-9) && close(sp.anchors[1].p.y, -50.0, 1e-9));
    let mid = sp.segment(0).eval(0.5);
    assert!(close(mid.to_vec2().hypot(), 50.0, 0.02), "{mid:?}");
    let pie = one("generate.arc", json!({"radius": 50, "sweep": -270, "closure": "pie"}));
    let pd = path_of(&pie[0]);
    assert!(pd.subpaths[0].closed && pd.anchor_count() == 5);
    assert!(one("generate.arc", json!({"sweep": 0})).is_empty());
}

#[test]
fn source_art_whole_and_separate() {
    let a = Arc::new(Node::path(NodeId(7), shapes::rectangle(Rect::new(0.0, 0.0, 10.0, 10.0)), Appearance::default_art()));
    let b = Arc::new(Node::path(NodeId(8), shapes::ellipse(Rect::new(20.0, 0.0, 30.0, 10.0)), Appearance::default_art()));
    let mut g = G::new();
    let mut n = arc_node(vec![a.clone(), b.clone()]);
    n.id = 1;
    g.g.nodes.push(n);
    let items = g.items(1);
    assert_eq!(items.len(), 1);
    assert!(matches!(items[0].geom, Geom::Art { weight: 3, .. }));
    assert_rect(bounds(&items), 0.0, 0.0, 30.0, 10.0, 1e-9);
    g.g.nodes[0].params.insert("separate".into(), json!(true));
    let items = g.items(1);
    assert_eq!(items.len(), 2);
    // Art feeds geometry nodes as paths.
    let mut g2 = G::new();
    g2.g.nodes.push(arc_node(vec![a, b]));
    let pts = g2.n("points.anchors", json!({}), &[1]);
    assert_eq!(g2.items(pts).len(), 8);
}

// ---------- points ----------

#[test]
fn scatter_in_rect_and_in_shape() {
    let p = one("points.scatter", json!({"count": 500, "width": 100, "height": 50}));
    assert_eq!(p.len(), 500);
    assert!(p.iter().all(|it| matches!(it.geom, Geom::Point)));
    let b = bounds(&p);
    assert!(b.x0 >= -50.0 && b.x1 <= 50.0 && b.y0 >= -25.0 && b.y1 <= 25.0);
    assert!(b.width() > 90.0 && b.height() > 45.0, "spread {b:?}");
    let q = one("points.scatter", json!({"count": 500, "width": 100, "height": 50, "seed": 3}));
    assert_ne!(origins(&p), origins(&q));
    assert_eq!(origins(&p), origins(&one("points.scatter", json!({"count": 500, "width": 100, "height": 50}))));
    // Inside a circle.
    let mut g = G::new();
    let c = g.n("generate.ellipse", json!({"width": 100, "height": 100}), &[]);
    let s = g.n("points.scatter", json!({"count": 300}), &[c]);
    let pts = g.items(s);
    assert_eq!(pts.len(), 300);
    assert!(pts.iter().all(|it| it.origin().to_vec2().hypot() <= 50.01));
    // Random numbers per point are in [0, 1) and differ.
    assert!(pts.iter().all(|it| (0.0..1.0).contains(&it.attrs.random)));
    assert_ne!(pts[0].attrs.random, pts[1].attrs.random);
}

#[test]
fn poisson_keeps_its_distance() {
    let p = one("points.poisson", json!({"distance": 10, "width": 200, "height": 100, "maxCount": 5000}));
    assert!(p.len() > 120, "only {}", p.len());
    let pts = origins(&p);
    for (i, a) in pts.iter().enumerate() {
        assert!(a.x >= -100.0 && a.x <= 100.0 && a.y >= -50.0 && a.y <= 50.0);
        for b in &pts[i + 1..] {
            assert!(a.distance(*b) >= 10.0 - 1e-9, "{a:?} {b:?}");
        }
    }
    assert_eq!(one("points.poisson", json!({"distance": 1, "width": 200, "height": 100, "maxCount": 50})).len(), 50);
    // Inside a shape.
    let mut g = G::new();
    let c = g.n("generate.ellipse", json!({"width": 100, "height": 100}), &[]);
    let s = g.n("points.poisson", json!({"distance": 8}), &[c]);
    let pts = g.items(s);
    assert!(pts.len() > 50 && pts.iter().all(|it| it.origin().to_vec2().hypot() <= 50.01));
}

#[test]
fn grid_and_circle_points() {
    let p = one("points.grid", json!({"columns": 4, "rows": 3, "spacingX": 10, "spacingY": 20}));
    assert_eq!(p.len(), 12);
    assert_rect(bounds(&p), -15.0, -20.0, 15.0, 20.0, 1e-9);
    let s = one("points.grid", json!({"columns": 4, "rows": 2, "spacingX": 10, "stagger": true}));
    assert_eq!(s[4].origin().x - s[0].origin().x, 5.0);
    let c = one("points.circle", json!({"count": 8, "radius": 30}));
    assert_eq!(c.len(), 8);
    assert!(c.iter().all(|it| close(it.origin().to_vec2().hypot(), 30.0, 1e-9)));
    // The first point is at 12 o'clock by default.
    assert!(close(c[0].origin().y, -30.0, 1e-9));
}

#[test]
fn points_along_paths_and_anchors() {
    let mut g = G::new();
    let l = g.n("generate.line", json!({"x1": 0, "y1": 0, "x2": 100, "y2": 0}), &[]);
    let a = g.n("points.alongPath", json!({"count": 5}), &[l]);
    let pts = g.items(a);
    let xs: Vec<f64> = pts.iter().map(|p| p.origin().x).collect();
    for (x, want) in xs.iter().zip([0.0, 25.0, 50.0, 75.0, 100.0]) {
        assert!(close(*x, want, 1e-9), "{xs:?}");
    }
    let sp = g.n("points.alongPath", json!({"spacing": 30}), &[l]);
    assert_eq!(g.items(sp).len(), 4);
    let c = g.n("generate.ellipse", json!({"width": 100, "height": 100}), &[]);
    let ca = g.n("points.alongPath", json!({"count": 12}), &[c]);
    let pts = g.items(ca);
    assert_eq!(pts.len(), 12);
    assert!(pts.iter().all(|p| close(p.origin().to_vec2().hypot(), 50.0, 0.1)));
    // Turned along the path: on a vertical line the points face down the page.
    let v = g.n("generate.line", json!({"x1": 0, "y1": 0, "x2": 0, "y2": 100}), &[]);
    let va = g.n("points.alongPath", json!({"count": 2}), &[v]);
    let p = &g.items(va)[0];
    let d = p.transform * Point::new(1.0, 0.0) - p.origin();
    assert!(close(d.x, 0.0, 1e-9) && close(d.y, 1.0, 1e-9), "{d:?}");
    let r = g.n("generate.rectangle", json!({}), &[]);
    let an = g.n("points.anchors", json!({}), &[r]);
    assert_eq!(g.items(an).len(), 4);
}

// ---------- instance ----------

#[test]
fn repeat_linear_grid_radial() {
    let mut g = G::new();
    let r = g.n("generate.rectangle", json!({"width": 10, "height": 10}), &[]);
    let lin = g.n("instance.linear", json!({"count": 4, "offsetX": 20, "offsetY": 5}), &[r]);
    let items = g.items(lin);
    assert_eq!(items.len(), 4);
    assert_rect(bounds(&items), -5.0, -5.0, 65.0, 20.0, 1e-9);
    assert_eq!(items.iter().map(|i| i.attrs.index).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
    let sc = g.n("instance.linear", json!({"count": 3, "offsetX": 0, "scale": 0.5}), &[r]);
    let items = g.items(sc);
    assert!(close(items[2].bounds().unwrap().width(), 2.5, 1e-9));
    let grid = g.n("instance.grid", json!({"columns": 3, "rows": 2, "spacingX": 20, "spacingY": 30}), &[r]);
    let items = g.items(grid);
    assert_eq!(items.len(), 6);
    assert_rect(bounds(&items), -25.0, -20.0, 25.0, 20.0, 1e-9);
    let rad = g.n("instance.radial", json!({"count": 6, "radius": 50}), &[r]);
    let items = g.items(rad);
    assert_eq!(items.len(), 6);
    for it in &items {
        assert!(close(it.center().to_vec2().hypot(), 50.0, 1e-9));
    }
    assert!(close(items[0].center().y, -50.0, 1e-9));
    // A partial sweep ends on the sweep.
    let half = g.n("instance.radial", json!({"count": 3, "radius": 50, "sweep": 180}), &[r]);
    let items = g.items(half);
    assert!(close(items[2].center().y, 50.0, 1e-9), "{:?}", items[2].center());
}

#[test]
fn copy_to_points_and_mirror() {
    let mut g = G::new();
    let pts = g.n("points.grid", json!({"columns": 3, "rows": 3, "spacingX": 50, "spacingY": 50}), &[]);
    let dot = g.n("generate.ellipse", json!({"width": 10, "height": 10}), &[]);
    let two = g.n("instance.linear", json!({"count": 2, "offsetX": 100}), &[dot]);
    let c = g.n("instance.copyToPoints", json!({}), &[pts, two]);
    let items = g.items(c);
    assert_eq!(items.len(), 18);
    // The instance's centre lands on each point.
    let first_pair = bounds(&items[..2]);
    assert!(close(first_pair.center().x, -50.0, 1e-9) && close(first_pair.center().y, -50.0, 1e-9));
    assert_eq!(items[17].attrs.index, 8);
    // Aligned copies turn with their points.
    let circ = g.n("points.circle", json!({"count": 4, "radius": 100}), &[]);
    let bar = g.n("generate.rectangle", json!({"width": 40, "height": 4}), &[]);
    let al = g.n("instance.copyToPoints", json!({"align": true}), &[circ, bar]);
    let items = g.items(al);
    // At 12 o'clock the bar lies along the circle (horizontal); at 9 o'clock it stands upright.
    assert!(items[0].bounds().unwrap().width() > 30.0 && items[1].bounds().unwrap().height() > 30.0);
    let m = g.n("instance.mirror", json!({"axis": "vertical", "offset": 100}), &[bar]);
    let items = g.items(m);
    assert_eq!(items.len(), 2);
    assert!(close(items[1].center().x, 200.0, 1e-9));
    let both = g.n("instance.mirror", json!({"axis": "both", "keepOriginal": false}), &[bar]);
    assert_eq!(g.items(both).len(), 3);
    // Nothing connected → nothing, without error.
    let empty = g.n("instance.copyToPoints", json!({}), &[pts]);
    assert!(g.items(empty).is_empty());
}

// ---------- modify ----------

#[test]
fn transform_and_jitter() {
    let mut g = G::new();
    let r = g.n("generate.rectangle", json!({"width": 20, "height": 10}), &[]);
    let t = g.n("modify.transform", json!({"x": 100, "rotate": 90}), &[r]);
    assert_rect(bounds(&g.items(t)), 95.0, -10.0, 105.0, 10.0, 1e-9);
    let s = g.n("modify.transform", json!({"scaleX": 2, "pivot": "origin"}), &[r]);
    assert_rect(bounds(&g.items(s)), -20.0, -5.0, 20.0, 5.0, 1e-9);
    let row = g.n("instance.linear", json!({"count": 3, "offsetX": 50}), &[r]);
    let each = g.n("modify.transform", json!({"rotate": 90, "each": true}), &[row]);
    let items = g.items(each);
    assert!(close(items[2].center().x, 100.0, 1e-9) && close(items[2].bounds().unwrap().height(), 20.0, 1e-9));
    let skew = g.n("modify.transform", json!({"skewX": 45}), &[r]);
    assert!(close(bounds(&g.items(skew)).width(), 30.0, 1e-9));
    // Jitter: nothing to do at zero, bounded otherwise, the same every time.
    let j0 = g.n("modify.jitter", json!({}), &[row]);
    assert_eq!(g.items(j0), g.items(row));
    let j = g.n("modify.jitter", json!({"move": 5, "rotate": 30, "scale": 0.5}), &[row]);
    let a = g.items(j);
    assert_eq!(a, g.items(j));
    for (x, y) in a.iter().zip(&g.items(row)) {
        assert!(x.center().distance(y.center()) <= 5.0 * std::f64::consts::SQRT_2 + 1e-9);
    }
    assert_ne!(a, g.items(row));
}

#[test]
fn noise_displaces_within_amplitude() {
    let mut g = G::new();
    let c = g.n("generate.ellipse", json!({"width": 200, "height": 200}), &[]);
    let n0 = g.n("modify.noise", json!({"amplitude": 0, "detail": 5}), &[c]);
    let flat = path_of(&g.items(n0)[0]);
    assert!(flat.anchor_count() > 100);
    for (_, _, a) in flat.anchors() {
        assert!(close(a.p.to_vec2().hypot(), 100.0, 0.05));
    }
    let n = g.n("modify.noise", json!({"amplitude": 10, "frequency": 0.03, "detail": 5}), &[c]);
    let p = path_of(&g.items(n)[0]);
    let mut moved = 0.0f64;
    for (_, _, a) in p.anchors() {
        let r = a.p.to_vec2().hypot();
        assert!(r > 100.0 - 10.0 * std::f64::consts::SQRT_2 - 0.1 && r < 100.0 + 10.0 * std::f64::consts::SQRT_2 + 0.1);
        moved = moved.max((r - 100.0).abs());
    }
    assert!(moved > 2.0, "barely moved: {moved}");
    // Seeded: another seed, another shape; the same seed, the same shape.
    let n2 = g.n("modify.noise", json!({"amplitude": 10, "frequency": 0.03, "detail": 5, "seed": 9}), &[c]);
    assert_ne!(path_of(&g.items(n2)[0]), p);
    assert_eq!(path_of(&g.items(n)[0]), p);
    // A tiny detail on a huge shape stays within the per-path cap.
    let big = g.n("generate.ellipse", json!({"width": 100000, "height": 100000}), &[]);
    let fine = g.n("modify.noise", json!({"detail": 0.25}), &[big]);
    assert!(path_of(&g.items(fine)[0]).anchor_count() <= vectorcraft_procedural::limits::MAX_ANCHORS_PER_PATH);
    // Points move too.
    let pts = g.n("points.grid", json!({"columns": 3, "rows": 1}), &[]);
    let np = g.n("modify.noise", json!({"amplitude": 20, "frequency": 0.1}), &[pts]);
    assert_ne!(origins(&g.items(np)), origins(&g.items(pts)));
}

#[test]
fn round_smooth_offset_simplify() {
    let mut g = G::new();
    let r = g.n("generate.rectangle", json!({"width": 100, "height": 50}), &[]);
    let rc = g.n("modify.roundCorners", json!({"radius": 10}), &[r]);
    let p = path_of(&g.items(rc)[0]);
    assert_eq!(p.anchor_count(), 8);
    assert_rect(p.bounds().unwrap(), -50.0, -25.0, 50.0, 25.0, 1e-9);
    // The radius never passes half a side.
    let big = g.n("modify.roundCorners", json!({"radius": 1000}), &[r]);
    assert_rect(path_of(&g.items(big)[0]).bounds().unwrap(), -50.0, -25.0, 50.0, 25.0, 1e-9);
    let sm = g.n("modify.smooth", json!({"amount": 1}), &[r]);
    assert!(path_of(&g.items(sm)[0]).subpaths[0].anchors.iter().all(|a| a.has_in() && a.has_out()));
    let off = g.n("modify.offset", json!({"distance": 5, "join": "miter"}), &[r]);
    assert_rect(path_of(&g.items(off)[0]).bounds().unwrap(), -55.0, -30.0, 55.0, 30.0, 1e-6);
    let ins = g.n("modify.offset", json!({"distance": -5}), &[r]);
    assert_rect(path_of(&g.items(ins)[0]).bounds().unwrap(), -45.0, -20.0, 45.0, 20.0, 1e-6);
    let c = g.n("generate.ellipse", json!({"width": 100, "height": 100}), &[]);
    let dense = g.n("modify.noise", json!({"amplitude": 0, "detail": 1, "smooth": false}), &[c]);
    let simp = g.n("modify.simplify", json!({"tolerance": 0.5}), &[dense]);
    let before = path_of(&g.items(dense)[0]).anchor_count();
    let after = path_of(&g.items(simp)[0]).anchor_count();
    assert!(after * 5 < before, "{before} → {after}");
}

#[test]
fn morph_blends_shapes_and_paint() {
    let mut g = G::new();
    let a = g.n("generate.rectangle", json!({"width": 100, "height": 100}), &[]);
    let af = g.n("style.fill", json!({"color": "#000000"}), &[a]);
    let b = g.n("generate.ellipse", json!({"width": 40, "height": 40}), &[]);
    let bf = g.n("style.fill", json!({"color": "#ffffff"}), &[b]);
    let m0 = g.n("modify.morph", json!({"t": 0}), &[af, bf]);
    let m1 = g.n("modify.morph", json!({"t": 1}), &[af, bf]);
    let mh = g.n("modify.morph", json!({"t": 0.5}), &[af, bf]);
    assert_rect(bounds(&g.items(m0)), -50.0, -50.0, 50.0, 50.0, 1e-6);
    assert_rect(bounds(&g.items(m1)), -20.0, -20.0, 20.0, 20.0, 0.5);
    let h = g.items(mh);
    let w = bounds(&h).width();
    assert!(w > 60.0 && w < 80.0, "{w}");
    let [r, _, _] = solid(&h[0]).to_rgb_uncalibrated();
    assert!((r - 0.5).abs() < 0.01);
    // Lists of different lengths: the shorter repeats.
    let three = g.n("instance.linear", json!({"count": 3}), &[af]);
    let m = g.n("modify.morph", json!({}), &[three, bf]);
    assert_eq!(g.items(m).len(), 3);
    // One side missing: the other passes through.
    let lone = g.n("modify.morph", json!({}), &[0, bf]);
    assert_eq!(g.items(lone), g.items(bf));
}

#[test]
fn booleans() {
    let area = |items: &[Item]| -> f64 { items.iter().map(|i| vectorcraft_pathops::area(&path_of(i), vectorcraft_geom::FillRule::NonZero)).sum() };
    let mut g = G::new();
    let a = g.n("generate.rectangle", json!({"width": 100, "height": 100}), &[]);
    let af = g.n("style.fill", json!({"color": "#ff0000"}), &[a]);
    let b0 = g.n("generate.rectangle", json!({"width": 100, "height": 100}), &[]);
    let b = g.n("modify.transform", json!({"x": 50}), &[b0]);
    for (op, want) in [("union", 15_000.0), ("subtract", 5_000.0), ("intersect", 5_000.0), ("exclude", 10_000.0)] {
        let n = g.n("modify.boolean", json!({"operation": op}), &[af, b]);
        let items = g.items(n);
        assert!(close(area(&items), want, 1.0), "{op}: {}", area(&items));
        // The result paints like A.
        assert_eq!(solid(&items[0]), Color::rgb(1.0, 0.0, 0.0), "{op}");
    }
    let d = g.n("modify.boolean", json!({"operation": "divide"}), &[af, b]);
    assert_eq!(g.items(d).len(), 3);
    // Without B: the items of A among themselves.
    let both = g.n("modify.merge", json!({}), &[af, b]);
    let u = g.n("modify.boolean", json!({"operation": "union"}), &[both]);
    assert!(close(area(&g.items(u)), 15_000.0, 1.0));
    let s = g.n("modify.boolean", json!({"operation": "subtract"}), &[both]);
    assert!(close(area(&g.items(s)), 5_000.0, 1.0));
    let dv = g.n("modify.boolean", json!({"operation": "divide"}), &[both]);
    assert_eq!(g.items(dv).len(), 3);
    // Empty A: nothing.
    let e = g.n("modify.boolean", json!({}), &[0, b]);
    assert!(g.items(e).is_empty());
}

#[test]
fn boolean_refuses_too_much_geometry() {
    let mut g = G::new();
    let c = g.n("generate.polygon", json!({"sides": 1000}), &[]);
    let many = g.n("instance.grid", json!({"columns": 30, "rows": 1}), &[c]);
    let u = g.n("modify.boolean", json!({}), &[many]);
    g.out(u);
    let e = g.eval();
    assert!(e.items.is_empty());
    let err = e.errors().next().expect("an error");
    assert_eq!(err.node, Some(u));
    assert!(err.message.contains("too much geometry"), "{}", err.message);
}

#[test]
fn merge_select_order_bbox() {
    let mut g = G::new();
    let pts = g.n("points.grid", json!({"columns": 10, "rows": 1, "spacingX": 10}), &[]);
    let c = g.n("generate.ellipse", json!({"width": 4, "height": 4}), &[]);
    let row = g.n("instance.copyToPoints", json!({}), &[pts, c]);
    let m = g.n("modify.merge", json!({}), &[row, c, 0, row]);
    let items = g.items(m);
    assert_eq!(items.len(), 21);
    assert_eq!(items[20].attrs.index, 20);
    let nth = g.n("modify.select", json!({"n": 3, "offset": 1}), &[row]);
    let xs: Vec<f64> = g.items(nth).iter().map(|i| i.center().x.round()).collect();
    assert_eq!(xs, vec![-35.0, -5.0, 25.0]);
    let inv = g.n("modify.select", json!({"n": 3, "offset": 1, "invert": true}), &[row]);
    assert_eq!(g.items(inv).len(), 7);
    let rng = g.n("modify.select", json!({"mode": "range", "start": 2, "end": -3}), &[row]);
    assert_eq!(g.items(rng).len(), 6);
    let half = g.n("modify.select", json!({"mode": "random", "fraction": 0.5}), &[row]);
    let n = g.items(half).len();
    assert!(n > 0 && n < 10, "{n}");
    let rev = g.n("modify.order", json!({"mode": "reverse"}), &[row]);
    assert!(g.items(rev)[0].center().x > 40.0);
    let sx = g.n("modify.order", json!({"mode": "x", "descending": true}), &[row]);
    assert_eq!(g.items(sx), g.items(rev));
    let sh = g.n("modify.order", json!({"mode": "shuffle"}), &[row]);
    let mut a: Vec<i64> = g.items(sh).iter().map(|i| i.center().x.round() as i64).collect();
    assert_ne!(a, g.items(row).iter().map(|i| i.center().x.round() as i64).collect::<Vec<_>>());
    a.sort();
    assert_eq!(a, g.items(row).iter().map(|i| i.center().x.round() as i64).collect::<Vec<_>>());
    let bb = g.n("modify.boundingBox", json!({"padding": 1}), &[row]);
    let items = g.items(bb);
    assert_eq!(items.len(), 1);
    assert_rect(bounds(&items), -48.0, -3.0, 48.0, 3.0, 1e-9);
    let each = g.n("modify.boundingBox", json!({"each": true}), &[row]);
    assert_eq!(g.items(each).len(), 10);
}

// ---------- style ----------

#[test]
fn paints_and_ramps() {
    let mut g = G::new();
    let r = g.n("generate.rectangle", json!({"width": 10, "height": 10}), &[]);
    let row = g.n("instance.linear", json!({"count": 5}), &[r]);
    let f = g.n("style.fill", json!({"color": "#336699"}), &[row]);
    assert!(g.items(f).iter().all(|i| solid(i) == Color::from_hex("#336699").unwrap()));
    let none = g.n("style.fill", json!({"color": "none"}), &[row]);
    assert!(g.items(none).iter().all(|i| i.style.fill == Some(Paint::None)));
    let s = g.n("style.stroke", json!({"color": "#ff0000", "width": 3}), &[row]);
    assert!(g.items(s).iter().all(|i| i.style.stroke_width == Some(3.0) && stroke_color(i) == Color::rgb(1.0, 0.0, 0.0)));
    let ramp = g.n("style.colorRamp", json!({"from": "#000000", "to": "#ffffff"}), &[row]);
    let items = g.items(ramp);
    assert_eq!(solid(&items[0]), Color::rgb(0.0, 0.0, 0.0));
    assert_eq!(solid(&items[4]), Color::rgb(1.0, 1.0, 1.0));
    assert!((solid(&items[2]).to_rgb_uncalibrated()[0] - 0.5).abs() < 1e-6);
    let rx = g.n("style.colorRamp", json!({"from": "#000000", "to": "#ffffff", "by": "x", "target": "stroke"}), &[row]);
    assert_eq!(stroke_color(&g.items(rx)[4]), Color::rgb(1.0, 1.0, 1.0));
    let hue = g.n("style.hueShift", json!({"degrees": 120}), &[ramp]);
    let _ = g.items(hue);
    let red = g.n("style.fill", json!({"color": "#ff0000"}), &[row]);
    let hs = g.n("style.hueShift", json!({"degrees": 120}), &[red]);
    let [r1, g1, _] = solid(&g.items(hs)[4]).to_rgb_uncalibrated();
    assert!(r1 < 0.01 && g1 > 0.99, "red + 120° = green");
    let op = g.n("style.opacityRamp", json!({"from": 1, "to": 0}), &[row]);
    let items = g.items(op);
    assert_eq!((items[0].style.opacity, items[4].style.opacity), (1.0, 0.0));
    let pal = ["#ff0000", "#00ff00"];
    let rc = g.n("style.randomColor", json!({"palette": pal}), &[row]);
    let items = g.items(rc);
    let allowed: Vec<Color> = pal.iter().map(|c| Color::from_hex(c).unwrap()).collect();
    assert!(items.iter().all(|i| allowed.contains(&solid(i))));
}

#[test]
fn bypass_passes_the_input_through() {
    let mut g = G::new();
    let r = g.n("generate.rectangle", json!({}), &[]);
    let t = g.n("modify.transform", json!({"x": 500}), &[r]);
    g.g.nodes[1].bypass = true;
    assert_eq!(g.items(t), g.items(r));
    // A bypassed generator has no input: it yields nothing.
    g.g.nodes[0].bypass = true;
    assert!(g.items(t).is_empty());
}

#[test]
fn trig_is_ours() {
    // The evaluator's maths comes from libm: pin a value so a switch to the platform's is noticed.
    assert_eq!(math::sin(1.0).to_bits(), libm_sin_1());
}

fn libm_sin_1() -> u64 {
    // sin(1) correctly rounded.
    0x3FEA_ED54_8F09_0CEE
}
