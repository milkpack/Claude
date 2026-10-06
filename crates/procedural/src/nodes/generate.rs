//! `generate.*` and `source.art`: nodes that make geometry from nothing.

use std::sync::Arc;

use kurbo::{Point, Rect};
use vectorcraft_color::{Color, Paint};
use vectorcraft_doc::{Node, NodeId};
use vectorcraft_geom::{Anchor, AnchorKind, PathData, SubPath, shapes};

use crate::catalogue::{Ctx, Inputs};
use crate::item::{Item, Out, Style};
use crate::limits::MAX_ANCHORS_PER_PATH;
use crate::math;

/// New closed shapes: white fill, 1 pt black stroke (like new art in the document).
pub fn shape_style() -> Style {
    Style { fill: Some(Paint::solid(Color::WHITE)), stroke: Some(Paint::solid(Color::BLACK)), stroke_width: Some(1.0), opacity: 1.0 }
}

/// New open paths: no fill, 1 pt black stroke.
pub fn line_style() -> Style {
    Style { fill: Some(Paint::None), stroke: Some(Paint::solid(Color::BLACK)), stroke_width: Some(1.0), opacity: 1.0 }
}

fn push_shape(out: &mut Out, path: PathData) {
    out.push(Item::path(path, shape_style()));
}

pub fn rectangle(cx: &Ctx, _: &Inputs, out: &mut Out) -> Result<(), String> {
    let (w, h) = (cx.p.num("width"), cx.p.num("height"));
    let r = Rect::new(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0);
    push_shape(out, shapes::rounded_rectangle(r, cx.p.num("radius")));
    Ok(())
}

pub fn ellipse(cx: &Ctx, _: &Inputs, out: &mut Out) -> Result<(), String> {
    let (w, h) = (cx.p.num("width"), cx.p.num("height"));
    push_shape(out, shapes::ellipse(Rect::new(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0)));
    Ok(())
}

/// `n` corners at radii `radius(i)`, the first straight up.
fn ring(n: usize, radius: impl Fn(usize) -> f64) -> PathData {
    let pts: Vec<Point> = (0..n).map(|i| math::polar(radius(i), 90.0 - 360.0 * i as f64 / n as f64)).collect();
    PathData::single(SubPath::polyline(&pts, true))
}

pub fn polygon(cx: &Ctx, _: &Inputs, out: &mut Out) -> Result<(), String> {
    let r = cx.p.num("radius");
    push_shape(out, ring(cx.p.count("sides").clamp(3, 1000), |_| r));
    Ok(())
}

pub fn star(cx: &Ctx, _: &Inputs, out: &mut Out) -> Result<(), String> {
    let r = cx.p.num("radius");
    let inner = r * cx.p.num("innerRatio");
    let n = cx.p.count("points").clamp(2, 1000);
    push_shape(out, ring(n * 2, |i| if i % 2 == 0 { r } else { inner }));
    Ok(())
}

pub fn line(cx: &Ctx, _: &Inputs, out: &mut Out) -> Result<(), String> {
    let a = Point::new(cx.p.num("x1"), cx.p.num("y1"));
    let b = Point::new(cx.p.num("x2"), cx.p.num("y2"));
    out.push(Item::path(PathData::single(SubPath::polyline(&[a, b], false)), line_style()));
    Ok(())
}

/// Anchors of a smooth curve sampled from `f(θ) → (point, derivative)` at `thetas`.
fn smooth_curve(thetas: &[f64], f: impl Fn(f64) -> (Point, kurbo::Vec2)) -> Vec<Anchor> {
    let n = thetas.len();
    (0..n)
        .map(|i| {
            let t = thetas.get(i).copied().unwrap_or(0.0);
            let (p, d) = f(t);
            let back = i.checked_sub(1).and_then(|j| thetas.get(j)).map_or(0.0, |prev| (t - prev) / 3.0);
            let fwd = thetas.get(i + 1).map_or(0.0, |next| (next - t) / 3.0);
            Anchor { p, h_in: p - d * back, h_out: p + d * fwd, kind: if i == 0 || i + 1 == n { AnchorKind::Corner } else { AnchorKind::Smooth } }
        })
        .collect()
}

