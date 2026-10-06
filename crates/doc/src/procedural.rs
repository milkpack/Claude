//! Procedural objects (Graphite-style node graphs): a group whose children are generated from a
//! [`ProcGraph`].
//!
//! The graph is pure data here; `vectorcraft-procedural` evaluates it and the engine's
//! `procedural.*` commands regenerate the group's children after every edit. Because the generated
//! children are ordinary nodes, rendering, export, hit testing and collaboration treat a procedural
//! object like any other group.
//!
//! Everything in a graph is untrusted (it comes from files, the clipboard, peers and command
//! params): the evaluator checks kinds, links and params and caps every size it reads.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use vectorcraft_geom::Affine;

use crate::Node;

/// The graph format version written by this build.
pub const PROC_GRAPH_VERSION: u32 = 1;

/// A wire end: output `port` of graph node `node`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Link {
    pub node: u32,
    /// Output port of `node` (every v1 node kind has one output, port 0).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub port: u32,
}

impl Link {
    pub fn new(node: u32) -> Self {
        Self { node, port: 0 }
    }
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// One node of a procedural graph.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProcNode {
    /// Stable within its graph (links refer to it).
    pub id: u32,
    /// Node kind id from the catalogue, e.g. `"generate.rectangle"`.
    pub kind: String,
    /// Parameter values by name; absent parameters take the catalogue default.
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
    /// Geometry inputs by port: `inputs[i]` feeds input port `i` (`None`: not connected).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Option<Link>>,
    /// Where the node-graph panel draws the node (graph-editor units, not document points).
    pub position: [f32; 2],
    /// A user-given name shown instead of the kind's label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Bypassed: the node passes its first input through unchanged.
    #[serde(skip_serializing_if = "is_false")]
    pub bypass: bool,
    /// `source.art`: the embedded art (copies of user objects, in graph space).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub art: Vec<Arc<Node>>,
}

/// A procedural object's graph (`Node::procedural`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProcGraph {
    /// Format version ([`PROC_GRAPH_VERSION`]).
    pub version: u32,
    pub nodes: Vec<ProcNode>,
    /// The node whose result becomes the group's children (`None`: the first `output` node).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Link>,
    /// Seed of every random node (each node also mixes in its own id and its `seed` param).
    pub seed: u64,
    /// Graph space → document: the moves, rotations and scales the object took since it was
    /// created, so regenerating keeps it where the user put it.
    #[serde(skip_serializing_if = "is_identity")]
    pub transform: Affine,
}

fn is_identity(a: &Affine) -> bool {
    *a == Affine::IDENTITY
}

impl Default for ProcGraph {
    fn default() -> Self {
        Self { version: PROC_GRAPH_VERSION, nodes: vec![], output: None, seed: 0, transform: Affine::IDENTITY }
    }
}

impl ProcGraph {
    pub fn node(&self, id: u32) -> Option<&ProcNode> {
        self.nodes.iter().find(|n| n.id == id)
    }
    pub fn node_mut(&mut self, id: u32) -> Option<&mut ProcNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }
    /// An id no node uses yet (one above the largest; `None` when the ids are exhausted).
    pub fn next_node_id(&self) -> Option<u32> {
        match self.nodes.iter().map(|n| n.id).max() {
            None => Some(1),
            Some(m) => m.checked_add(1),
        }
    }
    /// The node the result is taken from: [`ProcGraph::output`], else the first node of kind
    /// `output`.
    pub fn output_link(&self) -> Option<Link> {
        self.output.or_else(|| self.nodes.iter().find(|n| n.kind == "output").map(|n| Link::new(n.id)))
    }
    /// Apply a document transform to the procedural object (its regenerated art follows it).
    pub fn transform(&mut self, a: Affine) {
        self.transform = a * self.transform;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_round_trip() {
        let g: ProcGraph = serde_json::from_str("{}").unwrap();
        assert_eq!(g, ProcGraph::default());
        let mut g = ProcGraph { seed: 7, ..Default::default() };
        g.nodes.push(ProcNode { id: 1, kind: "generate.ellipse".into(), ..Default::default() });
        g.nodes.push(ProcNode { id: 2, kind: "output".into(), inputs: vec![Some(Link::new(1))], ..Default::default() });
        let s = serde_json::to_string(&g).unwrap();
        // Defaults stay out of the file.
        assert!(!s.contains("bypass") && !s.contains("port") && !s.contains("transform"), "{s}");
        let back: ProcGraph = serde_json::from_str(&s).unwrap();
        assert_eq!(back, g);
        assert_eq!(back.output_link(), Some(Link::new(2)));
        assert_eq!(back.next_node_id(), Some(3));
    }

    #[test]
    fn transform_composes() {
        let mut g = ProcGraph::default();
        g.transform(Affine::translate((10.0, 0.0)));
        g.transform(Affine::scale(2.0));
        assert_eq!(g.transform * vectorcraft_geom::Point::ZERO, vectorcraft_geom::Point::new(20.0, 0.0));
    }
}
