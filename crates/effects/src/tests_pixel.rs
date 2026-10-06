//! Raster filter effects (`pixel`): catalogue, parameters, the pixel bridge and every filter.

use proptest::prelude::*;
use serde_json::{Value, json};
use vectorcraft_doc::Effect;

use super::*;
use crate::pixel::{PixelFx, pixel_defs, run_filter};

/// A `w`×`h` premultiplied RGBA8 picture: transparent, with an opaque two-tone square in the
/// middle (red left, blue right, split left of the centre) and a half-transparent green bar.
fn picture(w: u32, h: u32) -> Vec<u8> {
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let inside = x >= w / 4 && x < 3 * w / 4 && y >= h / 4 && y < 3 * h / 4;
            let c: [u8; 4] = if inside {
                {
                    // A little texture, so edge-preserving filters have something to smooth.
                    let t = ((x * 7 + y * 13) % 5) as u8 * 9;
                    if x < w / 2 - 3 { [255, t, 0, 255] } else { [t, 0, 255, 255] }
                }
            } else if y == h / 8 && x > 2 && x < w - 2 {
                [0, 64, 0, 128]
            } else {
                [0, 0, 0, 0]
            };
            px[i..i + 4].copy_from_slice(&c);
        }
    }
    px
}

fn bounds(w: u32, h: u32) -> [i32; 4] {
    [(w / 4) as i32, (h / 4) as i32, (3 * w / 4) as i32, (3 * h / 4) as i32]
}

fn run(id: &str, params: Value, w: u32, h: u32) -> Vec<u8> {
    let fx = PixelFx::new(id, &params).unwrap();
    let f = fx.filter(1.0).unwrap();
    let mut px = picture(w, h);
    run_filter(&mut px, w, h, &f, bounds(w, h)).unwrap();
    px
}

/// Premultiplied pixels are well formed: no channel above alpha.
fn premultiplied(px: &[u8]) -> bool {
    px.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3])
}

/// Filters whose defaults leave this picture unchanged (or nearly), with parameters that don't.
fn overrides(id: &str) -> Value {
    match id {
        "distort.shear" => json!({"points": "0,0 0.5,0.8 1,0"}),
        "other.offset" => json!({"horizontal": 7, "vertical": 3}),
        "sharpen.unsharpMask" => json!({"amount": 300, "radius": 2}),
        "noise.median" | "noise.dustAndScratches" => json!({"radius": 3}),
        "blur.radial" => json!({"amount": 40}),
        "noise.add" => json!({"amount": 80}),
        "blur.smart" => json!({"radius": 5, "threshold": 100, "quality": "high"}),
        "stylize.tiles" => json!({"count": 3, "maxOffset": 40}),
        _ => json!({}),
    }
}

#[test]
fn catalogue_has_every_filter_with_parsable_defaults() {
    let cat = effect_catalog();
    let defs: Vec<_> = pixel_defs().collect();
    assert!(defs.len() >= 95, "{} raster filters", defs.len());
    for d in &defs {
        let e = cat.iter().find(|e| e.id == d.id).unwrap_or_else(|| panic!("{} missing from the catalogue", d.id));
        assert!(e.raster && is_raster(d.id) && is_pixel(d.id) && !is_geometry(d.id), "{}", d.id);
        assert!(e.defaults.is_object(), "{}: defaults {}", d.id, d.defaults);
        assert!(e.label.ends_with('…') == e.defaults.as_object().is_some_and(|o| !o.is_empty()), "{}: ellipsis iff a dialog", d.id);
        assert_eq!(e.menu.first(), Some(&"Effect"));
        assert!(pixel::RASTER_MENUS.contains(e.menu.last().unwrap()), "{}: menu {:?}", d.id, e.menu);
        // The defaults are valid.
        check_params(d.id, &json!({})).unwrap_or_else(|err| panic!("{err}"));
        for k in d.lengths {
            assert!(e.defaults.get(*k).is_some(), "{}: length `{k}`", d.id);
            assert!(is_length(d.id, k, false));
        }
    }
    // The path effect ZigZag and the raster one are different effects.
    assert!(is_geometry("distort.zigZag") && is_pixel("distort.rasterZigZag"));
    assert!(!is_pixel("blur.gaussian"), "the native Gaussian Blur stays native");
}

