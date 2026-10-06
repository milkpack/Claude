//! Ready-made graphs (`procedural.presets`, `procedural.create {preset}`). Ids are stable API.

use serde_json::{Map, Value, json};
use vectorcraft_doc::procedural::{Link, ProcGraph, ProcNode};

/// One preset as listed.
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub doc: &'static str,
    make: fn(&mut B),
}

/// The presets, in menu order.
pub static PRESETS: &[Preset] = &[
    Preset { id: "flower", label: "Radial Flower", doc: "two rings of petals round a heart, coloured round the wheel", make: flower },
    Preset { id: "dots", label: "Scattered Dots", doc: "random dots of random sizes, coloured from left to right", make: dots },
    Preset { id: "noiseWaves", label: "Noise Waves", doc: "stacked lines displaced by fractal noise", make: noise_waves },
    Preset {
        id: "jitterSquares",
        label: "Jittered Squares",
        doc: "a grid of squares turned at random, in a five-colour palette",
        make: jitter_squares,
    },
    Preset { id: "spiralCircles", label: "Spiral of Circles", doc: "circles turning and shrinking into a spiral", make: spiral_circles },
    Preset { id: "starburst", label: "Starburst", doc: "long and short rays round a star", make: starburst },
    Preset { id: "confetti", label: "Poisson Confetti", doc: "evenly spread confetti pieces, turned and coloured at random", make: confetti },
    Preset { id: "booleanPattern", label: "Punched Plate", doc: "a plate with a grid of round holes (a boolean subtract)", make: boolean_pattern },
    Preset { id: "hexTiles", label: "Hex Tiles", doc: "a honeycomb of hexagons shaded from the centre out", make: hex_tiles },
];

/// A graph builder: nodes laid out in columns for the node-graph panel.
pub(crate) struct B {
    g: ProcGraph,
}

impl B {
    fn node(&mut self, kind: &str, params: Value, inputs: &[u32], col: u32, row: u32) -> u32 {
        let id = self.g.nodes.len() as u32 + 1;
        let params: Map<String, Value> = params.as_object().cloned().unwrap_or_default();
        self.g.nodes.push(ProcNode {
            id,
            kind: kind.into(),
            params,
            inputs: inputs.iter().map(|i| Some(Link::new(*i))).collect(),
            position: [col as f32 * 220.0, row as f32 * 130.0],
            ..Default::default()
        });
        id
    }
    fn output(&mut self, from: u32, col: u32) {
        let o = self.node("output", json!({}), &[from], col, 0);
        self.g.output = Some(Link::new(o));
    }
}

/// The preset `id`'s graph.
pub fn preset(id: &str) -> Option<ProcGraph> {
    let p = PRESETS.iter().find(|p| p.id == id)?;
    let mut b = B { g: ProcGraph { seed: 1, ..ProcGraph::default() } };
    (p.make)(&mut b);
    Some(b.g)
}

/// The list as JSON (`procedural.presets`).
pub fn presets_json() -> Value {
    json!(PRESETS.iter().map(|p| json!({"id": p.id, "label": p.label, "doc": p.doc})).collect::<Vec<_>>())
}

fn flower(b: &mut B) {
    let petal = b.node("generate.ellipse", json!({"width": 28, "height": 90}), &[], 0, 0);
    let ring = b.node("instance.radial", json!({"count": 12, "radius": 58}), &[petal], 1, 0);
    let ramp = b.node("style.colorRamp", json!({"from": "#ff5d8f", "to": "#ffb347", "space": "hsl"}), &[ring], 2, 0);
    let small = b.node("generate.ellipse", json!({"width": 18, "height": 52}), &[], 0, 1);
    let ring2 = b.node("instance.radial", json!({"count": 12, "radius": 34, "startAngle": 15}), &[small], 1, 1);
    let fill2 = b.node("style.fill", json!({"color": "#ffd166"}), &[ring2], 2, 1);
    let heart = b.node("generate.ellipse", json!({"width": 34, "height": 34}), &[], 0, 2);
    let fill3 = b.node("style.fill", json!({"color": "#6a4c93"}), &[heart], 2, 2);
    let all = b.node("modify.merge", json!({}), &[ramp, fill2, fill3], 3, 0);
    let st = b.node("style.stroke", json!({"color": "none"}), &[all], 4, 0);
    b.output(st, 5);
}

fn dots(b: &mut B) {
    let pts = b.node("points.scatter", json!({"count": 160, "width": 420, "height": 260}), &[], 0, 0);
    let dot = b.node("generate.ellipse", json!({"width": 14, "height": 14}), &[], 0, 1);
    let copies = b.node("instance.copyToPoints", json!({}), &[pts, dot], 1, 0);
    let jit = b.node("modify.jitter", json!({"scale": 0.6}), &[copies], 2, 0);
    let ramp = b.node("style.colorRamp", json!({"from": "#00c9a7", "to": "#845ec2", "by": "x", "space": "hsl"}), &[jit], 3, 0);
    let st = b.node("style.stroke", json!({"color": "none"}), &[ramp], 4, 0);
    b.output(st, 5);
}

fn noise_waves(b: &mut B) {
    let line = b.node("generate.line", json!({"x1": -210, "y1": 0, "x2": 210, "y2": 0}), &[], 0, 0);
    let st = b.node("style.stroke", json!({"color": "#0081cf", "width": 1.5}), &[line], 1, 0);
    let rows = b.node("instance.linear", json!({"count": 24, "offsetX": 0, "offsetY": 10}), &[st], 2, 0);
    let mv = b.node("modify.transform", json!({"y": -115}), &[rows], 3, 0);
    let nz = b.node("modify.noise", json!({"amplitude": 14, "frequency": 0.012, "octaves": 3, "detail": 4}), &[mv], 4, 0);
    let ramp = b.node("style.colorRamp", json!({"from": "#0081cf", "to": "#00c9a7", "target": "stroke"}), &[nz], 5, 0);
    b.output(ramp, 6);
}

