//! Graph evaluation: validation, topological order with cycle detection, per-node errors and a
//! cache of unchanged subgraphs.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use serde::Serialize;
use vectorcraft_doc::ProcGraph;
use vectorcraft_doc::procedural::{Link, ProcNode};

use crate::catalogue::{self, Ctx, Inputs, NodeSpec};
use crate::item::{Item, Out};
use crate::limits::{MAX_ITEMS, MAX_NODES};
use crate::math;
use crate::params::{self, Reader};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// The node produced nothing.
    Error,
    /// The node ran, but something was ignored or clamped.
    Warning,
}

/// A message about one node (`node: None`: about the whole graph).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Diagnostic {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<u32>,
    pub level: Level,
    pub message: String,
}

impl Diagnostic {
    fn error(node: Option<u32>, message: impl Into<String>) -> Self {
        Self { node, level: Level::Error, message: message.into() }
    }
    fn warning(node: Option<u32>, message: impl Into<String>) -> Self {
        Self { node, level: Level::Warning, message: message.into() }
    }
}

/// The result of evaluating a graph.
#[derive(Clone, Debug)]
pub struct Evaluation {
    /// The output node's items (empty when there is no output or it failed).
    pub items: Arc<Vec<Item>>,
    /// Errors and warnings, graph-level first, then by node in graph order.
    pub diagnostics: Vec<Diagnostic>,
    /// Nodes actually run (the rest came from the cache or weren't needed).
    pub evaluated: usize,
}

impl Evaluation {
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| d.level == Level::Error)
    }
}

/// Outputs of nodes from earlier evaluations, keyed by everything they depend on. Lookups only:
/// what it holds never changes what an evaluation returns.
#[derive(Default)]
pub struct Cache {
    entries: HashMap<u32, Entry>,
}

struct Entry {
    key: (u64, u64),
    items: Arc<Vec<Item>>,
    diags: Vec<Diagnostic>,
}