#[test]
fn bad_parameters_are_errors() {
    assert!(check_params("blur.box", &json!({"radius": "lots"})).is_err());
    assert!(check_params("blur.box", &json!({"radius": null})).is_err());
    assert!(check_params("distort.wave", &json!({"type": "zigzag"})).is_err());
    assert!(check_params("other.custom", &json!({"kernel": "1 2 3"})).is_err());
    assert!(check_params("pixelate.pointillize", &json!({"background": "#12"})).is_err());
    assert!(check_params("gallery.roughPastels", &json!({"texture": "marble"})).is_err());
    // Out-of-range numbers are clamped, case and separators don't matter in choices.
    check_params("blur.box", &json!({"radius": 1e300})).unwrap();
    check_params("distort.spherize", &json!({"mode": "Horizontal Only"})).unwrap();
    check_params("gallery.roughPastels", &json!({"texture": "Brick", "strokeLength": 40})).unwrap();
    // Other effects aren't checked here.
    check_params("distort.roughen", &json!({"size": "x"})).unwrap();
    // An invalid effect paints nothing extra (and isn't a raster effect of the list).
    let bad = Effect { id: "blur.box".into(), params: json!({"radius": "x"}), visible: true };
    assert!(raster_effects(&[bad]).is_empty());
}

#[test]
fn distances_scale_with_the_resolution() {
    let fx = PixelFx::new("blur.box", &json!({"radius": 4})).unwrap();
    let at = |s: f64| match fx.filter(s).unwrap() {
        photocraft_algo::FilterParams::BoxBlur { radius } => radius,
        f => panic!("{f:?}"),
    };
    assert_eq!((at(1.0), at(300.0 / 72.0)), (4.0, 4.0 * 300.0 / 72.0));
    // Huge distances lower the evaluation resolution rather than being clipped.
    let big = PixelFx::new("blur.box", &json!({"radius": 400})).unwrap();
    assert!((big.max_scale() - 250.0 / 400.0).abs() < 1e-9);
    assert!(PixelFx::new("distort.twirl", &json!({})).unwrap().max_scale().is_infinite());
    // The outset is in points: the same at any resolution.
    let g = PixelFx::new("other.highPass", &json!({"radius": 2})).unwrap();
    assert!((g.outset_at(1.0) - g.outset_at(4.0)).abs() < 1.0, "{} {}", g.outset_at(1.0), g.outset_at(4.0));
    assert!(g.outset() >= 6.0);
    // Distortions stay within the object's bounds.
    assert_eq!(PixelFx::new("distort.twirl", &json!({})).unwrap().outset(), 0.0);
    assert!(PixelFx::new("distort.twirl", &json!({})).unwrap().is_global());
    // The list's outset adds the filters' reaches.
    let fx = |id: &str, p: Value| Effect { id: id.into(), params: p, visible: true };
    let two = [fx("blur.box", json!({"radius": 5})), fx("blur.box", json!({"radius": 5}))];
    assert!(outset(&two) >= 2.0 * outset(&two[..1]) - 1e-9);
}

#[test]
fn premultiplied_round_trip_is_lossless_enough() {
    // A filter that changes nothing (Offset by zero) gives back the same pixels, half-transparent
    // ones included.
    let (w, h) = (40, 30);
    let f = PixelFx::new("other.offset", &json!({})).unwrap().filter(1.0).unwrap();
    let src = picture(w, h);
    let mut px = src.clone();
    run_filter(&mut px, w, h, &f, [0, 0, w as i32, h as i32]).unwrap();
    for (a, b) in px.iter().zip(&src) {
        assert!(a.abs_diff(*b) <= 1, "{a} vs {b}");
    }
    // Invert keeps the alpha and inverts the straight colour: premultiplied (0, 64, 0, 128) is
    // straight (0, 128, 0) → (255, 127, 255) → premultiplied (128, 64, 128).
    let f = PixelFx::new("other.invert", &json!({})).unwrap().filter(1.0).unwrap();
    let mut px = picture(w, h);
    run_filter(&mut px, w, h, &f, bounds(w, h)).unwrap();
    let i = (((h / 8) * w + w / 2) * 4) as usize;
    let p = &px[i..i + 4];
    assert_eq!(p[3], 128);
    assert!(p[0].abs_diff(128) <= 1 && p[1].abs_diff(64) <= 1 && p[2].abs_diff(128) <= 1, "{p:?}");
    assert!(premultiplied(&px));
    // Fully transparent pixels stay transparent.
    assert_eq!(&px[0..4], &[0, 0, 0, 0]);
}

