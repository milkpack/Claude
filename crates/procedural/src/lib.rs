//! VectorCraft procedural generation (Graphite-style node graphs).
//!
//! A procedural object is a group whose children are generated from a
//! [`ProcGraph`](vectorcraft_doc::ProcGraph) (`Node::procedural`). This crate evaluates such graphs:
//!
//! - [`catalogue`]: every node kind (stable id, ports, params with defaults and ranges) and its
//!   implementation; [`catalogue::catalogue_json`] is what a generic node-graph UI reads.
//! - [`eval`]: validation, topological order with cycle detection, per-node errors (a failing
//!   node yields an error for that node and nothing downstream, never a panic) and a [`Cache`].
//! - [`build`]: the resulting items → document nodes.
//! - [`presets`]: ready-made graphs.
//!
//! Wires carry lists of [`Item`]s (geometry + style + transform + attributes). Evaluation is
//! deterministic: the same graph and seed give the same items, bit for bit, on every platform
//! (own SplitMix64 PRNG and gradient noise, trigonometry from `libm`, no hash-map order in
//! results). Every size an untrusted graph asks for is capped ([`limits`]).
#![forbid(unsafe_code)]

pub mod build;
pub mod catalogue;
pub mod eval;
pub mod geo;
pub mod item;
pub mod limits;
pub mod math;
pub mod nodes;
pub mod params;
pub mod presets;

pub use build::build_nodes;
pub use catalogue::{CATALOGUE, NodeSpec, catalogue_json, spec};
pub use eval::{Cache, Diagnostic, Evaluation, Level, evaluate, evaluate_with};
pub use item::{Attrs, Geom, Item, Style};
pub use presets::{PRESETS, preset, presets_json};

use std::sync::Arc;

use kurbo::Affine;
use vectorcraft_doc::procedural::{Link, ProcGraph, ProcNode};
use vectorcraft_doc::{Node, PROC_GRAPH_VERSION};

/// Make an untrusted graph safe to store: at most [`limits::MAX_NODES`] nodes with distinct ids,
/// finite positions and transform, short names, at most [`limits::MAX_PORTS`] inputs per node.
/// Kinds, params and links are left as they are: the evaluator reports what is wrong with them.
pub fn sanitize(g: &mut ProcGraph) {
    if g.version == 0 {
        g.version = PROC_GRAPH_VERSION;
    }
    g.nodes.truncate(limits::MAX_NODES);
    let mut seen = std::collections::BTreeSet::new();
    g.nodes.retain(|n| seen.insert(n.id));
    for n in &mut g.nodes {
        if !n.position.iter().all(|c| c.is_finite()) {
            n.position = [0.0, 0.0];
        }
        n.position = n.position.map(|c| c.clamp(-1.0e6, 1.0e6));
        n.inputs.truncate(limits::MAX_PORTS);
        if n.kind.chars().count() > limits::MAX_NAME {
            n.kind = n.kind.chars().take(limits::MAX_NAME).collect();
        }
        if let Some(name) = &mut n.name
            && name.chars().count() > limits::MAX_NAME
        {
            *name = name.chars().take(limits::MAX_NAME).collect();
        }
        if n.kind != "source.art" {
            n.art.clear();
        }
    }
    if !math::affine_finite(&g.transform) {
        g.transform = Affine::IDENTITY;
    }
}

/// Would a wire from `from` into `to` close a cycle (does `from` already depend on `to`)?
pub fn would_cycle(g: &ProcGraph, from: u32, to: u32) -> bool {
    if from == to {
        return true;
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut stack = vec![from];
    while let Some(id) = stack.pop() {
        if id == to {
            return true;
        }
        if !seen.insert(id) {
            continue;
        }
        if let Some(n) = g.node(id) {
            stack.extend(n.inputs.iter().flatten().map(|l| l.node));
        }
    }
    false
}

/// A graph that shows `art` as it is: `source.art` → `output` (Object › Procedural › Make from
/// a selection).
pub fn art_graph(art: Vec<Arc<Node>>) -> ProcGraph {
    let src = ProcNode { id: 1, kind: "source.art".into(), position: [0.0, 0.0], art, ..Default::default() };
    let out = ProcNode { id: 2, kind: "output".into(), inputs: vec![Some(Link::new(1))], position: [220.0, 0.0], ..Default::default() };
    ProcGraph { nodes: vec![src, out], output: Some(Link::new(2)), seed: 1, ..ProcGraph::default() }
}

/// A new node of `kind` with its params checked against the catalogue (bad ones are errors).
pub fn new_node(g: &ProcGraph, kind: &str, params: &serde_json::Map<String, serde_json::Value>) -> Result<ProcNode, String> {
    let spec = catalogue::spec(kind).ok_or_else(|| format!("unknown node kind `{}`", kind.chars().take(60).collect::<String>()))?;
    let mut checked = serde_json::Map::new();
    for (k, v) in params {
        let p = spec
            .params
            .iter()
            .find(|p| p.name == k)
            .ok_or_else(|| format!("`{kind}` has no parameter `{}`", k.chars().take(60).collect::<String>()))?;
        checked.insert(k.clone(), p.check(v)?);
    }
    let id = g.next_node_id().ok_or("the graph has no free node ids")?;
    Ok(ProcNode { id, kind: kind.into(), params: checked, inputs: vec![], ..Default::default() })
}
