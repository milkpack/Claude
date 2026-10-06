//! Raster filter effects (PhotoCraft's filters) in the renderer: every filter on a vector
//! rectangle, the reach of blurs, resolution independence, stacking with other raster effects,
//! every object kind, and the cache.

use std::sync::Arc;

use serde_json::{Value, json};
use vectorcraft_color::{Color, Paint};
use vectorcraft_doc::text::{CharStyle, TextObject};
use vectorcraft_doc::{Appearance, Effect, Node, NodeKind, Symbol};
use vectorcraft_geom::{Point, shapes};

use super::*;

fn fx(id: &str, params: Value) -> Effect {
    Effect { id: id.into(), params, visible: true }
}

fn doc_with(mut d: Document, mut n: Node) -> Document {
    n.id = d.alloc_id();
    let l = d.layers[0].id;
    d.insert(Some(l), 0, n).unwrap();
    d
}

/// A 100×100 document with a red 40×40 rectangle at (30, 30) stroked 4 pt blue, with `effects`.
fn rect_doc(effects: Vec<Effect>) -> Document {
    let mut n = Node::path(
        NodeId(0),
        shapes::rectangle(Rect::new(30.0, 30.0, 70.0, 70.0)),
        Appearance::basic(Paint::solid(Color::rgb(0.85, 0.2, 0.2)), Paint::solid(Color::rgb(0.0, 0.0, 1.0)), 4.0),
    );
    n.appearance.effects = effects;
    doc_with(Document::new(100.0, 100.0), n)
}

fn render_at(d: &Document, scale: f64, threads: u16) -> Rendered {
    let mut r = Renderer::new();
    r.threads = threads;
    let px = (100.0 * scale).round() as u32;
    r.render(d, px, px, Affine::scale(scale), &RenderOptions { background: Some([255, 255, 255, 255]), ..Default::default() })
}

fn render(d: &Document) -> Rendered {
    render_at(d, 1.0, 0)
}

fn lum(p: [u8; 4]) -> u32 {
    p[0] as u32 + p[1] as u32 + p[2] as u32
}

const WHITE: u32 = 765;

fn changed(a: &Rendered, b: &Rendered) -> usize {
    a.pixels.chunks(4).zip(b.pixels.chunks(4)).filter(|(x, y)| x.iter().zip(y.iter()).any(|(p, q)| p.abs_diff(*q) > 3)).count()
}

/// Parameters that make each filter visible on the test rectangle.
fn params_for(id: &str) -> Value {
    match id {
        "distort.shear" => json!({"points": "0,0 0.5,0.8 1,0"}),
        "other.offset" => json!({"horizontal": 9, "vertical": 4}),
        "sharpen.unsharpMask" => json!({"amount": 400, "radius": 3}),
        "sharpen.smart" => json!({"amount": 400, "radius": 3}),
        "noise.median" | "noise.dustAndScratches" => json!({"radius": 4}),
        "blur.radial" => json!({"amount": 60}),
        "noise.add" => json!({"amount": 100}),
        "other.minimum" | "other.maximum" => json!({"radius": 3}),
        "pixelate.mosaic" => json!({"cellSize": 12}),
        _ => json!({}),
    }
}

#[test]
fn every_raster_filter_renders_on_a_vector_rectangle() {
    let plain = render(&rect_doc(vec![]));
    let mut unchanged = vec![];
    for d in vectorcraft_effects::pixel::pixel_defs() {
        let doc = rect_doc(vec![fx(d.id, params_for(d.id))]);
        let a = render(&doc);
        assert_eq!(a.pixels.len(), plain.pixels.len());
        // Deterministic: a fresh renderer gives the same picture.
        assert_eq!(a.pixels, render(&doc).pixels, "{} is deterministic", d.id);
        if changed(&a, &plain) == 0 {
            unchanged.push(d.id);
        }
    }
    // Edge-preserving smoothing leaves flat colours alone (the effects crate's tests run them on
    // textured pixels).
    unchanged.retain(|id| !matches!(*id, "blur.smart" | "blur.surface" | "pixelate.facet"));
    assert!(unchanged.is_empty(), "filters that left the art unchanged: {unchanged:?}");
}

