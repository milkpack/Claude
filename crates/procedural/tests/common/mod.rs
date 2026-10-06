//! Graph-building helpers for the tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use kurbo::{Point, Rect};
use serde_json::Value;
use vectorcraft_doc::ProcGraph;
use vectorcraft_doc::procedural::{Link, ProcNode};
use vectorcraft_procedural::{Evaluation, Geom, Item, evaluate};

/// A graph under construction.
pub struct G {
    pub g: ProcGraph,
}

impl Default for G {
    fn default() -> Self {
        Self::new()
    }
}

impl G {
    pub fn new() -> Self {
        Self { g: ProcGraph { seed: 1, ..ProcGraph::default() } }
    }
    /// Add a node; returns its id.
    pub fn n(&mut self, kind: &str, params: Value, inputs: &[u32]) -> u32 {
        let id = self.g.nodes.len() as u32 + 1;
        self.g.nodes.push(ProcNode {
            id,
            kind: kind.into(),
            params: params.as_object().cloned().unwrap_or_default(),
            inputs: inputs.iter().map(|i| if *i == 0 { None } else { Some(Link::new(*i)) }).collect(),
            ..Default::default()
        });
        id
    }
    pub fn out(&mut self, id: u32) -> &mut Self {
        self.g.output = Some(Link::new(id));
        self
    }
    pub fn eval(&self) -> Evaluation {
        evaluate(&self.g)
    }
    /// Evaluate with `id` as the output; panics on any error.
    pub fn items(&mut self, id: u32) -> Vec<Item> {
        self.out(id);
        let e = self.eval();
        assert!(e.errors().next().is_none(), "errors: {:?}", e.diagnostics);
        e.items.to_vec()
    }
}

/// One node of `kind` fed by the given single-node graphs' outputs… (shortcut: a node on its own).
pub fn one(kind: &str, params: Value) -> Vec<Item> {
    let mut g = G::new();
    let id = g.n(kind, params, &[]);
    g.items(id)
}

pub fn bounds(items: &[Item]) -> Rect {
    vectorcraft_procedural::item::list_bounds(items).expect("bounds")
}

pub fn path_of(it: &Item) -> vectorcraft_geom::PathData {
    match &it.geom {
        Geom::Path { path, .. } => path.transformed(it.transform),
        _ => panic!("not a path: {it:?}"),
    }
}

pub fn origins(items: &[Item]) -> Vec<Point> {
    items.iter().map(Item::origin).collect()
}

pub fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

pub fn assert_rect(r: Rect, x0: f64, y0: f64, x1: f64, y1: f64, tol: f64) {
    assert!(close(r.x0, x0, tol) && close(r.y0, y0, tol) && close(r.x1, x1, tol) && close(r.y1, y1, tol), "{r:?} ≠ ({x0}, {y0}, {x1}, {y1})");
}

pub fn arc_node(art: Vec<Arc<vectorcraft_doc::Node>>) -> ProcNode {
    ProcNode { id: 1, kind: "source.art".into(), art, ..Default::default() }
}
