//! `points.*`: point clouds (items with [`Geom::Point`]) for copy-to-points and friends.

use kurbo::{Affine, BezPath, Point, Rect, Shape};

use crate::catalogue::{Ctx, Inputs};
use crate::geo::{self, Measured};
use crate::item::{Item, Out};
use crate::limits::{MAX_ANCHORS_PER_PATH, MAX_ITEMS, WORK_BUDGET};
use crate::math;

/// A point item turned by `deg` (its rotation tells copy-to-points how to align).
fn point(p: Point, deg: f64, index: usize, cx: &Ctx) -> Item {
    let mut it = Item::point(p);
    if deg != 0.0 {
        it.transform = Affine::translate(p.to_vec2()) * math::rotate_deg(deg);
    }
    it.attrs.index = index as u32;
    it.attrs.random = cx.rand(index, 0xA77);
    it
}

/// The input's shapes as one non-zero region to test points against: each closed subpath with
/// its bounds (a point only walks the subpaths whose box holds it).
struct Region {
    parts: Vec<(Rect, BezPath, usize)>,
    bounds: Rect,
}

fn region(items: &[Item]) -> Option<Region> {
    let mut parts = vec![];
    for it in items {
        for (pd, _, _) in it.world_paths() {
            for sp in &pd.subpaths {
                if sp.anchors.len() < 2 {
                    continue;
                }
                let mut closed = sp.clone();
                closed.closed = true;
                let mut bp = BezPath::new();
                closed.to_bezpath_into(&mut bp);
                let b = bp.bounding_box();
                if b.is_finite() && b.area() > 0.0 {
                    parts.push((b, bp, closed.segment_count()));
                }
            }
        }
    }
    let bounds = parts.iter().map(|p| p.0).reduce(|a, b| a.union(b))?;
    (bounds.width() > 0.0 && bounds.height() > 0.0).then_some(Region { parts, bounds })
}

impl Region {
    /// Inside under the non-zero rule; `work` counts the segments walked.
    fn contains(&self, p: Point, work: &mut usize) -> bool {
        let mut w = 0;
        for (b, bp, segs) in &self.parts {
            *work += 1;
            if p.x >= b.x0 && p.x <= b.x1 && p.y >= b.y0 && p.y <= b.y1 {
                *work += segs;
                w += bp.winding(p);
            }
        }
        w != 0
    }
}

/// The area to fill: the input's shapes, or a `width` × `height` rectangle round the origin.
fn area(cx: &Ctx, inputs: &Inputs) -> (Rect, Option<Region>) {
    if inputs.connected(0) {
        let r = region(inputs.get(0));
        let b = r.as_ref().map_or(Rect::ZERO, |r| r.bounds);
        return (b, r);
    }
    let (w, h) = (cx.p.num("width"), cx.p.num("height"));
    (Rect::new(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0), None)
}

pub fn scatter(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let count = cx.p.count("count").min(MAX_ITEMS);
    let (b, region) = area(cx, inputs);
    if inputs.connected(0) && region.is_none() {
        return Ok(());
    }
    let mut rng = cx.rng(1);
    // Rejection sampling inside a shape: bounded attempts, and bounded work.
    let max_attempts = count.saturating_mul(50);
    let (mut placed, mut attempts, mut work) = (0, 0, 0);
    while placed < count && attempts < max_attempts && work < WORK_BUDGET {
        attempts += 1;
        work += 1;
        let p = Point::new(rng.range(b.x0, b.x1), rng.range(b.y0, b.y1));
        if region.as_ref().is_some_and(|r| !r.contains(p, &mut work)) {
            continue;
        }
        if !out.push(point(p, 0.0, placed, cx)) {
            break;
        }
        placed += 1;
    }
    Ok(())
}

pub fn poisson(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let r = cx.p.num("distance");
    let max = cx.p.count("maxCount").min(MAX_ITEMS);
    let (b, region) = area(cx, inputs);
    if (inputs.connected(0) && region.is_none()) || b.width() <= 0.0 || b.height() <= 0.0 || max == 0 {
        return Ok(());
    }
    // Background grid of cells r/√2 wide: at most one point per cell. Capped so a tiny distance
    // over a huge area can't allocate without bound (the distance grows to fit instead).
    let mut cell = r / std::f64::consts::SQRT_2;
    const MAX_CELLS: f64 = 1_000_000.0;
    let cells = (b.width() / cell).ceil() * (b.height() / cell).ceil();
    let mut r = r;
    if !cells.is_finite() || cells > MAX_CELLS {
        let k = math::sqrt(cells / MAX_CELLS);
        cell *= k;
        r *= k;
    }
    let cols = ((b.width() / cell).ceil() as usize).max(1);
    let rows = ((b.height() / cell).ceil() as usize).max(1);
    const EMPTY: u32 = u32::MAX;
    let mut grid: Vec<u32> = vec![EMPTY; cols.saturating_mul(rows)];
    let cell_of = |p: Point| -> Option<(usize, usize)> {
        let c = ((p.x - b.x0) / cell).floor();
        let rr = ((p.y - b.y0) / cell).floor();
        (c >= 0.0 && rr >= 0.0 && (c as usize) < cols && (rr as usize) < rows).then_some((c as usize, rr as usize))
    };
    let mut work = 0usize;
    let mut rng = cx.rng(2);
    let mut pts: Vec<Point> = vec![];
    let mut active: Vec<usize> = vec![];
    let inside = |p: Point, work: &mut usize| -> bool {
        if !b.contains(p) {
            return false;
        }
        match &region {
            Some(reg) => reg.contains(p, work),
            None => true,
        }
    };
    // Seed point: the first random point inside.
    for _ in 0..1000 {
        let p = Point::new(rng.range(b.x0, b.x1), rng.range(b.y0, b.y1));
        if inside(p, &mut work) {
            if let Some((c, rr)) = cell_of(p)
                && let Some(slot) = grid.get_mut(rr * cols + c)
            {
                *slot = 0;
            }
            pts.push(p);
            active.push(0);
            break;
        }
    }
    const K: usize = 30;
    while !active.is_empty() && pts.len() < max && work < crate::limits::WORK_BUDGET {
        let ai = rng.below(active.len());
        let Some(&pi) = active.get(ai) else { break };
        let Some(&base) = pts.get(pi) else { break };
        let mut found = false;
        for _ in 0..K {
            work += 1;
            let ang = rng.range(0.0, 360.0);
            let dist = r * (1.0 + rng.next_f64());
            let q = base + math::polar(dist, ang).to_vec2();
            if !inside(q, &mut work) {
                continue;
            }
            let Some((c, rr)) = cell_of(q) else { continue };
            let mut ok = true;
            'n: for y in rr.saturating_sub(2)..(rr + 3).min(rows) {
                for x in c.saturating_sub(2)..(c + 3).min(cols) {
                    if let Some(&j) = grid.get(y * cols + x)
                        && j != EMPTY
                        && let Some(o) = pts.get(j as usize)
                        && math::hypot(*o - q) < r
                    {
                        ok = false;
                        break 'n;
                    }
                }
            }
            if ok {
                if let Some(slot) = grid.get_mut(rr * cols + c) {
                    *slot = pts.len() as u32;
                }
                active.push(pts.len());
                pts.push(q);
                found = true;
                break;
            }
        }
        if !found {
            active.swap_remove(ai);
        }
    }
    for (i, p) in pts.into_iter().enumerate() {
        if !out.push(point(p, 0.0, i, cx)) {
            break;
        }
    }
    Ok(())
}