fn jitter_squares(b: &mut B) {
    let sq = b.node("generate.rectangle", json!({"width": 22, "height": 22, "radius": 3}), &[], 0, 0);
    let grid = b.node("instance.grid", json!({"columns": 10, "rows": 10, "spacingX": 34, "spacingY": 34}), &[sq], 1, 0);
    let jit = b.node("modify.jitter", json!({"rotate": 45, "move": 3}), &[grid], 2, 0);
    let col = b.node("style.randomColor", json!({}), &[jit], 3, 0);
    let st = b.node("style.stroke", json!({"color": "none"}), &[col], 4, 0);
    b.output(st, 5);
}

fn spiral_circles(b: &mut B) {
    let c = b.node("generate.ellipse", json!({"width": 26, "height": 26}), &[], 0, 0);
    let mv = b.node("modify.transform", json!({"x": 160}), &[c], 1, 0);
    let rep =
        b.node("instance.linear", json!({"count": 72, "offsetX": 0, "offsetY": 0, "rotate": 20, "scale": 0.965, "pivot": "origin"}), &[mv], 2, 0);
    let ramp = b.node("style.colorRamp", json!({"from": "#f9f871", "to": "#d65db1", "space": "hsl"}), &[rep], 3, 0);
    let st = b.node("style.stroke", json!({"color": "none"}), &[ramp], 4, 0);
    b.output(st, 5);
}

fn starburst(b: &mut B) {
    let ray = b.node("generate.line", json!({"x1": 40, "y1": 0, "x2": 170, "y2": 0}), &[], 0, 0);
    let st = b.node("style.stroke", json!({"color": "#ff006e", "width": 3}), &[ray], 1, 0);
    let rays = b.node("instance.radial", json!({"count": 48, "radius": 0}), &[st], 2, 0);
    let long = b.node("modify.select", json!({"mode": "everyNth", "n": 2, "offset": 0}), &[rays], 3, 0);
    let short = b.node("modify.select", json!({"mode": "everyNth", "n": 2, "offset": 1}), &[rays], 3, 1);
    let shorter = b.node("modify.transform", json!({"scaleX": 0.65, "scaleY": 0.65, "pivot": "origin"}), &[short], 4, 1);
    let both = b.node("modify.merge", json!({}), &[long, shorter], 5, 0);
    let ramp = b.node("style.colorRamp", json!({"from": "#ff006e", "to": "#ffbe0b", "target": "stroke", "space": "hsl"}), &[both], 6, 0);
    let star = b.node("generate.star", json!({"points": 12, "radius": 30, "innerRatio": 0.55}), &[], 5, 2);
    let sf = b.node("style.fill", json!({"color": "#ffbe0b"}), &[star], 6, 2);
    let sn = b.node("style.stroke", json!({"color": "none"}), &[sf], 7, 2);
    let all = b.node("modify.merge", json!({}), &[ramp, sn], 8, 0);
    b.output(all, 9);
}

fn confetti(b: &mut B) {
    let pts = b.node("points.poisson", json!({"distance": 22, "width": 420, "height": 280, "maxCount": 400}), &[], 0, 0);
    let piece = b.node("generate.rectangle", json!({"width": 6, "height": 14, "radius": 1}), &[], 0, 1);
    let copies = b.node("instance.copyToPoints", json!({}), &[pts, piece], 1, 0);
    let jit = b.node("modify.jitter", json!({"rotate": 180, "scale": 0.3}), &[copies], 2, 0);
    let col = b.node("style.randomColor", json!({"palette": ["#ff595e", "#ffca3a", "#8ac926", "#1982c4", "#6a4c93"]}), &[jit], 3, 0);
    let st = b.node("style.stroke", json!({"color": "none"}), &[col], 4, 0);
    b.output(st, 5);
}

fn boolean_pattern(b: &mut B) {
    let plate = b.node("generate.rectangle", json!({"width": 360, "height": 240, "radius": 16}), &[], 0, 0);
    let grid = b.node("points.grid", json!({"columns": 9, "rows": 6, "spacingX": 38, "spacingY": 38}), &[], 0, 1);
    let hole = b.node("generate.ellipse", json!({"width": 22, "height": 22}), &[], 0, 2);
    let holes = b.node("instance.copyToPoints", json!({}), &[grid, hole], 1, 1);
    let cut = b.node("modify.boolean", json!({"operation": "subtract"}), &[plate, holes], 2, 0);
    let fill = b.node("style.fill", json!({"color": "#264653"}), &[cut], 3, 0);
    let st = b.node("style.stroke", json!({"color": "none"}), &[fill], 4, 0);
    b.output(st, 5);
}

fn hex_tiles(b: &mut B) {
    let hex = b.node("generate.polygon", json!({"sides": 6, "radius": 17}), &[], 0, 0);
    let grid = b.node("instance.grid", json!({"columns": 12, "rows": 9, "spacingX": 31.2, "spacingY": 27, "stagger": true}), &[hex], 1, 0);
    let ramp = b.node("style.colorRamp", json!({"from": "#ffe066", "to": "#247ba0", "by": "radius"}), &[grid], 2, 0);
    let st = b.node("style.stroke", json!({"color": "#ffffff", "width": 1.5}), &[ramp], 3, 0);
    b.output(st, 4);
}
