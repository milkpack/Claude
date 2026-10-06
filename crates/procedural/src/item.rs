//! What flows along the wires: a list of [`Item`]s, each a piece of geometry with its own style,
//! transform and attributes (Graphite's "vector data" instances).

use std::sync::Arc;

use kurbo::{Affine, Point, Rect};
use vectorcraft_color::Paint;
use vectorcraft_doc::{Node, NodeKind};
use vectorcraft_geom::{FillRule, PathData};

use crate::limits::{MAX_ANCHORS_PER_PATH, MAX_ITEMS, MAX_TOTAL_ANCHORS, MAX_WEIGHT};
use crate::math;

/// The geometry of one item, in the item's own space (its [`Item::transform`] maps it to graph
/// space).
#[derive(Clone, Debug, PartialEq)]
pub enum Geom {
    /// A (possibly compound) path.
    Path { path: PathData, rule: FillRule },
    /// A bare point at the item's origin (scatter, grid points…); drawn as a small dot when it
    /// reaches the output.
    Point,
    /// A copy of user art (`source.art`), kept as document nodes so nothing of its look is lost.
    Art { node: Arc<Node>, weight: usize },
}

/// Paint of an item. For paths `None` means none; for art, `None` keeps the art's own paint.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub fill: Option<Paint>,
    pub stroke: Option<Paint>,
    /// Stroke weight in points (`None`: the art's own; paths default to 1).
    pub stroke_width: Option<f64>,
    /// Multiplies the object's opacity (0..1).
    pub opacity: f64,
}

impl Default for Style {
    fn default() -> Self {
        Self { fill: None, stroke: None, stroke_width: None, opacity: 1.0 }
    }
}

impl Style {
    /// A filled shape without a stroke.
    pub fn filled(p: Paint) -> Self {
        Self { fill: Some(p), ..Self::default() }
    }
    /// A stroked line without a fill.
    pub fn stroked(p: Paint, width: f64) -> Self {
        Self { stroke: Some(p), stroke_width: Some(width), ..Self::default() }
    }
}

/// Per-item attributes (Graphite's instance attributes).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Attrs {
    /// Position in the list that made it (instancers number their copies 0, 1, 2…).
    pub index: u32,
    /// A random number in [0, 1) fixed per item (from the seed, the node and the index).
    pub random: f64,
}

/// One item on a wire.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub geom: Geom,
    pub transform: Affine,
    pub style: Style,
    pub attrs: Attrs,
}

impl Item {
    pub fn path(path: PathData, style: Style) -> Self {
        Self { geom: Geom::Path { path, rule: FillRule::NonZero }, transform: Affine::IDENTITY, style, attrs: Attrs::default() }
    }
    pub fn point(p: Point) -> Self {
        Self { geom: Geom::Point, transform: Affine::translate(p.to_vec2()), style: Style::default(), attrs: Attrs::default() }
    }
    pub fn art(node: Arc<Node>) -> Self {
        let weight = node.count().max(1);
        Self { geom: Geom::Art { node, weight }, transform: Affine::IDENTITY, style: Style::default(), attrs: Attrs::default() }
    }
    /// The same item placed by `a` (applied after its own transform).
    pub fn transformed(&self, a: Affine) -> Self {
        Self { transform: a * self.transform, ..self.clone() }
    }
    /// How many document nodes this item turns into.
    pub fn weight(&self) -> usize {
        match &self.geom {
            Geom::Art { weight, .. } => *weight,
            _ => 1,
        }
    }
    pub fn anchors(&self) -> usize {
        match &self.geom {
            Geom::Path { path, .. } => path.anchor_count(),
            Geom::Point => 1,
            Geom::Art { weight, .. } => *weight,
        }
    }
    /// The item's origin in graph space.
    pub fn origin(&self) -> Point {
        self.transform * Point::ZERO
    }
    /// Graph-space bounds (art: the box round its transformed bounds).
    pub fn bounds(&self) -> Option<Rect> {
        let r = match &self.geom {
            Geom::Path { path, .. } => path.transformed(self.transform).bounds(),
            Geom::Point => {
                let p = self.origin();
                Some(Rect::from_points(p, p))
            }
            Geom::Art { node, .. } => node.geometric_bounds().map(|b| self.transform.transform_rect_bbox(b)),
        }?;
        (r.x0.is_finite() && r.y0.is_finite() && r.x1.is_finite() && r.y1.is_finite()).then_some(r)
    }
    /// The item's centre (its bounds' centre, or its origin when it has no bounds).
    pub fn center(&self) -> Point {
        self.bounds().map_or_else(|| self.origin(), |b| b.center())
    }
    /// The item's paths in graph space, each with its fill rule and the style it paints with.
    /// Art contributes the paths and compound paths in it (type and images have none).
    pub fn world_paths(&self) -> Vec<(PathData, FillRule, Style)> {
        match &self.geom {
            Geom::Path { path, rule } => vec![(path.transformed(self.transform), *rule, self.style.clone())],
            Geom::Point => vec![],
            Geom::Art { node, .. } => {
                let mut out = vec![];
                art_paths(node, self.transform, &self.style, &mut out);
                out
            }
        }
    }
    /// The item as plain path items in graph space (art is broken into its paths).
    pub fn to_path_items(&self) -> Vec<Item> {
        match &self.geom {
            Geom::Path { path, rule } => {
                vec![Item { geom: Geom::Path { path: path.transformed(self.transform), rule: *rule }, transform: Affine::IDENTITY, ..self.clone() }]
            }
            Geom::Point => vec![self.clone()],
            Geom::Art { .. } => self
                .world_paths()
                .into_iter()
                .map(|(path, rule, style)| Item { geom: Geom::Path { path, rule }, transform: Affine::IDENTITY, style, attrs: self.attrs })
                .collect(),
        }
    }
}