#[test]
fn a_blur_spreads_past_the_object_and_isnt_clipped() {
    let plain = render(&rect_doc(vec![]));
    assert_eq!(lum(plain.pixel(24, 50)), WHITE);
    let img = render(&rect_doc(vec![fx("blur.box", json!({"radius": 6}))]));
    let edge = lum(img.pixel(24, 50));
    assert!(edge < WHITE - 10, "the blur reaches 6 pt outside: {edge}");
    // Symmetric on both sides: the raster had room on each.
    let (l, r) = (lum(img.pixel(25, 50)), lum(img.pixel(74, 50)));
    assert!(l.abs_diff(r) < 40, "left {l} right {r}");
    // The bounds the renderer culls with include the reach.
    let n = &rect_doc(vec![fx("blur.box", json!({"radius": 6}))]).layers[0].children().unwrap()[0].clone();
    let b = crate::fx::cull_bounds(n).unwrap();
    assert!(b.x0 <= 22.0 && b.x1 >= 78.0, "{b:?}");
}

#[test]
fn distances_are_points_so_the_look_doesnt_change_with_zoom() {
    // At 2× the blur reaches as far in document units: the same point blurs the same.
    let d = rect_doc(vec![fx("blur.box", json!({"radius": 6}))]);
    let one = render_at(&d, 1.0, 0);
    let two = render_at(&d, 2.0, 0);
    for x in [24u32, 27, 33, 50] {
        let (a, b) = (lum(one.pixel(x, 50)), lum(two.pixel(2 * x, 100)));
        assert!(a.abs_diff(b) < 60, "x {x}: 1× {a} vs 2× {b}");
    }
    // Twirl centres on the object whatever the zoom.
    let t = rect_doc(vec![fx("distort.twirl", json!({"angle": 120}))]);
    let (a, b) = (render_at(&t, 1.0, 0), render_at(&t, 2.0, 0));
    let mismatched = (20..80)
        .step_by(5)
        .flat_map(|y| (20..80).step_by(5).map(move |x| (x, y)))
        .filter(|(x, y)| lum(a.pixel(*x, *y)).abs_diff(lum(b.pixel(2 * x + 1, 2 * y + 1))) > 150)
        .count();
    assert!(mismatched <= 6, "{mismatched} samples differ between zooms");
}

#[test]
fn multithreaded_rendering_matches() {
    for id in ["blur.motion", "distort.twirl", "gallery.cutout", "stylize.emboss"] {
        let d = rect_doc(vec![fx(id, params_for(id))]);
        let (st, mt) = (render_at(&d, 1.0, 0), render_at(&d, 1.0, 3));
        let worst = st.pixels.iter().zip(&mt.pixels).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(worst <= 4, "{id}: max channel difference {worst}");
    }
}

#[test]
fn effects_after_a_filter_apply_to_the_filtered_art() {
    // A drop shadow after a 40-pt-wide offset: the shadow is of the offset art.
    let shadow = json!({"x": 6, "y": 6, "blur": 0, "opacity": 100, "mode": "normal", "color": "#000000"});
    let a = render(&rect_doc(vec![fx("stylize.dropShadow", shadow.clone()), fx("other.invert", json!({}))]));
    let b = render(&rect_doc(vec![fx("other.invert", json!({})), fx("stylize.dropShadow", shadow)]));
    // Shadow first, then invert: the shadow is inverted too (white); invert first: a black shadow.
    assert!(lum(a.pixel(73, 73)) > 600, "inverted shadow: {:?}", a.pixel(73, 73));
    assert!(lum(b.pixel(73, 73)) < 100, "black shadow under the inverted art: {:?}", b.pixel(73, 73));
    // The art is inverted in both: red becomes cyan.
    let p = b.pixel(45, 45);
    assert!(p[0] < 80 && p[1] > 180 && p[2] > 180, "{p:?}");
}

#[test]
fn hidden_and_invalid_filters_leave_the_art_alone() {
    let plain = render(&rect_doc(vec![]));
    let mut hidden = fx("blur.box", json!({"radius": 6}));
    hidden.visible = false;
    assert_eq!(changed(&render(&rect_doc(vec![hidden])), &plain), 0);
    assert_eq!(changed(&render(&rect_doc(vec![fx("blur.box", json!({"radius": "much"}))])), &plain), 0);
}

