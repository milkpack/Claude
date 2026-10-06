//! Property tests: random graphs (random kinds, params, links, garbage JSON) never panic, stay
//! within the caps, give finite geometry and evaluate the same way twice.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kurbo::Affine;
use proptest::prelude::*;
use serde_json::{Map, Value, json};
use vectorcraft_doc::NodeId;
use vectorcraft_doc::procedural::{Link, ProcGraph, ProcNode};
use vectorcraft_procedural::limits::{MAX_ITEMS, MAX_TOTAL_ANCHORS};
use vectorcraft_procedural::params::ParamKind;
use vectorcraft_procedural::{CATALOGUE, Cache, Geom, build_nodes, evaluate, evaluate_with, sanitize};

fn value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-1.0e6f64..1.0e6).prop_map(|x| json!(x)),
        prop_oneof![Just(0.0), Just(-1.0), Just(1e308), Just(-1e308), Just(f64::MIN_POSITIVE), Just(1e9), Just(0.5)].prop_map(|x| json!(x)),
        any::<i64>().prop_map(|x| json!(x)),
        any::<u64>().prop_map(|x| json!(x)),
        prop_oneof![Just("none"), Just("#ff0000"), Just("#abc"), Just(""), Just("x"), Just("rgb"), Just("hsl"), Just("origin"), Just("divide")]
            .prop_map(|s| json!(s)),
        ".{0,12}".prop_map(Value::String),
    ];
    leaf.prop_recursive(2, 8, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map(".{0,4}", inner, 0..3).prop_map(|m| json!(m))
        ]
    })
}

/// Param maps: mostly real names (so nodes run), with some garbage.
fn params(kind_idx: usize) -> impl Strategy<Value = Map<String, Value>> {
    let names: Vec<String> =
        CATALOGUE.get(kind_idx % CATALOGUE.len()).map(|s| s.params.iter().map(|p| p.name.to_string()).collect()).unwrap_or_default();
    let name = if names.is_empty() { Just("seed".to_string()).boxed() } else { prop::sample::select(names).boxed() };
    prop::collection::vec((prop_oneof![9 => name, 1 => ".{0,6}"], value()), 0..4).prop_map(|kv| kv.into_iter().collect())
}

fn node() -> impl Strategy<Value = (usize, bool, Map<String, Value>, Vec<Option<u32>>, bool)> {
    (0usize..CATALOGUE.len() + 3).prop_flat_map(|k| {
        (Just(k), prop::bool::weighted(0.05), params(k), prop::collection::vec(prop::option::of(0u32..14), 0..4), prop::bool::weighted(0.05))
    })
}

fn graph() -> impl Strategy<Value = ProcGraph> {
    (prop::collection::vec(node(), 0..12), any::<u64>(), prop::option::of(0u32..14)).prop_map(|(nodes, seed, out)| {
        let nodes = nodes
            .into_iter()
            .enumerate()
            .map(|(i, (k, garbage, params, inputs, bypass))| ProcNode {
                id: i as u32 + 1,
                kind: match CATALOGUE.get(k) {
                    Some(s) if !garbage => s.kind.to_string(),
                    _ => format!("junk.{k}"),
                },
                params,
                inputs: inputs.into_iter().map(|l| l.map(Link::new)).collect(),
                bypass,
                ..Default::default()
            })
            .collect();
        ProcGraph { nodes, output: out.map(Link::new), seed, ..ProcGraph::default() }
    })
}

