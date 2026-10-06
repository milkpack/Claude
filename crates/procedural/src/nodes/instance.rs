//! `instance.*`: copies of the input, placed in patterns.

use kurbo::{Affine, Point, Vec2};

use crate::catalogue::{Ctx, Inputs};
use crate::item::{Geom, Item, Out, list_bounds};
use crate::math;

/// The pivot of a list: the centre of its bounds, or the origin.
fn pivot(cx: &Ctx, items: &[Item]) -> Point {
    if cx.p.choice("pivot") == "origin" {
        return Point::ZERO;
    }
    list_bounds(items).map_or(Point::ZERO, |b| b.center())
}

/// Push a copy of every item placed by `a`, numbered as copy `copy` (its `index` attribute and
/// its random number). False once the output is full.
fn push_copy(cx: &Ctx, out: &mut Out, items: &[Item], a: Affine, copy: usize) -> bool {
    for it in items {
        let mut c = it.transformed(a);
        c.attrs.index = copy as u32;
        c.attrs.random = cx.rand(copy, 0x1C);
        if !out.push(c) {
            return false;
        }
    }
    true
}

pub fn linear(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let items = inputs.get(0);
    if items.is_empty() {
        return Ok(());
    }
    let n = cx.p.count("count").max(1);
    let off = Vec2::new(cx.p.num("offsetX"), cx.p.num("offsetY"));
    let (rot, scale) = (cx.p.num("rotate"), cx.p.num("scale"));
    let c = pivot(cx, items);
    for i in 0..n {
        let k = i as f64;
        let s = math::pow(scale, k);
        if !s.is_finite() {
            break;
        }
        let a = Affine::translate(off * k) * math::about(c, math::rotate_deg(rot * k) * Affine::scale(s));
        if !push_copy(cx, out, items, a, i) {
            break;
        }
    }
    Ok(())
}

pub fn grid(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let items = inputs.get(0);
    if items.is_empty() {
        return Ok(());
    }
    let (cols, rows) = (cx.p.count("columns").max(1), cx.p.count("rows").max(1));
    let (sx, sy) = (cx.p.num("spacingX"), cx.p.num("spacingY"));
    let stagger = cx.p.bool("stagger");
    let (w, h) = ((cols - 1) as f64 * sx, (rows - 1) as f64 * sy);
    let mut i = 0;
    for r in 0..rows {
        for c in 0..cols {
            let shift = if stagger && r % 2 == 1 { sx / 2.0 } else { 0.0 };
            let a = Affine::translate((c as f64 * sx - w / 2.0 + shift, r as f64 * sy - h / 2.0));
            if !push_copy(cx, out, items, a, i) {
                return Ok(());
            }
            i += 1;
        }
    }
    Ok(())
}

pub fn radial(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let items = inputs.get(0);
    if items.is_empty() {
        return Ok(());
    }
    let n = cx.p.count("count").max(1);
    let r = cx.p.num("radius");
    let (start, sweep) = (cx.p.num("startAngle"), cx.p.num("sweep"));
    let rotate = cx.p.bool("rotate");
    // A full circle doesn't repeat its first copy at the end; a partial one ends on its sweep.
    let full = (sweep.abs() - 360.0).abs() < 1e-9;
    let div = if full || n == 1 { n as f64 } else { (n - 1) as f64 };
    let up = Affine::translate((0.0, -r));
    for i in 0..n {
        let ang = start + sweep * i as f64 / div;
        let a = if rotate {
            math::rotate_deg(ang) * up
        } else {
            let p = math::rotate_deg(ang) * Point::new(0.0, -r);
            Affine::translate(p.to_vec2())
        };
        if !push_copy(cx, out, items, a, i) {
            break;
        }
    }
    Ok(())
}

pub fn copy_to_points(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let (targets, inst) = (inputs.get(0), inputs.get(1));
    if targets.is_empty() || inst.is_empty() {
        return Ok(());
    }
    let align = cx.p.bool("align");
    let scale = cx.p.num("scale");
    let c = pivot(cx, inst);
    let base = Affine::scale(scale) * Affine::translate(-c.to_vec2());
    for t in targets {
        let place = match t.geom {
            Geom::Point if align => t.transform,
            Geom::Point => Affine::translate(t.origin().to_vec2()),
            _ => Affine::translate(t.center().to_vec2()),
        };
        // Copies take the point's number, so later nodes can ramp or select by it.
        for it in inst {
            let mut c = it.transformed(place * base);
            c.attrs = t.attrs;
            if !out.push(c) {
                return Ok(());
            }
        }
    }
    Ok(())
}

pub fn mirror(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let items = inputs.get(0);
    let o = cx.p.num("offset");
    let fx = Affine::new([-1.0, 0.0, 0.0, 1.0, 2.0 * o, 0.0]);
    let fy = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 2.0 * o]);
    let mut copies = vec![];
    if cx.p.bool("keepOriginal") {
        copies.push(Affine::IDENTITY);
    }
    match cx.p.choice("axis") {
        "horizontal" => copies.push(fy),
        "both" => copies.extend([fx, fy, fx * fy]),
        _ => copies.push(fx),
    }
    for (i, a) in copies.into_iter().enumerate() {
        if !push_copy(cx, out, items, a, i) {
            break;
        }
    }
    Ok(())
}
