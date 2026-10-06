//! `style.*`: paint the items.

use kurbo::Point;
use vectorcraft_color::{Color, Paint};

use crate::catalogue::{Ctx, Inputs};
use crate::item::{Item, Out, list_bounds};
use crate::math;

/// 0..1 per item, by the `by` param: list order, the `index` attribute, the random attribute,
/// x, y, or the distance from the list's centre.
pub fn ramp(cx: &Ctx, items: &[Item]) -> Vec<f64> {
    let n = items.len();
    let norm = |v: Vec<f64>| -> Vec<f64> {
        let lo = v.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let span = hi - lo;
        v.into_iter().map(|x| if span > 1e-12 && span.is_finite() { ((x - lo) / span).clamp(0.0, 1.0) } else { 0.0 }).collect()
    };
    match cx.p.choice("by") {
        "index" => norm(items.iter().map(|it| f64::from(it.attrs.index)).collect()),
        "random" => items.iter().map(|it| it.attrs.random.clamp(0.0, 1.0)).collect(),
        "x" => norm(items.iter().map(|it| it.center().x).collect()),
        "y" => norm(items.iter().map(|it| it.center().y).collect()),
        "radius" => {
            let c = list_bounds(items).map_or(Point::ZERO, |b| b.center());
            norm(items.iter().map(|it| math::hypot(it.center() - c)).collect())
        }
        _ => (0..n).map(|i| if n > 1 { i as f64 / (n - 1) as f64 } else { 0.0 }).collect(),
    }
}

fn rgb(c: Color) -> [f64; 3] {
    let [r, g, b] = c.to_rgb_uncalibrated();
    [f64::from(r), f64::from(g), f64::from(b)]
}

fn from_rgb([r, g, b]: [f64; 3]) -> Color {
    Color::rgb(r.clamp(0.0, 1.0) as f32, g.clamp(0.0, 1.0) as f32, b.clamp(0.0, 1.0) as f32)
}

/// RGB blend of two colours (as display RGB, without colour management).
pub fn lerp_color(a: Color, b: Color, t: f64) -> Color {
    let (a, b) = (rgb(a), rgb(b));
    from_rgb([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t])
}

/// RGB → HSL (hue in degrees 0..360, s and l 0..1).
pub fn to_hsl([r, g, b]: [f64; 3]) -> [f64; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-12 {
        return [0.0, 0.0, l];
    }
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [h * 60.0, s, l]
}

/// HSL → RGB.
pub fn from_hsl([h, s, l]: [f64; 3]) -> [f64; 3] {
    let h = h.rem_euclid(360.0) / 360.0;
    if s <= 0.0 {
        return [l, l, l];
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |t: f64| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0)]
}

/// HSL blend, the hue taking the short way round the wheel.
fn lerp_hsl(a: Color, b: Color, t: f64) -> Color {
    let (x, y) = (to_hsl(rgb(a)), to_hsl(rgb(b)));
    let mut dh = y[0] - x[0];
    if dh > 180.0 {
        dh -= 360.0;
    } else if dh < -180.0 {
        dh += 360.0;
    }
    from_rgb(from_hsl([x[0] + dh * t, x[1] + (y[1] - x[1]) * t, x[2] + (y[2] - x[2]) * t]))
}

fn set_paint(it: &mut Item, stroke: bool, p: Paint) {
    if stroke {
        it.style.stroke = Some(p);
        if it.style.stroke_width.is_none() {
            it.style.stroke_width = Some(1.0);
        }
    } else {
        it.style.fill = Some(p);
    }
}

fn solid_of(it: &Item, stroke: bool) -> Option<Color> {
    match if stroke { &it.style.stroke } else { &it.style.fill } {
        Some(Paint::Solid { color, .. }) => Some(*color),
        _ => None,
    }
}

fn each(inputs: &Inputs, out: &mut Out, mut f: impl FnMut(usize, &mut Item)) {
    for (i, it) in inputs.get(0).iter().enumerate() {
        let mut c = it.clone();
        f(i, &mut c);
        if !out.push(c) {
            break;
        }
    }
}

pub fn fill(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let p = cx.p.paint("color");
    each(inputs, out, |_, it| it.style.fill = Some(p.clone()));
    Ok(())
}

pub fn stroke(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let p = cx.p.paint("color");
    let w = cx.p.num("width");
    each(inputs, out, |_, it| {
        it.style.stroke = Some(p.clone());
        it.style.stroke_width = Some(w);
    });
    Ok(())
}

pub fn color_ramp(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let (from, to) = (cx.p.color("from"), cx.p.color("to"));
    let hsl = cx.p.choice("space") == "hsl";
    let stroke = cx.p.choice("target") == "stroke";
    let ts = ramp(cx, inputs.get(0));
    each(inputs, out, |i, it| {
        let t = ts.get(i).copied().unwrap_or(0.0);
        let p = match (from, to) {
            (Some(a), Some(b)) => Paint::solid(if hsl { lerp_hsl(a, b, t) } else { lerp_color(a, b, t) }),
            // One end none: the first half takes one, the second half the other.
            (a, b) => (if t < 0.5 { a } else { b }).map_or(Paint::None, Paint::solid),
        };
        set_paint(it, stroke, p);
    });
    Ok(())
}

pub fn hue_shift(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let deg = cx.p.num("degrees");
    let stroke = cx.p.choice("target") == "stroke";
    let ts = ramp(cx, inputs.get(0));
    each(inputs, out, |i, it| {
        if let Some(c) = solid_of(it, stroke) {
            let [h, s, l] = to_hsl(rgb(c));
            let t = ts.get(i).copied().unwrap_or(0.0);
            set_paint(it, stroke, Paint::solid(from_rgb(from_hsl([h + deg * t, s, l]))));
        }
    });
    Ok(())
}

pub fn opacity_ramp(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let (a, b) = (cx.p.num("from"), cx.p.num("to"));
    let ts = ramp(cx, inputs.get(0));
    each(inputs, out, |i, it| {
        let t = ts.get(i).copied().unwrap_or(0.0);
        it.style.opacity = (it.style.opacity * (a + (b - a) * t)).clamp(0.0, 1.0);
    });
    Ok(())
}

pub fn random_color(cx: &Ctx, inputs: &Inputs, out: &mut Out) -> Result<(), String> {
    let palette = cx.p.palette("palette");
    let stroke = cx.p.choice("target") == "stroke";
    if palette.is_empty() {
        return Err("the palette is empty".into());
    }
    each(inputs, out, |i, it| {
        let k = ((cx.rand(i, 0xC0) * palette.len() as f64) as usize).min(palette.len() - 1);
        if let Some(c) = palette.get(k) {
            set_paint(it, stroke, Paint::solid(*c));
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_round_trips() {
        for c in [[1.0, 0.0, 0.0], [0.2, 0.4, 0.6], [0.5, 0.5, 0.5], [0.0, 1.0, 0.3], [0.9, 0.1, 0.8]] {
            let back = from_hsl(to_hsl(c));
            for k in 0..3 {
                assert!((back[k] - c[k]).abs() < 1e-9, "{c:?} → {back:?}");
            }
        }
        assert_eq!(to_hsl([0.0, 0.0, 1.0])[0], 240.0);
        // Red to blue the short way passes magenta, not green.
        let mid = rgb(lerp_hsl(Color::rgb(1.0, 0.0, 0.0), Color::rgb(0.0, 0.0, 1.0), 0.5));
        assert!(mid[0] > 0.9 && mid[2] > 0.9 && mid[1] < 0.1, "{mid:?}");
    }
}