#[test]
fn wrong_buffers_are_errors_not_panics() {
    let f = PixelFx::new("other.invert", &json!({})).unwrap().filter(1.0).unwrap();
    assert!(run_filter(&mut [0u8; 15], 2, 2, &f, [0, 0, 2, 2]).is_err());
    assert!(run_filter(&mut [], 0, 0, &f, [0, 0, 0, 0]).is_err());
    assert!(run_filter(&mut [0u8; 16], 2, 2, &f, [5, 5, -3, 9]).is_ok(), "nonsense bounds fall back to the whole raster");
}

#[test]
fn every_filter_changes_the_art_deterministically() {
    let (w, h) = (48, 40);
    let src = picture(w, h);
    let mut unchanged = vec![];
    for d in pixel_defs() {
        let p = overrides(d.id);
        let a = run(d.id, p.clone(), w, h);
        let b = run(d.id, p, w, h);
        assert_eq!(a, b, "{} is deterministic", d.id);
        assert!(premultiplied(&a), "{}: premultiplied output", d.id);
        let changed = a.iter().zip(&src).filter(|(x, y)| x.abs_diff(**y) > 2).count();
        if changed == 0 {
            unchanged.push(d.id);
        }
    }
    assert!(unchanged.is_empty(), "unchanged: {unchanged:?}");
}

#[test]
fn blurs_spread_beyond_the_art() {
    // The blurred square bleeds into the margin the outset leaves (not clipped at the art).
    let (w, h) = (64, 64);
    let a = run("blur.box", json!({"radius": 4}), w, h);
    let edge = |px: &[u8], x: u32, y: u32| px[((y * w + x) * 4 + 3) as usize];
    assert_eq!(edge(&picture(w, h), w / 4 - 3, h / 2), 0);
    assert!(edge(&a, w / 4 - 3, h / 2) > 0, "blur reaches outside the square");
    // Halo-free filters don't.
    let inv = run("other.invert", json!({}), w, h);
    assert_eq!(edge(&inv, w / 4 - 3, h / 2), 0);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    /// Random parameter values never panic: they are clamped or rejected, and the filters run in
    /// bounded time on a small raster.
    #[test]
    fn random_params_never_panic(
        which in 0usize..1000,
        nums in proptest::collection::vec(prop_oneof![-1e6f64..1e6, -10.0f64..10.0, Just(f64::MAX), Just(0.0)], 8),
        words in proptest::collection::vec("[a-zA-Z#0-9 ,.]{0,12}", 4),
        scale in prop_oneof![Just(1.0f64), 0.01f64..20.0],
    ) {
        let defs: Vec<_> = pixel_defs().collect();
        let d = defs[which % defs.len()];
        let defaults: Value = serde_json::from_str(d.defaults).unwrap();
        let mut p = serde_json::Map::new();
        for (i, (k, v)) in defaults.as_object().unwrap().iter().enumerate() {
            let nv = match (v, i % 3) {
                (Value::Number(_), 0 | 1) => json!(nums[i % nums.len()]),
                (Value::String(_), 0) => json!(words[i % words.len()]),
                (_, 2) => json!(words[(i + 1) % words.len()]),
                _ => v.clone(),
            };
            p.insert(k.clone(), nv);
        }
        let Some(fx) = PixelFx::new(d.id, &Value::Object(p)) else { panic!("{}", d.id) };
        let _ = fx.outset();
        let _ = fx.max_scale();
        let s = scale.min(fx.max_scale()).max(1e-3);
        if let Ok(f) = fx.filter(s) {
            let (w, h) = (24, 20);
            let mut px = picture(w, h);
            let start = std::time::Instant::now();
            run_filter(&mut px, w, h, &f, bounds(w, h)).unwrap();
            prop_assert!(start.elapsed().as_secs() < 20, "{} took {:?}", d.id, start.elapsed());
            prop_assert!(premultiplied(&px));
        }
    }
}