impl Cache {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Two independent 64-bit hashes of `bytes` (FNV-1a and a SplitMix-based one).
fn hash_bytes(bytes: &[u8], seed: (u64, u64)) -> (u64, u64) {
    let mut a = 0xcbf2_9ce4_8422_2325u64 ^ seed.0;
    let mut b = seed.1;
    for chunk in bytes.chunks(8) {
        let mut w = [0u8; 8];
        for (d, s) in w.iter_mut().zip(chunk) {
            *d = *s;
        }
        for byte in chunk {
            a = (a ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        b = math::mix64(b ^ u64::from_le_bytes(w)).wrapping_add(chunk.len() as u64);
    }
    (a, math::mix64(b ^ bytes.len() as u64))
}

/// A node as the evaluator sees it once validated.
struct Prepared<'a> {
    node: &'a ProcNode,
    spec: Option<&'static NodeSpec>,
    /// Source node index per input port (`None`: nothing usable connected).
    deps: Vec<Option<usize>>,
    /// Unknown kind, bad params or part of a cycle: produces nothing.
    broken: bool,
}

/// Evaluate `g` from scratch.
pub fn evaluate(g: &ProcGraph) -> Evaluation {
    evaluate_with(g, &mut Cache::default())
}

/// Evaluate `g`, reusing (and refreshing) `cache`.
pub fn evaluate_with(g: &ProcGraph, cache: &mut Cache) -> Evaluation {
    let mut graph_diags = vec![];
    let mut node_diags: BTreeMap<usize, Vec<Diagnostic>> = BTreeMap::new();
    if g.nodes.len() > MAX_NODES {
        graph_diags.push(Diagnostic::error(None, format!("the graph has {} nodes; only the first {MAX_NODES} are used", g.nodes.len())));
    }
    // Index by id; later duplicates are ignored.
    let mut index: BTreeMap<u32, usize> = BTreeMap::new();
    let mut nodes: Vec<&ProcNode> = vec![];
    for n in g.nodes.iter().take(MAX_NODES) {
        if index.contains_key(&n.id) {
            graph_diags.push(Diagnostic::error(Some(n.id), format!("duplicate node id {} (ignored)", n.id)));
            continue;
        }
        index.insert(n.id, nodes.len());
        nodes.push(n);
    }
    let mut prep: Vec<Prepared> = Vec::with_capacity(nodes.len());
    for (i, n) in nodes.iter().enumerate() {
        let diags = node_diags.entry(i).or_default();
        let spec = catalogue::spec(&n.kind);
        let mut broken = false;
        match spec {
            None => {
                diags.push(Diagnostic::error(Some(n.id), format!("unknown node kind `{}`", n.kind.chars().take(60).collect::<String>())));
                broken = true;
            }
            Some(s) => {
                if let Err(e) = params::validate(s.params, &n.params) {
                    diags.push(Diagnostic::error(Some(n.id), e));
                    broken = true;
                }
            }
        }
        let ports = spec.map_or(n.inputs.len(), |s| s.port_count(n.inputs.len()));
        let mut deps = vec![None; ports];
        for (port, link) in n.inputs.iter().enumerate() {
            let Some(Link { node: src, port: out_port }) = *link else { continue };
            let Some(slot) = deps.get_mut(port) else {
                diags.push(Diagnostic::warning(Some(n.id), format!("input {port} is not a port of this node (ignored)")));
                continue;
            };
            match index.get(&src) {
                None => diags.push(Diagnostic::warning(Some(n.id), format!("input {port}: there is no node {src} (treated as not connected)"))),
                Some(_) if out_port != 0 => diags.push(Diagnostic::warning(Some(n.id), format!("input {port}: node {src} has no output {out_port}"))),
                Some(&j) => *slot = Some(j),
            }
        }
        prep.push(Prepared { node: n, spec, deps, broken });
    }
    let order = topo_order(&mut prep, &mut node_diags);

    // The output, and the nodes it needs.
    let out_idx = match g.output_link() {
        None => {
            graph_diags.push(Diagnostic::warning(None, "no output: add an `output` node or set the graph's output"));
            None
        }
        Some(l) => match index.get(&l.node) {
            None => {
                graph_diags.push(Diagnostic::error(None, format!("the output refers to node {}, which doesn't exist", l.node)));
                None
            }
            Some(_) if l.port != 0 => {
                graph_diags.push(Diagnostic::error(None, format!("node {} has no output {}", l.node, l.port)));
                None
            }
            Some(&i) => Some(i),
        },
    };
    let mut needed = vec![false; prep.len()];
    if let Some(o) = out_idx {
        let mut stack = vec![o];
        while let Some(i) = stack.pop() {
            if let Some(slot) = needed.get_mut(i)
                && !*slot
            {
                *slot = true;
                stack.extend(prep.get(i).into_iter().flat_map(|p| p.deps.iter().flatten().copied()));
            }
        }
    }

    // Run the needed nodes in order.
    let empty: Arc<Vec<Item>> = Arc::new(vec![]);
    let mut outputs: Vec<Arc<Vec<Item>>> = vec![empty.clone(); prep.len()];
    let mut keys: Vec<(u64, u64)> = vec![(0, 0); prep.len()];
    let mut evaluated = 0;
    for &i in &order {
        if !needed.get(i).copied().unwrap_or(false) {
            continue;
        }
        let Some(p) = prep.get(i) else { continue };
        let inputs: Vec<Arc<Vec<Item>>> = p.deps.iter().map(|d| d.and_then(|j| outputs.get(j).cloned()).unwrap_or_else(|| empty.clone())).collect();
        let input_keys: Vec<(u64, u64)> = p.deps.iter().map(|d| d.and_then(|j| keys.get(j).copied()).unwrap_or((0, 0))).collect();
        let key = node_key(g.seed, p, &input_keys);
        if let Some(k) = keys.get_mut(i) {
            *k = key;
        }
        if p.broken {
            continue;
        }
        let Some(spec) = p.spec else { continue };
        let (items, diags) = match cache.entries.get(&p.node.id) {
            Some(e) if e.key == key => (e.items.clone(), e.diags.clone()),
            _ => {
                evaluated += 1;
                let (items, diags) = run(g.seed, p.node, spec, inputs);
                cache.entries.insert(p.node.id, Entry { key, items: items.clone(), diags: diags.clone() });
                (items, diags)
            }
        };
        node_diags.entry(i).or_default().extend(diags);
        if let Some(o) = outputs.get_mut(i) {
            *o = items;
        }
    }
    // Forget nodes that left the graph.
    cache.entries.retain(|id, _| index.contains_key(id));

    let items = out_idx.and_then(|o| outputs.get(o).cloned()).unwrap_or(empty);
    let mut diagnostics = graph_diags;
    diagnostics.extend(node_diags.into_values().flatten());
    Evaluation { items, diagnostics, evaluated }
}

/// Everything a node's output depends on, hashed.
fn node_key(seed: u64, p: &Prepared, inputs: &[(u64, u64)]) -> (u64, u64) {
    let n = p.node;
    let body = serde_json::to_vec(&(&n.kind, &n.params, n.bypass, &n.art, n.id, seed, inputs)).unwrap_or_default();
    hash_bytes(&body, (seed, u64::from(n.id)))
}

/// Run one node.
fn run(graph_seed: u64, node: &ProcNode, spec: &'static NodeSpec, inputs: Vec<Arc<Vec<Item>>>) -> (Arc<Vec<Item>>, Vec<Diagnostic>) {
    let reader = Reader { specs: spec.params, map: &node.params };
    let own_seed = u64::try_from(reader.int("seed")).unwrap_or(0);
    let cx = Ctx { seed: math::hash(&[graph_seed, u64::from(node.id), own_seed]), node, p: reader };
    let inputs = Inputs { lists: inputs };
    let mut out = Out::default();
    let mut diags = vec![];
    let r = if node.bypass {
        catalogue::spec("output").map_or(Ok(()), |o| (o.eval)(&cx, &inputs, &mut out))
    } else {
        (spec.eval)(&cx, &inputs, &mut out)
    };
    match r {
        Err(e) => {
            diags.push(Diagnostic::error(Some(node.id), e));
            return (Arc::new(vec![]), diags);
        }
        Ok(()) => {
            if out.truncated {
                diags.push(Diagnostic::warning(
                    Some(node.id),
                    format!("output limited to {} items (at most {MAX_ITEMS} items, and a cap on anchors and objects)", out.items.len()),
                ));
            }
        }
    }
    (Arc::new(out.items), diags)
}

/// Kahn's order over all nodes. Nodes on a cycle are marked broken (with an error) and treated
/// as producing nothing, so the nodes after them still run.
fn topo_order(prep: &mut [Prepared], diags: &mut BTreeMap<usize, Vec<Diagnostic>>) -> Vec<usize> {
    let n = prep.len();
    let mut users: Vec<Vec<usize>> = vec![vec![]; n];
    let mut pending = vec![0usize; n];
    for (i, p) in prep.iter().enumerate() {
        for &j in p.deps.iter().flatten() {
            if let Some(u) = users.get_mut(j) {
                u.push(i);
            }
            if let Some(c) = pending.get_mut(i) {
                *c += 1;
            }
        }
    }
    let mut done = vec![false; n];
    let mut order = Vec::with_capacity(n);
    let mut queue: VecDeque<usize> = (0..n).filter(|i| pending.get(*i) == Some(&0)).collect();
    loop {
        while let Some(i) = queue.pop_front() {
            if done.get(i).copied().unwrap_or(true) {
                continue;
            }
            if let Some(d) = done.get_mut(i) {
                *d = true;
            }
            order.push(i);
            for &u in users.get(i).map(Vec::as_slice).unwrap_or(&[]) {
                if let Some(c) = pending.get_mut(u) {
                    *c = c.saturating_sub(1);
                    if *c == 0 {
                        queue.push_back(u);
                    }
                }
            }
        }
        let left: Vec<usize> = (0..n).filter(|i| !done.get(*i).copied().unwrap_or(true)).collect();
        if left.is_empty() {
            break;
        }
        // Stuck: some of what is left lies on a cycle. Break every node that reaches itself.
        let on_cycle: Vec<usize> = left.iter().copied().filter(|&v| reaches_itself(prep, &done, v)).collect();
        if on_cycle.is_empty() {
            // Can't happen (a stuck set always holds a cycle); bail out rather than loop.
            break;
        }
        for &v in &on_cycle {
            if let Some(p) = prep.get_mut(v) {
                p.broken = true;
                diags.entry(v).or_default().push(Diagnostic::error(Some(p.node.id), "this node is part of a cycle"));
            }
            if let Some(c) = pending.get_mut(v) {
                *c = 0;
            }
            queue.push_back(v);
        }
    }
    order
}

/// Does `v` reach itself through the dependencies of nodes not yet done?
fn reaches_itself(prep: &[Prepared], done: &[bool], v: usize) -> bool {
    let mut seen = vec![false; prep.len()];
    let mut stack: Vec<usize> = prep.get(v).into_iter().flat_map(|p| p.deps.iter().flatten().copied()).collect();
    while let Some(i) = stack.pop() {
        if i == v {
            return true;
        }
        if done.get(i).copied().unwrap_or(true) || seen.get(i).copied().unwrap_or(true) {
            continue;
        }
        if let Some(s) = seen.get_mut(i) {
            *s = true;
        }
        stack.extend(prep.get(i).into_iter().flat_map(|p| p.deps.iter().flatten().copied()));
    }
    false
}