/// The paths in `n` (transformed by `a`), painted with the leaf's own fill and stroke unless
/// `over` sets them.
fn art_paths(n: &Node, a: Affine, over: &Style, out: &mut Vec<(PathData, FillRule, Style)>) {
    if !n.visible {
        return;
    }
    let own = || {
        let ap = &n.appearance;
        let fill = over.fill.clone().unwrap_or_else(|| ap.fill_paint());
        let stroke = over.stroke.clone().unwrap_or_else(|| ap.stroke_paint());
        let width = over.stroke_width.unwrap_or_else(|| ap.stroke_width().max(0.0));
        Style { fill: Some(fill), stroke: Some(stroke), stroke_width: Some(width), opacity: over.opacity * f64::from(n.opacity) }
    };
    match &n.kind {
        NodeKind::Path { path, rule, guide: false, .. } => out.push((path.transformed(a), *rule, own())),
        NodeKind::Compound { children, rule } => {
            let mut pd = PathData::default();
            for c in children {
                if let Some(p) = c.path_data() {
                    pd.subpaths.extend(p.transformed(a).subpaths);
                }
            }
            out.push((pd, *rule, own()));
        }
        _ => {
            for c in n.children().into_iter().flatten() {
                art_paths(c, a, over, out);
            }
        }
    }
}

/// An item list being built, with the caps every node output obeys: at most [`MAX_ITEMS`]
/// items, [`MAX_WEIGHT`] document nodes and [`MAX_TOTAL_ANCHORS`] anchors.
#[derive(Debug, Default)]
pub struct Out {
    pub items: Vec<Item>,
    weight: usize,
    anchors: usize,
    /// Something was left out to stay within the caps.
    pub truncated: bool,
}

impl Out {
    /// Add `it` if it fits (paths with too many anchors, or with non-finite coordinates, are
    /// dropped). Returns false once the list is full.
    pub fn push(&mut self, it: Item) -> bool {
        if self.is_full() {
            self.truncated = true;
            return false;
        }
        if !math::affine_finite(&it.transform) {
            return true;
        }
        if let Geom::Path { path, .. } = &it.geom {
            if path.anchor_count() > MAX_ANCHORS_PER_PATH {
                self.truncated = true;
                return true;
            }
            if !path
                .subpaths
                .iter()
                .flat_map(|s| &s.anchors)
                .all(|a| math::point_finite(a.p) && math::point_finite(a.h_in) && math::point_finite(a.h_out))
            {
                return true;
            }
        }
        let (w, a) = (it.weight(), it.anchors());
        if self.weight + w > MAX_WEIGHT || self.anchors + a > MAX_TOTAL_ANCHORS {
            self.truncated = true;
            return false;
        }
        self.weight += w;
        self.anchors += a;
        self.items.push(it);
        true
    }
    pub fn is_full(&self) -> bool {
        self.items.len() >= MAX_ITEMS || self.weight >= MAX_WEIGHT || self.anchors >= MAX_TOTAL_ANCHORS
    }
    /// Anchors still allowed in this list.
    pub fn anchor_budget(&self) -> usize {
        MAX_TOTAL_ANCHORS.saturating_sub(self.anchors)
    }
}

/// Bounds of a whole list.
pub fn list_bounds(items: &[Item]) -> Option<Rect> {
    items.iter().filter_map(Item::bounds).reduce(|a, b| a.union(b))
}
