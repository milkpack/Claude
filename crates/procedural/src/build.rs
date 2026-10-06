//! Items → document nodes (the procedural group's children).

use std::sync::Arc;

use kurbo::{Affine, Rect};
use vectorcraft_color::{Color, Paint};
use vectorcraft_doc::{Appearance, Node, NodeId, NodeKind};

use crate::item::{Geom, Item, Style};
use crate::math;

/// Radius of the dot a bare point is drawn as.
pub const POINT_RADIUS: f64 = 2.0;

/// Document nodes for `items`, placed by `placement` (graph space → document). `alloc` hands out
/// node ids (the document's, so collaboration id ranges are respected).
pub fn build_nodes(items: &[Item], placement: Affine, alloc: &mut dyn FnMut() -> NodeId) -> Vec<Arc<Node>> {
    let placement = if math::affine_finite(&placement) { placement } else { Affine::IDENTITY };
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        let a = placement * it.transform;
        let node = match &it.geom {
            Geom::Path { path, rule } => {
                let mut n = Node::path(alloc(), path.transformed(a), appearance(&it.style, None));
                if let NodeKind::Path { rule: r, .. } = &mut n.kind {
                    *r = *rule;
                }
                n.opacity = opacity(it.style.opacity);
                n
            }
            Geom::Point => {
                let c = placement * it.origin();
                let r = Rect::new(c.x - POINT_RADIUS, c.y - POINT_RADIUS, c.x + POINT_RADIUS, c.y + POINT_RADIUS);
                let style = Style { stroke: Some(it.style.stroke.clone().unwrap_or(Paint::None)), ..it.style.clone() };
                let mut n = Node::path(alloc(), vectorcraft_geom::shapes::ellipse(r), appearance(&style, Some(Paint::solid(Color::BLACK))));
                n.opacity = opacity(it.style.opacity);
                n
            }
            Geom::Art { node, .. } => {
                let mut n = (**node).clone();
                fresh_ids(&mut n, alloc);
                restyle(&mut n, &it.style);
                n.opacity = (n.opacity * opacity(it.style.opacity)).clamp(0.0, 1.0);
                if a != Affine::IDENTITY {
                    n.transform(a, false);
                }
                n
            }
        };
        out.push(Arc::new(node));
    }
    out
}

fn opacity(o: f64) -> f32 {
    if o.is_finite() { o.clamp(0.0, 1.0) as f32 } else { 1.0 }
}

/// A basic fill + stroke appearance from a path item's style (`default_fill` when it has none).
fn appearance(s: &Style, default_fill: Option<Paint>) -> Appearance {
    let fill = s.fill.clone().or(default_fill).unwrap_or(Paint::None);
    let stroke = s.stroke.clone().unwrap_or(Paint::None);
    let w = s.stroke_width.filter(|w| w.is_finite()).unwrap_or(1.0).clamp(0.0, 1000.0);
    Appearance::basic(fill, stroke, w)
}

/// New ids for `n` and everything in it (its children and its opacity mask's art).
fn fresh_ids(n: &mut Node, alloc: &mut dyn FnMut() -> NodeId) {
    n.id = alloc();
    if let Some(m) = &mut n.mask {
        fresh_ids(Arc::make_mut(&mut m.art), alloc);
    }
    for c in n.children_mut().into_iter().flatten() {
        fresh_ids(Arc::make_mut(c), alloc);
    }
}

/// Style overrides on the paths in art (type, images and the rest keep their look).
fn restyle(n: &mut Node, s: &Style) {
    if s.fill.is_none() && s.stroke.is_none() && s.stroke_width.is_none() {
        return;
    }
    match &n.kind {
        NodeKind::Path { .. } | NodeKind::Compound { .. } => {
            if let Some(f) = &s.fill {
                n.appearance.set_fill(f.clone());
            }
            if let Some(p) = &s.stroke {
                n.appearance.set_stroke(p.clone());
            }
            if let Some(w) = s.stroke_width.filter(|w| w.is_finite())
                && let Some(st) = n.appearance.stroke_mut()
            {
                st.width = w.clamp(0.0, 1000.0);
            }
        }
        _ => {
            for c in n.children_mut().into_iter().flatten() {
                restyle(Arc::make_mut(c), s);
            }
        }
    }
}