pub fn grid(cx: &Ctx, _: &Inputs, out: &mut Out) -> Result<(), String> {
    let (cols, rows) = (cx.p.count("columns").max(1), cx.p.count("rows").max(1));
    let (sx, sy) = (cx.p.num("spacingX"), cx.p.num("spacingY"));
    let stagger = cx.p.bool("stagger");
    let (w, h) = ((cols - 1) as f64 * sx, (rows - 1) as f64 * sy);
    let mut i = 0;
    'g: for r in 0..rows {
        for c in 0..cols {
            let shift = if stagger && r % 2 == 1 { sx / 2.0 } else { 0.0 };
            let p = Point::new(c as f64 * sx - w / 2.0 + shift, r as f64 * sy - h / 2.0);
            if !out.push(point(p, 0.0, i, cx)) {
                break 'g;
            }
            i += 1;
        }
    }
    Ok(())
}

pub fn circle(cx: &Ctx, _: &Inputs, out: &mut Out) -> Result<(), String> {
    let n = cx.p.count("count").max(1);
    let r = cx.p.num("radius");
    let start = cx.p.num("startAngle");
    for i in 0..n {
        let a = start + 360.0 * i as f64 / n as f64;
        // Turned along the circle: a copy aligned to it faces its direction of travel.
        if !out.push(point(math::polar(r, a), a + 90.0, i, cx)) {
            break;
        }
    }
    Ok(())
}

pub fn along_path(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let count = cx.p.count("count").max(1);
    let spacing = cx.p.num("spacing");
    let mut idx = 0;
    for it in inputs.get(0) {
        for (pd, _, _) in it.world_paths() {
            for sp in &pd.subpaths {
                let m = Measured::new(geo::flatten_fine(sp, MAX_ANCHORS_PER_PATH), sp.closed);
                let len = m.length();
                let n = if spacing > 0.0 {
                    let k = (len / spacing).floor();
                    let k = if k.is_finite() { (k as usize).min(MAX_ITEMS) } else { 0 };
                    if sp.closed { k.max(1) } else { k + 1 }
                } else {
                    count
                };
                for j in 0..n {
                    let s = if spacing > 0.0 {
                        j as f64 * spacing
                    } else if sp.closed {
                        len * j as f64 / n as f64
                    } else if n > 1 {
                        len * j as f64 / (n - 1) as f64
                    } else {
                        len / 2.0
                    };
                    let Some((p, d)) = m.at(s) else { continue };
                    if !out.push(point(p, math::angle_deg(d), idx, cx)) {
                        return Ok(());
                    }
                    idx += 1;
                }
            }
        }
    }
    Ok(())
}

pub fn anchors(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let mut idx = 0;
    for it in inputs.get(0) {
        for (pd, _, _) in it.world_paths() {
            for sp in &pd.subpaths {
                let n = sp.anchors.len();
                for (i, a) in sp.anchors.iter().enumerate() {
                    // Turned along the path: the direction from the previous anchor to the next.
                    let prev = if i > 0 {
                        sp.anchors.get(i - 1)
                    } else if sp.closed {
                        sp.anchors.last()
                    } else {
                        None
                    };
                    let next = if i + 1 < n {
                        sp.anchors.get(i + 1)
                    } else if sp.closed {
                        sp.anchors.first()
                    } else {
                        None
                    };
                    let d = match (prev, next) {
                        (Some(p), Some(q)) => q.p - p.p,
                        (None, Some(q)) => q.p - a.p,
                        (Some(p), None) => a.p - p.p,
                        (None, None) => kurbo::Vec2::new(1.0, 0.0),
                    };
                    if !out.push(point(a.p, math::angle_deg(d), idx, cx)) {
                        return Ok(());
                    }
                    idx += 1;
                }
            }
        }
    }
    Ok(())
}