#[test]
fn filters_apply_to_type_images_symbols_and_groups() {
    // Type.
    let style = CharStyle { size: 40.0, fill: Paint::solid(Color::BLACK), ..Default::default() };
    let text = |effects| {
        let mut n = Node::new(NodeId(0), NodeKind::Text(Box::new(TextObject::point(Point::new(5.0, 60.0), "IIII", style.clone()))));
        n.appearance.effects = effects;
        doc_with(Document::new(100.0, 100.0), n)
    };
    assert!(changed(&render(&text(vec![fx("blur.motion", json!({"distance": 12}))])), &render(&text(vec![]))) > 50);
    // A symbol instance.
    let symbol = |effects| {
        let mut d = Document::new(100.0, 100.0);
        let art = Node::path(
            d.alloc_id(),
            shapes::rectangle(Rect::new(-15.0, -15.0, 15.0, 15.0)),
            Appearance::basic(Paint::solid(Color::BLACK), Paint::None, 0.0),
        );
        d.symbols.push(Symbol { name: "Sq".into(), art: Arc::new(art) });
        let mut n = Node::new(NodeId(0), NodeKind::SymbolInstance { symbol: "Sq".into(), xf: Affine::translate((50.0, 50.0)) });
        n.appearance.effects = effects;
        doc_with(d, n)
    };
    assert!(changed(&render(&symbol(vec![fx("blur.motion", json!({"distance": 12}))])), &render(&symbol(vec![]))) > 50);
    // A group: one filter over its members' composite.
    let group = |effects| {
        let mut d = Document::new(100.0, 100.0);
        let a = Node::path(
            d.alloc_id(),
            shapes::rectangle(Rect::new(20.0, 20.0, 50.0, 50.0)),
            Appearance::basic(Paint::solid(Color::BLACK), Paint::None, 0.0),
        );
        let b = Node::path(
            d.alloc_id(),
            shapes::rectangle(Rect::new(50.0, 50.0, 80.0, 80.0)),
            Appearance::basic(Paint::solid(Color::rgb(0.0, 0.5, 0.0)), Paint::None, 0.0),
        );
        let mut g = Node::new(NodeId(0), NodeKind::Group { children: vec![Arc::new(a), Arc::new(b)], clip: false });
        g.appearance.effects = effects;
        doc_with(d, g)
    };
    assert!(changed(&render(&group(vec![fx("pixelate.mosaic", json!({"cellSize": 13}))])), &render(&group(vec![]))) > 50);
    // One fill's own filter.
    let mut d = rect_doc(vec![]);
    let id = d.layers[0].children().unwrap()[0].id;
    let mut n = (*d.node(id).unwrap()).clone();
    n.appearance.items[0].effects_mut().push(fx("other.invert", json!({})));
    let l = d.layers[0].id;
    d.remove(id).unwrap();
    d.insert(Some(l), 0, n).unwrap();
    let p = render(&d).pixel(50, 50);
    assert!(p[0] < 80 && p[1] > 180, "the fill is inverted: {p:?}");
}

#[test]
fn the_filtered_raster_is_reused_while_panning() {
    let d = rect_doc(vec![fx("gallery.watercolor", json!({}))]);
    let mut r = Renderer::new();
    r.threads = 0;
    let opts = RenderOptions { background: Some([255, 255, 255, 255]), ..Default::default() };
    let a = r.render(&d, 100, 100, Affine::IDENTITY, &opts);
    assert_eq!(r.pixel_cache.len(), 1);
    let b = r.render(&d, 100, 100, Affine::translate((10.0, 0.0)), &opts);
    assert_eq!(r.pixel_cache.len(), 1, "the pan reused the raster");
    // The panned picture is the same art moved.
    for (x, y) in [(40u32, 40u32), (50, 50), (60, 45)] {
        assert_eq!(a.pixel(x, y), b.pixel(x + 10, y), "({x}, {y})");
    }
}