/// A valid DAG: real kinds, in-range params picked from the catalogue, links to earlier nodes,
/// the last node as the output. These graphs really run, so they exercise every node's maths.
fn valid_graph() -> impl Strategy<Value = ProcGraph> {
    let node = (0usize..CATALOGUE.len(), prop::collection::vec(0.0f64..1.0, 12), prop::collection::vec(any::<u32>(), 3), prop::bool::weighted(0.3));
    (prop::collection::vec(node, 1..9), any::<u64>()).prop_map(|(nodes, seed)| {
        let n = nodes.len();
        let nodes = nodes
            .into_iter()
            .enumerate()
            .map(|(i, (k, r, links, sparse))| {
                let spec = &CATALOGUE[k];
                let mut params = Map::new();
                for (j, p) in spec.params.iter().enumerate() {
                    let x = r.get(j).copied().unwrap_or(0.5);
                    if sparse && x < 0.5 {
                        continue;
                    }
                    let v = match p.kind {
                        // Mostly modest values, sometimes the extremes.
                        ParamKind::Number { min, max, .. } => {
                            let (lo, hi) = (min.max(-500.0), max.min(500.0));
                            json!(if x < 0.05 {
                                min
                            } else if x > 0.95 {
                                max
                            } else {
                                lo + (hi - lo) * x
                            })
                        }
                        ParamKind::Int { min, max, .. } => {
                            let hi = max.min(min + 40);
                            json!(if x > 0.97 { max } else { min + ((hi - min) as f64 * x) as i64 })
                        }
                        ParamKind::Bool { .. } => json!(x < 0.5),
                        ParamKind::Choice { options, .. } => json!(options[((x * options.len() as f64) as usize).min(options.len() - 1)]),
                        ParamKind::Color { .. } => json!(if x < 0.1 { "none".to_string() } else { format!("#{:06x}", (x * 16_777_215.0) as u32) }),
                        ParamKind::Palette { .. } => json!(["#102030", "#ffeedd"]),
                    };
                    params.insert(p.name.to_string(), v);
                }
                let ports = if spec.variadic { 3 } else { spec.inputs.len() };
                let inputs = (0..ports).map(|p| if i == 0 { None } else { links.get(p).map(|l| Link::new(l % i as u32 + 1)) }).collect();
                ProcNode { id: i as u32 + 1, kind: spec.kind.to_string(), params, inputs, ..Default::default() }
            })
            .collect();
        ProcGraph { nodes, output: Some(Link::new(n as u32)), seed, ..ProcGraph::default() }
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    #[test]
    fn valid_graphs_run_clean(g in valid_graph()) {
        let e = evaluate(&g);
        // Valid graphs only fail on the documented caps of the expensive nodes.
        for d in e.errors() {
            prop_assert!(d.message.contains("too much geometry") || d.message.contains("too many shapes") || d.message.contains("too degenerate"), "{:?}", d);
        }
        prop_assert!(e.items.len() <= MAX_ITEMS);
        for it in e.items.iter() {
            if let Geom::Path { path, .. } = &it.geom {
                prop_assert!(path.anchors().all(|(_, _, a)| a.p.x.is_finite() && a.p.y.is_finite() && a.h_in.x.is_finite() && a.h_out.y.is_finite()));
            }
        }
        prop_assert_eq!(&evaluate(&g).items, &e.items);
        let mut cache = Cache::new();
        prop_assert_eq!(&evaluate_with(&g, &mut cache).items, &e.items);
        // A different seed may differ, but never breaks anything.
        let mut h = g.clone();
        h.seed = h.seed.wrapping_add(1);
        let _ = evaluate_with(&h, &mut cache);
    }

    #[test]
    fn random_graphs_never_panic_and_stay_capped(g in graph()) {
        let mut g = g;
        sanitize(&mut g);
        let e = evaluate(&g);
        prop_assert!(e.items.len() <= MAX_ITEMS);
        prop_assert!(e.items.iter().map(|i| i.anchors()).sum::<usize>() <= MAX_TOTAL_ANCHORS);
        for it in e.items.iter() {
            prop_assert!(it.transform.as_coeffs().iter().all(|c| c.is_finite()));
            if let Geom::Path { path, .. } = &it.geom {
                prop_assert!(path.anchors().all(|(_, _, a)| a.p.x.is_finite() && a.p.y.is_finite()));
            }
        }
        // Deterministic, cached or not.
        let mut cache = Cache::new();
        let a = evaluate_with(&g, &mut cache);
        let b = evaluate_with(&g, &mut cache);
        prop_assert_eq!(&a.items, &e.items);
        prop_assert_eq!(&b.items, &e.items);
        prop_assert_eq!(&a.diagnostics, &e.diagnostics);
        // Documents nodes come out with unique ids.
        let mut next = 0u64;
        let mut alloc = || { next += 1; NodeId(next) };
        let nodes = build_nodes(&e.items, Affine::scale(2.0), &mut alloc);
        prop_assert_eq!(nodes.len(), e.items.len());
        // Every diagnostic names a node of the graph (or none).
        for d in &e.diagnostics {
            prop_assert!(d.node.is_none_or(|n| g.nodes.iter().any(|x| x.id == n)));
        }
    }

    /// Garbage JSON never deserializes into something that breaks the evaluator.
    #[test]
    fn garbage_json_graphs(v in value()) {
        if let Ok(mut g) = serde_json::from_value::<ProcGraph>(v) {
            sanitize(&mut g);
            let _ = evaluate(&g);
        }
    }
}
