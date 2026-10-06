//! `modify.*` (and `output`): nodes that change, combine, filter or reorder items.

use kurbo::{Affine, Point, Rect};
use vectorcraft_color::Paint;
use vectorcraft_geom::{Anchor, AnchorKind, FillRule, PathData, SubPath, shapes};
use vectorcraft_pathops::{self as pathops, BoolOp, Join, PathfinderOp};

use crate::catalogue::{Ctx, Inputs};
use crate::geo::{self, Measured};
use crate::item::{Geom, Item, Out, Style, list_bounds};
use crate::limits::{EXPENSIVE_ANCHORS, MAX_ANCHORS_PER_PATH, MAX_BOOLEAN_ITEMS};
use crate::math;
use crate::nodes::style::lerp_color;

pub fn passthrough(_: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    for it in inputs.get(0) {
        if !out.push(it.clone()) {
            break;
        }
    }
    Ok(())
}

pub fn merge(_: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let mut i = 0u32;
    for list in &inputs.lists {
        for it in list.iter() {
            let mut c = it.clone();
            c.attrs.index = i;
            i = i.saturating_add(1);
            if !out.push(c) {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// The linear part of the Transform node (scale, then skew, then rotate).
fn linear_part(cx: &Ctx) -> Affine {
    math::rotate_deg(cx.p.num("rotate"))
        * math::skew_deg(cx.p.num("skewX"), cx.p.num("skewY"))
        * Affine::scale_non_uniform(cx.p.num("scaleX"), cx.p.num("scaleY"))
}

pub fn transform(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let items = inputs.get(0);
    let lin = linear_part(cx);
    let mv = Affine::translate((cx.p.num("x"), cx.p.num("y")));
    let origin = cx.p.choice("pivot") == "origin";
    let whole = if origin { Point::ZERO } else { list_bounds(items).map_or(Point::ZERO, |b| b.center()) };
    let each = cx.p.bool("each");
    for it in items {
        let c = if each && !origin { it.center() } else { whole };
        if !out.push(it.transformed(mv * math::about(c, lin))) {
            break;
        }
    }
    Ok(())
}

pub fn jitter(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let (mv, rot, sc) = (cx.p.num("move"), cx.p.num("rotate"), cx.p.num("scale"));
    for (i, it) in inputs.get(0).iter().enumerate() {
        // Independent numbers per item and per channel: adding items doesn't reshuffle the rest.
        let r = |salt: u64| cx.rand(i, salt) * 2.0 - 1.0;
        let a = Affine::translate((r(1) * mv, r(2) * mv)) * math::about(it.center(), math::rotate_deg(r(3) * rot) * Affine::scale(1.0 + r(4) * sc));
        if !out.push(it.transformed(a)) {
            break;
        }
    }
    Ok(())
}

pub fn noise(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let amp = cx.p.num("amplitude");
    let freq = cx.p.num("frequency");
    let oct = cx.p.int("octaves").clamp(1, 8) as u32;
    let step = cx.p.num("detail");
    let smooth = cx.p.bool("smooth");
    let (sx, sy) = (cx.seed, cx.seed ^ 0x5DEE_CE66_D1CE_4E5B);
    let displace = |p: Point| -> Point {
        let (x, y) = (p.x * freq, p.y * freq);
        Point::new(p.x + amp * math::fbm(sx, x, y, oct), p.y + amp * math::fbm(sy, x + 31.7, y + 17.3, oct))
    };
    for it in inputs.get(0) {
        for pi in it.to_path_items() {
            let c = match &pi.geom {
                Geom::Point => {
                    let p = displace(pi.origin());
                    Item { transform: Affine::translate(p.to_vec2()) * pi.transform.translation_free(), ..pi }
                }
                Geom::Path { path, rule } => {
                    let budget = out.anchor_budget().min(MAX_ANCHORS_PER_PATH);
                    let mut subs = vec![];
                    let mut used = 0;
                    for sp in &path.subpaths {
                        let left = budget.saturating_sub(used);
                        if left < 2 {
                            break;
                        }
                        let pts: Vec<Point> = geo::flatten(sp, step, left).into_iter().map(displace).collect();
                        used += pts.len();
                        subs.push(geo::through(&pts, sp.closed, smooth));
                    }
                    Item { geom: Geom::Path { path: PathData::new(subs), rule: *rule }, ..pi }
                }
                Geom::Art { .. } => pi,
            };
            if !out.push(c) {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Translation dropped (the linear part only).
trait TranslationFree {
    fn translation_free(&self) -> Affine;
}

impl TranslationFree for Affine {
    fn translation_free(&self) -> Affine {
        let [a, b, c, d, _, _] = self.as_coeffs();
        Affine::new([a, b, c, d, 0.0, 0.0])
    }
}

/// Apply `f` to the path of every item (art broken into paths; points pass through).
fn map_paths(inputs: &Inputs, out: &mut Out, f: impl Fn(&PathData) -> PathData) -> Result<(), String> {
    for it in inputs.get(0) {
        for mut pi in it.to_path_items() {
            if let Geom::Path { path, .. } = &mut pi.geom {
                *path = guarded(|| f(path))?;
            }
            if !out.push(pi) {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Anchors in the input (art counted by its paths): expensive nodes refuse more than
/// [`EXPENSIVE_ANCHORS`].
fn check_expensive(items: &[Item], what: &str) -> Result<(), String> {
    let n: usize = items.iter().map(Item::anchors).sum();
    if n > EXPENSIVE_ANCHORS {
        return Err(format!("too much geometry for {what}: {n} anchors (limit {EXPENSIVE_ANCHORS})"));
    }
    Ok(())
}

pub fn round_corners(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let r = cx.p.num("radius");
    map_paths(inputs, out, |p| round_path(p, r))
}

/// Round every corner between two straight segments with a quarter-circle-like curve of
/// `radius` (shortened to half the shorter neighbouring segment).
pub fn round_path(path: &PathData, radius: f64) -> PathData {
    if radius <= 0.0 {
        return path.clone();
    }
    let k = vectorcraft_geom::shapes::KAPPA;
    let mut out = PathData::default();
    for sp in &path.subpaths {
        let n = sp.anchors.len();
        if !(3..=MAX_ANCHORS_PER_PATH / 2).contains(&n) {
            out.subpaths.push(sp.clone());
            continue;
        }
        let mut anchors = Vec::with_capacity(n * 2);
        for i in 0..n {
            let Some(a) = sp.anchors.get(i) else { continue };
            let end = !sp.closed && (i == 0 || i + 1 == n);
            let prev = sp.anchors.get((i + n - 1) % n);
            let next = sp.anchors.get((i + 1) % n);
            let (Some(prev), Some(next)) = (prev, next) else { continue };
            if end || a.has_in() || a.has_out() || prev.has_out() || next.has_in() {
                anchors.push(*a);
                continue;
            }
            let (u, v) = (prev.p - a.p, next.p - a.p);
            let (lu, lv) = (math::hypot(u), math::hypot(v));
            let d = radius.min(lu / 2.0).min(lv / 2.0);
            if d <= 1e-9 || geo::unit(u).dot(geo::unit(v)) < -0.9999 {
                anchors.push(*a);
                continue;
            }
            let p1 = a.p + geo::unit(u) * d;
            let p2 = a.p + geo::unit(v) * d;
            anchors.push(Anchor { p: p1, h_in: p1, h_out: p1 + (a.p - p1) * k, kind: AnchorKind::Corner });
            anchors.push(Anchor { p: p2, h_in: p2 + (a.p - p2) * k, h_out: p2, kind: AnchorKind::Corner });
        }
        out.subpaths.push(SubPath::new(anchors, sp.closed));
    }
    out
}

pub fn smooth(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let amount = cx.p.num("amount");
    map_paths(inputs, out, |p| pathops::smooth(p, amount))
}

pub fn offset(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    check_expensive(inputs.get(0), "Offset Path")?;
    let d = cx.p.num("distance");
    let join = match cx.p.choice("join") {
        "round" => Join::Round,
        "bevel" => Join::Bevel,
        _ => Join::Miter,
    };
    let ml = cx.p.num("miterLimit");
    map_paths(inputs, out, |p| if d == 0.0 { p.clone() } else { pathops::offset_path(p, d, join, ml) })
}

pub fn simplify(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    check_expensive(inputs.get(0), "Simplify")?;
    let tol = cx.p.num("tolerance");
    map_paths(inputs, out, |p| pathops::simplify(p, tol))
}

// ---------- morph ----------

/// An item as polylines for morphing (a point is a one-point line).
fn polylines(it: &Item) -> Vec<(Vec<Point>, bool)> {
    match &it.geom {
        Geom::Point => vec![(vec![it.origin()], false)],
        _ => it
            .world_paths()
            .into_iter()
            .flat_map(|(p, _, _)| p.subpaths.into_iter().filter(|s| !s.anchors.is_empty()).map(|s| (geo::flatten_fine(&s, 4096), s.closed)))
            .collect(),
    }
}

fn lerp_paint(a: &Option<Paint>, b: &Option<Paint>, t: f64) -> Option<Paint> {
    match (a, b) {
        (Some(Paint::Solid { color: ca, .. }), Some(Paint::Solid { color: cb, .. })) => Some(Paint::solid(lerp_color(*ca, *cb, t))),
        _ if t < 0.5 => a.clone(),
        _ => b.clone(),
    }
}

fn lerp_style(a: &Style, b: &Style, t: f64) -> Style {
    let w = match (a.stroke_width, b.stroke_width) {
        (Some(x), Some(y)) => Some(x + (y - x) * t),
        (x, y) => {
            if t < 0.5 {
                x
            } else {
                y
            }
        }
    };
    Style {
        fill: lerp_paint(&a.fill, &b.fill, t),
        stroke: lerp_paint(&a.stroke, &b.stroke, t),
        stroke_width: w,
        opacity: a.opacity + (b.opacity - a.opacity) * t,
    }
}

pub fn morph(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let (a, b) = (inputs.get(0), inputs.get(1));
    let t = cx.p.num("t");
    if a.is_empty() || b.is_empty() {
        for it in if a.is_empty() { b } else { a } {
            if !out.push(it.clone()) {
                break;
            }
        }
        return Ok(());
    }
    let n = a.len().max(b.len());
    for i in 0..n {
        let (Some(ia), Some(ib)) = (a.get(i % a.len()), b.get(i % b.len())) else { break };
        let (pa, pb) = (polylines(ia), polylines(ib));
        if pa.is_empty() || pb.is_empty() {
            continue;
        }
        let mut subs = vec![];
        let mut budget = out.anchor_budget().min(MAX_ANCHORS_PER_PATH);
        for j in 0..pa.len().max(pb.len()) {
            let (Some((sa, ca)), Some((sb, cb))) = (pa.get(j % pa.len()), pb.get(j % pb.len())) else { break };
            let closed = *ca && *cb;
            let samples = (sa.len().max(sb.len())).clamp(8, 256).min(budget);
            if samples < 2 {
                break;
            }
            budget -= samples;
            let ra = Measured::new(sa.clone(), closed).resample(samples, closed);
            let mut rb = Measured::new(sb.clone(), closed).resample(samples, closed);
            if closed && ra.len() == rb.len() && !ra.is_empty() {
                // Same winding, and B starting at the point nearest A's start: no twisting.
                if (geo::polygon_area(&ra) > 0.0) != (geo::polygon_area(&rb) > 0.0) {
                    rb.reverse();
                }
                if let Some(a0) = ra.first() {
                    let k =
                        rb.iter().enumerate().min_by(|x, y| x.1.distance_squared(*a0).total_cmp(&y.1.distance_squared(*a0))).map_or(0, |(k, _)| k);
                    rb.rotate_left(k);
                }
            }
            let pts: Vec<Point> = ra.iter().zip(&rb).map(|(p, q)| p.lerp(*q, t)).collect();
            subs.push(geo::through(&pts, closed, false));
        }
        let style = lerp_style(&ia.style, &ib.style, t);
        let it =
            Item { geom: Geom::Path { path: PathData::new(subs), rule: FillRule::NonZero }, transform: Affine::IDENTITY, style, attrs: ia.attrs };
        if !out.push(it) {
            break;
        }
    }
    Ok(())
}

// ---------- boolean ----------

pub fn boolean(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let (a, b) = (inputs.get(0), inputs.get(1));
    check_expensive(a, "a boolean")?;
    check_expensive(b, "a boolean")?;
    let pa: Vec<(PathData, FillRule, Style)> = a.iter().flat_map(Item::world_paths).collect();
    let pb: Vec<(PathData, FillRule, Style)> = b.iter().flat_map(Item::world_paths).collect();
    if pa.len() + pb.len() > MAX_BOOLEAN_ITEMS {
        return Err(format!("too many shapes for a boolean: {} (limit {MAX_BOOLEAN_ITEMS})", pa.len() + pb.len()));
    }
    let Some(style) = pa.first().map(|p| p.2.clone()) else { return Ok(()) };
    let op = cx.p.choice("operation");
    let unite = |v: &[(PathData, FillRule, Style)]| -> PathData {
        match v {
            [(p, rule, _)] if *rule == FillRule::NonZero => p.clone(),
            _ => pathops::unite_all(&v.iter().map(|(p, r, _)| (p, *r)).collect::<Vec<_>>()),
        }
    };
    let results: Vec<(PathData, Style)> = guarded(|| {
        if !b.is_empty() {
            let (ua, ub) = (unite(&pa), unite(&pb));
            let bop = match op {
                "subtract" => BoolOp::Difference,
                "intersect" => BoolOp::Intersect,
                "exclude" => BoolOp::Xor,
                "divide" => {
                    let shapes = [pathops::Shape::new(ua, FillRule::NonZero, 0), pathops::Shape::new(ub, FillRule::NonZero, 1)];
                    let sb = pb.first().map_or_else(|| style.clone(), |p| p.2.clone());
                    return pathops::pathfinder(PathfinderOp::Divide, &shapes)
                        .into_iter()
                        .map(|s| (s.path, if s.key == 0 { style.clone() } else { sb.clone() }))
                        .collect();
                }
                _ => BoolOp::Union,
            };
            return vec![(pathops::boolean(&ua, FillRule::NonZero, &ub, FillRule::NonZero, bop), style.clone())];
        }
        let pop = match op {
            "subtract" => PathfinderOp::MinusFront,
            "intersect" => PathfinderOp::Intersect,
            "exclude" => PathfinderOp::Exclude,
            "divide" => PathfinderOp::Divide,
            _ => PathfinderOp::Unite,
        };
        let shapes: Vec<pathops::Shape> = pa.iter().enumerate().map(|(i, (p, r, _))| pathops::Shape::new(p.clone(), *r, i as u64)).collect();
        pathops::pathfinder(pop, &shapes)
            .into_iter()
            .map(|s| (s.path, usize::try_from(s.key).ok().and_then(|k| pa.get(k)).map_or_else(|| style.clone(), |p| p.2.clone())))
            .collect()
    })?;
    for (path, style) in results {
        if !path.is_empty()
            && !out.push(Item { geom: Geom::Path { path, rule: FillRule::NonZero }, transform: Affine::IDENTITY, style, attrs: Default::default() })
        {
            break;
        }
    }
    Ok(())
}

/// Run a path operation, turning a panic deep in the geometry kernel (a sanity check that
/// near-degenerate input can trip in debug builds) into an error for the node.
fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|_| "the shapes are too degenerate to combine".to_string())
}

// ---------- lists ----------

pub fn select(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let items = inputs.get(0);
    let n = items.len() as i64;
    let invert = cx.p.bool("invert");
    let mode = cx.p.choice("mode");
    let (every, offset) = (cx.p.int("n").max(1), cx.p.int("offset").max(0));
    let resolve = |i: i64| if i < 0 { n + i } else { i };
    let (start, end) = (resolve(cx.p.int("start")), resolve(cx.p.int("end")));
    let fraction = cx.p.num("fraction");
    for (i, it) in items.iter().enumerate() {
        let k = i as i64;
        let keep = match mode {
            "range" => k >= start && k <= end,
            "random" => cx.rand(i, 0x5E1) < fraction,
            _ => k >= offset && (k - offset) % every == 0,
        };
        if keep != invert && !out.push(it.clone()) {
            break;
        }
    }
    Ok(())
}

pub fn order(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let mut items: Vec<Item> = inputs.get(0).to_vec();
    let desc = cx.p.bool("descending");
    match cx.p.choice("mode") {
        "shuffle" => {
            let mut rng = cx.rng(0x50F);
            for i in (1..items.len()).rev() {
                let j = rng.below(i + 1);
                items.swap(i, j);
            }
        }
        "reverse" => items.reverse(),
        mode => {
            let c = list_bounds(&items).map_or(Point::ZERO, |b| b.center());
            let key = |it: &Item| -> f64 {
                match mode {
                    "x" => it.center().x,
                    "y" => it.center().y,
                    "size" => it.bounds().map_or(0.0, |b| b.area()),
                    _ => math::hypot(it.center() - c),
                }
            };
            let mut keyed: Vec<(f64, Item)> = items.into_iter().map(|it| (key(&it), it)).collect();
            // Stable, and total on NaN: equal keys keep their order.
            keyed.sort_by(|a, b| if desc { b.0.total_cmp(&a.0) } else { a.0.total_cmp(&b.0) });
            items = keyed.into_iter().map(|(_, it)| it).collect();
        }
    }
    for it in items {
        if !out.push(it) {
            break;
        }
    }
    Ok(())
}

pub fn bounding_box(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let items = inputs.get(0);
    let pad = cx.p.num("padding");
    let boxed = |b: Rect, style: &Style| -> Option<Item> {
        let r = b.inflate(pad, pad);
        if r.width() < 0.0 || r.height() < 0.0 {
            return None;
        }
        let style = if style.fill.is_none() && style.stroke.is_none() { crate::nodes::generate::line_style() } else { style.clone() };
        Some(Item::path(shapes::rectangle(r), style))
    };
    if cx.p.bool("each") {
        for it in items {
            if let Some(b) = it.bounds().and_then(|b| boxed(b, &it.style))
                && !out.push(Item { attrs: it.attrs, ..b })
            {
                break;
            }
        }
    } else if let (Some(b), Some(first)) = (list_bounds(items), items.first())
        && let Some(it) = boxed(b, &first.style)
    {
        out.push(it);
    }
    Ok(())
}
