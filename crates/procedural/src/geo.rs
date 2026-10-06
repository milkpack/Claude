//! Path helpers shared by the nodes: flattening, arc-length sampling, smooth curves through
//! points. Arithmetic only (plus `libm` square roots), so results are the same everywhere.

use kurbo::{CubicBez, ParamCurve, Point, Vec2};
use vectorcraft_geom::{Anchor, AnchorKind, PathData, SubPath};

use crate::math;

fn dist(a: Point, b: Point) -> f64 {
    math::hypot(b - a)
}

/// Upper bound of a cubic's length (its control polygon).
fn cubic_hull_len(c: &CubicBez) -> f64 {
    dist(c.p0, c.p1) + dist(c.p1, c.p2) + dist(c.p2, c.p3)
}

/// `sp` as a polyline with points at most about `step` apart (closed subpaths don't repeat the
/// first point). At most `max_points` points: the step grows to fit.
pub fn flatten(sp: &SubPath, step: f64, max_points: usize) -> Vec<Point> {
    let n = sp.segment_count();
    let Some(first) = sp.anchors.first() else { return vec![] };
    if n == 0 {
        return vec![first.p];
    }
    let segs: Vec<CubicBez> = (0..n).map(|i| sp.segment(i)).collect();
    let lens: Vec<f64> = segs.iter().enumerate().map(|(i, c)| if sp.segment_is_line(i) { dist(c.p0, c.p3) } else { cubic_hull_len(c) }).collect();
    let total: f64 = lens.iter().sum();
    let max_points = max_points.max(n + 1);
    let mut step = if step.is_finite() && step > 0.0 { step } else { 1.0 };
    if total.is_finite() && total / step > max_points as f64 {
        step = total / max_points as f64;
    }
    let mut out = vec![first.p];
    for (i, c) in segs.iter().enumerate() {
        let len = lens.get(i).copied().unwrap_or(0.0);
        let k = if len.is_finite() { ((len / step).ceil() as usize).clamp(1, max_points) } else { 1 };
        for j in 1..=k {
            if out.len() >= max_points {
                break;
            }
            out.push(c.eval(j as f64 / k as f64));
        }
    }
    if sp.closed && out.len() > 1 && out.first().zip(out.last()).is_some_and(|(a, b)| dist(*a, *b) < 1e-9) {
        out.pop();
    }
    out
}

/// A finer flattening for measuring (about 16 points per segment, a fixed count so it is cheap).
pub fn flatten_fine(sp: &SubPath, max_points: usize) -> Vec<Point> {
    let n = sp.segment_count();
    let Some(first) = sp.anchors.first() else { return vec![] };
    let per = max_points.checked_div(n).unwrap_or(1).clamp(1, 16);
    let mut out = vec![first.p];
    for i in 0..n {
        let c = sp.segment(i);
        let k = if sp.segment_is_line(i) { 1 } else { per };
        for j in 1..=k {
            out.push(c.eval(j as f64 / k as f64));
        }
    }
    out
}

/// A polyline measured by arc length.
pub struct Measured {
    pts: Vec<Point>,
    /// Cumulative length at each point.
    cum: Vec<f64>,
}

impl Measured {
    /// `pts` (closed: the walk returns to the first point).
    pub fn new(mut pts: Vec<Point>, closed: bool) -> Self {
        if closed && let Some(f) = pts.first().copied() {
            pts.push(f);
        }
        let mut cum = Vec::with_capacity(pts.len());
        let mut acc = 0.0;
        for (i, p) in pts.iter().enumerate() {
            if let Some(prev) = i.checked_sub(1).and_then(|j| pts.get(j)) {
                acc += dist(*prev, *p);
            }
            cum.push(acc);
        }
        Self { pts, cum }
    }
    pub fn length(&self) -> f64 {
        self.cum.last().copied().unwrap_or(0.0)
    }
    /// The point at arc length `s` and the direction of travel there.
    pub fn at(&self, s: f64) -> Option<(Point, Vec2)> {
        let first = *self.pts.first()?;
        if self.pts.len() < 2 {
            return Some((first, Vec2::new(1.0, 0.0)));
        }
        let s = if s.is_finite() { s.clamp(0.0, self.length()) } else { 0.0 };
        // First point whose cumulative length reaches `s`.
        let i = self.cum.partition_point(|c| *c < s).clamp(1, self.pts.len() - 1);
        let (a, b) = (*self.pts.get(i - 1)?, *self.pts.get(i)?);
        let (ca, cb) = (*self.cum.get(i - 1)?, *self.cum.get(i)?);
        let t = if cb - ca > 1e-12 { (s - ca) / (cb - ca) } else { 0.0 };
        let mut d = b - a;
        // A zero-length piece: look along the path for a direction.
        if math::hypot(d) < 1e-12 {
            d = self.pts.windows(2).skip(i.saturating_sub(1)).map(|w| w[1] - w[0]).find(|v| math::hypot(*v) > 1e-12).unwrap_or(Vec2::new(1.0, 0.0));
        }
        Some((a.lerp(b, t), d))
    }
    /// `n` points evenly by arc length (closed: without repeating the start).
    pub fn resample(&self, n: usize, closed: bool) -> Vec<Point> {
        let len = self.length();
        (0..n)
            .filter_map(|i| {
                let f = if closed {
                    i as f64 / n as f64
                } else if n > 1 {
                    i as f64 / (n - 1) as f64
                } else {
                    0.5
                };
                self.at(f * len).map(|(p, _)| p)
            })
            .collect()
    }
}