pub fn spiral(cx: &Ctx, _: &Inputs, out: &mut Out) -> Result<(), String> {
    let turns = cx.p.num("turns");
    let (r0, r1) = (cx.p.num("innerRadius"), cx.p.num("outerRadius"));
    let exponential = cx.p.choice("growth") == "exponential";
    let dir = if cx.p.bool("clockwise") { -1.0 } else { 1.0 };
    let total = std::f64::consts::TAU * turns;
    // Eight anchors per turn keeps the cubic fit close.
    let steps = ((turns * 8.0).ceil() as usize).clamp(2, MAX_ANCHORS_PER_PATH - 1);
    let thetas: Vec<f64> = (0..=steps).map(|i| total * i as f64 / steps as f64).collect();
    // r(θ) and r'(θ).
    let radius = |t: f64| -> (f64, f64) {
        if exponential {
            let a = r0.max(1e-3);
            let b = r1.max(a);
            let k = if total > 0.0 { (libm::log(b) - libm::log(a)) / total } else { 0.0 };
            let r = a * libm::exp(k * t);
            (r, k * r)
        } else {
            let k = if total > 0.0 { (r1 - r0) / total } else { 0.0 };
            (r0 + k * t, k)
        }
    };
    let anchors = smooth_curve(&thetas, |t| {
        let (r, dr) = radius(t);
        let (s, c) = (math::sin(t), math::cos(t));
        // Page coordinates: counter-clockwise is −y.
        let p = Point::new(r * c, -dir * r * s);
        let d = kurbo::Vec2::new(dr * c - r * s, -dir * (dr * s + r * c));
        (p, d)
    });
    out.push(Item::path(PathData::single(SubPath::new(anchors, false)), line_style()));
    Ok(())
}

pub fn arc(cx: &Ctx, _: &Inputs, out: &mut Out) -> Result<(), String> {
    let r = cx.p.num("radius");
    let start = math::rad(cx.p.num("startAngle"));
    let sweep = math::rad(cx.p.num("sweep"));
    let closure = cx.p.choice("closure");
    if sweep.abs() < 1e-9 {
        return Ok(());
    }
    // Up to 90° per cubic.
    let pieces = ((sweep.abs() / std::f64::consts::FRAC_PI_2).ceil() as usize).clamp(1, 4);
    let thetas: Vec<f64> = (0..=pieces).map(|i| start + sweep * i as f64 / pieces as f64).collect();
    // Exact circle handles: 4/3·tan(φ/4) of the radius per piece of φ.
    let k = 4.0 / 3.0 * math::tan(sweep / pieces as f64 / 4.0) / (sweep / pieces as f64);
    let mut anchors = smooth_curve(&thetas, |t| {
        let (s, c) = (math::sin(t), math::cos(t));
        (Point::new(r * c, -r * s), kurbo::Vec2::new(-r * s, -r * c) * (3.0 * k))
    });
    let closed = closure != "open";
    if closure == "pie" {
        anchors.push(Anchor::corner(Point::ZERO));
    }
    let style = if closed { shape_style() } else { line_style() };
    out.push(Item::path(PathData::single(SubPath::new(anchors, closed)), style));
    Ok(())
}

pub fn art(cx: &Ctx, _: &Inputs, out: &mut Out) -> Result<(), String> {
    let art = &cx.node.art;
    if art.is_empty() {
        return Ok(());
    }
    if cx.p.bool("separate") || art.len() == 1 {
        for (i, n) in art.iter().enumerate() {
            let mut it = Item::art(n.clone());
            it.attrs.index = i as u32;
            if !out.push(it) {
                break;
            }
        }
    } else {
        // Ids are placeholders: generated art always gets fresh ids from the document.
        out.push(Item::art(Arc::new(Node::group(NodeId(0), art.clone()))));
    }
    Ok(())
}