/// A subpath through `pts`: straight segments, or smooth Catmull-Rom curves.
pub fn through(pts: &[Point], closed: bool, smooth: bool) -> SubPath {
    if !smooth || pts.len() < 3 {
        return SubPath::polyline(pts, closed);
    }
    let n = pts.len();
    let get = |i: isize| -> Point {
        let idx = if closed { i.rem_euclid(n as isize) as usize } else { i.clamp(0, n as isize - 1) as usize };
        pts.get(idx).copied().unwrap_or_default()
    };
    let anchors = (0..n as isize)
        .map(|i| {
            let p = get(i);
            let tangent = (get(i + 1) - get(i - 1)) / 6.0;
            let open_end = !closed && (i == 0 || i == n as isize - 1);
            if open_end {
                let mut a = Anchor::corner(p);
                if i == 0 {
                    a.h_out = p + tangent;
                } else {
                    a.h_in = p - tangent;
                }
                a
            } else {
                Anchor { p, h_in: p - tangent, h_out: p + tangent, kind: AnchorKind::Smooth }
            }
        })
        .collect();
    SubPath::new(anchors, closed)
}

/// A path's anchors moved by `f` (handles move with their anchor).
pub fn map_points(path: &PathData, f: impl Fn(Point) -> Point) -> PathData {
    let mut out = path.clone();
    for sp in &mut out.subpaths {
        for a in &mut sp.anchors {
            let q = f(a.p);
            let d = q - a.p;
            *a = Anchor { p: q, h_in: a.h_in + d, h_out: a.h_out + d, kind: a.kind };
        }
    }
    out
}

/// Signed area of a closed polyline (shoelace; positive = clockwise on a y-down page).
pub fn polygon_area(pts: &[Point]) -> f64 {
    let n = pts.len();
    (0..n).map(|i| pts.get(i).zip(pts.get((i + 1) % n)).map_or(0.0, |(a, b)| a.x * b.y - b.x * a.y)).sum::<f64>() / 2.0
}

/// Unit vector (or zero).
pub fn unit(v: Vec2) -> Vec2 {
    let l = math::hypot(v);
    if l > 1e-12 { v / l } else { Vec2::ZERO }
}

#[cfg(test)]
mod tests {
    use vectorcraft_geom::shapes;

    use super::*;

    #[test]
    fn flatten_caps_and_measures() {
        let c = shapes::ellipse(kurbo::Rect::new(-50.0, -50.0, 50.0, 50.0));
        let sp = &c.subpaths[0];
        let pts = flatten(sp, 1.0, 100_000);
        assert!(pts.len() > 250 && pts.len() < 450, "{}", pts.len());
        let pts = flatten(sp, 1e-9, 64);
        assert!(pts.len() <= 64);
        let m = Measured::new(flatten_fine(sp, 1000), true);
        assert!((m.length() - std::f64::consts::TAU * 50.0).abs() < 1.0, "{}", m.length());
        let r = m.resample(8, true);
        assert_eq!(r.len(), 8);
        let line = SubPath::polyline(&[Point::new(0.0, 0.0), Point::new(10.0, 0.0)], false);
        let m = Measured::new(flatten_fine(&line, 100), false);
        let (p, d) = m.at(2.5).unwrap();
        assert_eq!(p, Point::new(2.5, 0.0));
        assert!(d.x > 0.0);
        assert_eq!(m.resample(3, false), vec![Point::new(0.0, 0.0), Point::new(5.0, 0.0), Point::new(10.0, 0.0)]);
        assert!(Measured::new(vec![], false).at(1.0).is_none());
        assert!(flatten(&SubPath::default(), 1.0, 10).is_empty());
    }

    #[test]
    fn smooth_through_points() {
        let pts = [Point::new(0.0, 0.0), Point::new(10.0, 5.0), Point::new(20.0, 0.0), Point::new(30.0, 5.0)];
        let sp = through(&pts, false, true);
        assert_eq!(sp.anchors.len(), 4);
        assert!(sp.anchors[1].has_in() && sp.anchors[1].has_out());
        let sp = through(&pts, true, true);
        assert!(sp.closed && sp.anchors[0].has_in());
        assert_eq!(through(&pts[..2], false, true).anchors.len(), 2);
    }
}
