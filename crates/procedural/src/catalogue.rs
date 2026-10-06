//! The node catalogue: every node kind's id, ports and params (machine readable, so a node-graph
//! UI can be generic) and the function that evaluates it.
//!
//! Kind ids, port order and param names are stable API: files and scripts refer to them.

use std::sync::Arc;

use serde_json::{Value, json};
use vectorcraft_doc::ProcNode;

use crate::item::{Item, Out};
use crate::math::{self, Rng};
use crate::nodes;
use crate::params::{ParamSpec, Reader};

/// Evaluates one node: reads `cx` and the inputs, pushes its result into `out`.
pub type EvalFn = fn(&Ctx, &Inputs, &mut Out) -> Result<(), String>;

/// A geometry input port.
#[derive(Clone, Copy, Debug)]
pub struct PortSpec {
    pub name: &'static str,
    pub label: &'static str,
    pub doc: &'static str,
}

/// One node kind.
pub struct NodeSpec {
    pub kind: &'static str,
    pub label: &'static str,
    /// `generate`, `source`, `points`, `instance`, `modify`, `style` or `output`.
    pub category: &'static str,
    pub doc: &'static str,
    pub inputs: &'static [PortSpec],
    /// Takes any number of inputs (up to [`crate::limits::MAX_PORTS`]); `inputs` names the first.
    pub variadic: bool,
    pub params: &'static [ParamSpec],
    pub eval: EvalFn,
}

impl NodeSpec {
    /// How many input ports a node of this kind with `connected` input slots has.
    pub fn port_count(&self, connected: usize) -> usize {
        if self.variadic { connected.max(self.inputs.len()).min(crate::limits::MAX_PORTS) } else { self.inputs.len() }
    }
    pub fn to_json(&self) -> Value {
        json!({
            "kind": self.kind,
            "label": self.label,
            "category": self.category,
            "doc": self.doc,
            "inputs": self.inputs.iter().map(|p| json!({"name": p.name, "label": p.label, "type": "geometry", "doc": p.doc})).collect::<Vec<_>>(),
            "variadic": self.variadic,
            "outputs": [{"name": "out", "label": "Out", "type": "geometry"}],
            "params": self.params.iter().map(ParamSpec::to_json).collect::<Vec<_>>(),
        })
    }
}

/// What a node's evaluation sees of itself.
pub struct Ctx<'a> {
    /// The node's own seed: the graph seed mixed with the node id and its `seed` param.
    pub seed: u64,
    pub node: &'a ProcNode,
    pub p: Reader<'a>,
}

impl Ctx<'_> {
    /// A random stream for this node (`salt` separates independent uses).
    pub fn rng(&self, salt: u64) -> Rng {
        Rng::new(math::hash(&[self.seed, salt]))
    }
    /// A random number in [0, 1) for item `i` (independent of how many items come before).
    pub fn rand(&self, i: usize, salt: u64) -> f64 {
        math::unit(math::hash(&[self.seed, salt, i as u64]))
    }
}

/// The lists on a node's input ports.
pub struct Inputs {
    pub lists: Vec<Arc<Vec<Item>>>,
}

impl Inputs {
    /// Items on `port` (empty when nothing is connected).
    pub fn get(&self, port: usize) -> &[Item] {
        self.lists.get(port).map(|l| l.as_slice()).unwrap_or(&[])
    }
    pub fn connected(&self, port: usize) -> bool {
        self.lists.get(port).is_some_and(|l| !l.is_empty())
    }
}

const GEOMETRY: PortSpec = PortSpec { name: "geometry", label: "Geometry", doc: "the items to work on" };
const IN1: &[PortSpec] = &[GEOMETRY];

const SEED: ParamSpec = ParamSpec::int("seed", "Seed", 0, 0, 1_000_000_000, "varies this node's randomness (mixed with the object's seed)");
const BY_OPTIONS: &[&str] = &["order", "index", "random", "x", "y", "radius"];
const BY: ParamSpec = ParamSpec::choice(
    "by",
    "By",
    "order",
    BY_OPTIONS,
    "what runs from 0 to 1 across the items: list order, the `index` attribute, the item's random number, its x or y, its distance from the items' centre",
);
const PIVOT: ParamSpec =
    ParamSpec::choice("pivot", "Pivot", "center", &["center", "origin"], "turn and scale round the items' centre or the graph origin");
const TARGET: ParamSpec = ParamSpec::choice("target", "Target", "fill", &["fill", "stroke"], "which paint to change");

const SIZE_MAX: f64 = 100_000.0;

/// The whole catalogue, in menu order.
pub static CATALOGUE: &[NodeSpec] = &[
    // ---------- generate ----------
    NodeSpec {
        kind: "generate.rectangle",
        label: "Rectangle",
        category: "generate",
        doc: "a rectangle centred on the origin",
        inputs: &[],
        variadic: false,
        params: &[
            ParamSpec::num("width", "Width", 100.0, 0.0, SIZE_MAX, "pt"),
            ParamSpec::num("height", "Height", 100.0, 0.0, SIZE_MAX, "pt"),
            ParamSpec::num("radius", "Corner Radius", 0.0, 0.0, SIZE_MAX, "pt (clamped to half the shorter side)"),
        ],
        eval: nodes::generate::rectangle,
    },
    NodeSpec {
        kind: "generate.ellipse",
        label: "Ellipse",
        category: "generate",
        doc: "an ellipse centred on the origin",
        inputs: &[],
        variadic: false,
        params: &[ParamSpec::num("width", "Width", 100.0, 0.0, SIZE_MAX, "pt"), ParamSpec::num("height", "Height", 100.0, 0.0, SIZE_MAX, "pt")],
        eval: nodes::generate::ellipse,
    },
    NodeSpec {
        kind: "generate.polygon",
        label: "Polygon",
        category: "generate",
        doc: "a regular polygon centred on the origin, first corner straight up",
        inputs: &[],
        variadic: false,
        params: &[ParamSpec::int("sides", "Sides", 6, 3, 1000, ""), ParamSpec::num("radius", "Radius", 50.0, 0.0, SIZE_MAX, "pt, centre to corner")],
        eval: nodes::generate::polygon,
    },
    NodeSpec {
        kind: "generate.star",
        label: "Star",
        category: "generate",
        doc: "a star centred on the origin, first point straight up",
        inputs: &[],
        variadic: false,
        params: &[
            ParamSpec::int("points", "Points", 5, 2, 1000, ""),
            ParamSpec::num("radius", "Radius", 50.0, 0.0, SIZE_MAX, "pt, centre to the points"),
            ParamSpec::num("innerRatio", "Inner Ratio", 0.5, 0.0, 1.0, "inner radius as a fraction of the radius"),
        ],
        eval: nodes::generate::star,
    },
    NodeSpec {
        kind: "generate.line",
        label: "Line",
        category: "generate",
        doc: "a straight line between two points",
        inputs: &[],
        variadic: false,
        params: &[
            ParamSpec::num("x1", "Start X", -50.0, -SIZE_MAX, SIZE_MAX, "pt"),
            ParamSpec::num("y1", "Start Y", 0.0, -SIZE_MAX, SIZE_MAX, "pt"),
            ParamSpec::num("x2", "End X", 50.0, -SIZE_MAX, SIZE_MAX, "pt"),
            ParamSpec::num("y2", "End Y", 0.0, -SIZE_MAX, SIZE_MAX, "pt"),
        ],
        eval: nodes::generate::line,
    },
    NodeSpec {
        kind: "generate.spiral",
        label: "Spiral",
        category: "generate",
        doc: "an open spiral round the origin, from the inner to the outer radius",
        inputs: &[],
        variadic: false,
        params: &[
            ParamSpec::num("turns", "Turns", 3.0, 0.05, 100.0, ""),
            ParamSpec::num("innerRadius", "Inner Radius", 0.0, 0.0, SIZE_MAX, "pt"),
            ParamSpec::num("outerRadius", "Outer Radius", 100.0, 0.0, SIZE_MAX, "pt"),
            ParamSpec::choice(
                "growth",
                "Growth",
                "linear",
                &["linear", "exponential"],
                "linear: Archimedean spiral (even spacing); exponential: logarithmic spiral",
            ),
            ParamSpec::bool("clockwise", "Clockwise", false, "wind clockwise on the page"),
        ],
        eval: nodes::generate::spiral,
    },
    NodeSpec {
        kind: "generate.arc",
        label: "Arc",
        category: "generate",
        doc: "a circular arc round the origin",
        inputs: &[],
        variadic: false,
        params: &[
            ParamSpec::num("radius", "Radius", 50.0, 0.0, SIZE_MAX, "pt"),
            ParamSpec::num("startAngle", "Start Angle", 0.0, -3600.0, 3600.0, "degrees, counter-clockwise from 3 o'clock"),
            ParamSpec::num("sweep", "Sweep", 90.0, -360.0, 360.0, "degrees (negative: clockwise)"),
            ParamSpec::choice("closure", "Closure", "open", &["open", "chord", "pie"], "open arc, closed by a chord, or a pie slice"),
        ],
        eval: nodes::generate::arc,
    },
    // ---------- source ----------
    NodeSpec {
        kind: "source.art",
        label: "Art",
        category: "source",
        doc: "copies of objects from the document, embedded in the graph (procedural.create from a selection fills it)",
        inputs: &[],
        variadic: false,
        params: &[ParamSpec::bool("separate", "Separate Objects", false, "one item per object instead of one item for all of them")],
        eval: nodes::generate::art,
    },
    // ---------- points ----------
    NodeSpec {
        kind: "points.scatter",
        label: "Scatter Points",
        category: "points",
        doc: "random points in a rectangle centred on the origin, or inside the shapes on the input when connected",
        inputs: &[PortSpec { name: "shape", label: "Inside", doc: "optional: scatter inside these shapes" }],
        variadic: false,
        params: &[
            ParamSpec::int("count", "Count", 100, 0, 20_000, ""),
            ParamSpec::num("width", "Width", 300.0, 0.0, SIZE_MAX, "pt (without a shape)"),
            ParamSpec::num("height", "Height", 200.0, 0.0, SIZE_MAX, "pt (without a shape)"),
            SEED,
        ],
        eval: nodes::points::scatter,
    },
    NodeSpec {
        kind: "points.poisson",
        label: "Poisson Disk Points",
        category: "points",
        doc: "evenly spread random points no closer than a minimum distance (Bridson's algorithm), in a rectangle or inside the input shapes",
        inputs: &[PortSpec { name: "shape", label: "Inside", doc: "optional: fill these shapes" }],
        variadic: false,
        params: &[
            ParamSpec::num("distance", "Min Distance", 20.0, 0.5, SIZE_MAX, "pt"),
            ParamSpec::num("width", "Width", 300.0, 0.0, SIZE_MAX, "pt (without a shape)"),
            ParamSpec::num("height", "Height", 200.0, 0.0, SIZE_MAX, "pt (without a shape)"),
            ParamSpec::int("maxCount", "Max Count", 2000, 1, 20_000, "stop after this many points"),
            SEED,
        ],
        eval: nodes::points::poisson,
    },
    NodeSpec {
        kind: "points.grid",
        label: "Grid Points",
        category: "points",
        doc: "a grid of points centred on the origin",
        inputs: &[],
        variadic: false,
        params: &[
            ParamSpec::int("columns", "Columns", 10, 1, 20_000, ""),
            ParamSpec::int("rows", "Rows", 10, 1, 20_000, ""),
            ParamSpec::num("spacingX", "Horizontal Spacing", 20.0, 0.0, SIZE_MAX, "pt"),
            ParamSpec::num("spacingY", "Vertical Spacing", 20.0, 0.0, SIZE_MAX, "pt"),
            ParamSpec::bool("stagger", "Stagger", false, "shift every other row by half a column (a hexagonal grid)"),
        ],
        eval: nodes::points::grid,
    },
    NodeSpec {
        kind: "points.circle",
        label: "Circle Points",
        category: "points",
        doc: "points evenly round a circle centred on the origin, each turned along the circle",
        inputs: &[],
        variadic: false,
        params: &[
            ParamSpec::int("count", "Count", 12, 1, 20_000, ""),
            ParamSpec::num("radius", "Radius", 100.0, 0.0, SIZE_MAX, "pt"),
            ParamSpec::num("startAngle", "Start Angle", 90.0, -3600.0, 3600.0, "degrees, counter-clockwise from 3 o'clock"),
        ],
        eval: nodes::points::circle,
    },
    NodeSpec {
        kind: "points.alongPath",
        label: "Points Along Path",
        category: "points",
        doc: "points spread evenly along each path of the input, each turned along the path",
        inputs: &[PortSpec { name: "path", label: "Path", doc: "the paths to sample" }],
        variadic: false,
        params: &[
            ParamSpec::int("count", "Count", 10, 1, 20_000, "points per subpath (when spacing is 0)"),
            ParamSpec::num("spacing", "Spacing", 0.0, 0.0, SIZE_MAX, "pt between points; 0 uses count"),
        ],
        eval: nodes::points::along_path,
    },
    NodeSpec {
        kind: "points.anchors",
        label: "Anchor Points",
        category: "points",
        doc: "a point on every anchor of the input paths",
        inputs: &[PortSpec { name: "path", label: "Path", doc: "the paths whose anchors to take" }],
        variadic: false,
        params: &[],
        eval: nodes::points::anchors,
    },
    // ---------- instance ----------
    NodeSpec {
        kind: "instance.linear",
        label: "Repeat Linear",
        category: "instance",
        doc: "copies of the input, each step moved, turned and scaled a little more",
        inputs: IN1,
        variadic: false,
        params: &[
            ParamSpec::int("count", "Count", 5, 1, 20_000, ""),
            ParamSpec::num("offsetX", "Offset X", 20.0, -SIZE_MAX, SIZE_MAX, "pt per step"),
            ParamSpec::num("offsetY", "Offset Y", 0.0, -SIZE_MAX, SIZE_MAX, "pt per step"),
            ParamSpec::num("rotate", "Rotate", 0.0, -3600.0, 3600.0, "degrees per step"),
            ParamSpec::num("scale", "Scale", 1.0, 0.01, 100.0, "scale factor per step"),
            PIVOT,
        ],
        eval: nodes::instance::linear,
    },
    NodeSpec {
        kind: "instance.grid",
        label: "Repeat Grid",
        category: "instance",
        doc: "copies of the input in a grid centred on it",
        inputs: IN1,
        variadic: false,
        params: &[
            ParamSpec::int("columns", "Columns", 5, 1, 20_000, ""),
            ParamSpec::int("rows", "Rows", 5, 1, 20_000, ""),
            ParamSpec::num("spacingX", "Horizontal Spacing", 40.0, -SIZE_MAX, SIZE_MAX, "pt between copies"),
            ParamSpec::num("spacingY", "Vertical Spacing", 40.0, -SIZE_MAX, SIZE_MAX, "pt between copies"),
            ParamSpec::bool("stagger", "Stagger", false, "shift every other row by half a column"),
        ],
        eval: nodes::instance::grid,
    },
    NodeSpec {
        kind: "instance.radial",
        label: "Repeat Radial",
        category: "instance",
        doc: "copies of the input round the origin: each is moved `radius` up, then turned round the origin",
        inputs: IN1,
        variadic: false,
        params: &[
            ParamSpec::int("count", "Count", 8, 1, 20_000, ""),
            ParamSpec::num("radius", "Radius", 100.0, -SIZE_MAX, SIZE_MAX, "pt"),
            ParamSpec::num("startAngle", "Start Angle", 0.0, -3600.0, 3600.0, "degrees"),
            ParamSpec::num("sweep", "Sweep", 360.0, -360.0, 360.0, "degrees the copies spread over"),
            ParamSpec::bool("rotate", "Rotate Copies", true, "turn each copy to face out from the centre"),
        ],
        eval: nodes::instance::radial,
    },
    NodeSpec {
        kind: "instance.copyToPoints",
        label: "Copy to Points",
        category: "instance",
        doc: "a copy of the instance on every item of the points input (bare points, or the centre of any item)",
        inputs: &[
            PortSpec { name: "points", label: "Points", doc: "where the copies go" },
            PortSpec { name: "instance", label: "Instance", doc: "what is copied" },
        ],
        variadic: false,
        params: &[
            ParamSpec::bool("align", "Align to Points", false, "turn each copy with its point (along a path or circle)"),
            ParamSpec::num("scale", "Scale", 1.0, 0.0, 1000.0, "scale of every copy"),
            PIVOT,
        ],
        eval: nodes::instance::copy_to_points,
    },
    NodeSpec {
        kind: "instance.mirror",
        label: "Mirror",
        category: "instance",
        doc: "the input and its reflection",
        inputs: IN1,
        variadic: false,
        params: &[
            ParamSpec::choice(
                "axis",
                "Axis",
                "vertical",
                &["vertical", "horizontal", "both"],
                "vertical: reflect left↔right; horizontal: top↔bottom; both: four copies",
            ),
            ParamSpec::num("offset", "Axis Position", 0.0, -SIZE_MAX, SIZE_MAX, "pt from the origin"),
            ParamSpec::bool("keepOriginal", "Keep Original", true, ""),
        ],
        eval: nodes::instance::mirror,
    },
    // ---------- modify ----------
    NodeSpec {
        kind: "modify.transform",
        label: "Transform",
        category: "modify",
        doc: "move, turn, scale and skew the items",
        inputs: IN1,
        variadic: false,
        params: &[
            ParamSpec::num("x", "Move X", 0.0, -SIZE_MAX, SIZE_MAX, "pt"),
            ParamSpec::num("y", "Move Y", 0.0, -SIZE_MAX, SIZE_MAX, "pt"),
            ParamSpec::num("rotate", "Rotate", 0.0, -3600.0, 3600.0, "degrees, counter-clockwise"),
            ParamSpec::num("scaleX", "Scale X", 1.0, -1000.0, 1000.0, "factor"),
            ParamSpec::num("scaleY", "Scale Y", 1.0, -1000.0, 1000.0, "factor"),
            ParamSpec::num("skewX", "Skew X", 0.0, -89.0, 89.0, "degrees"),
            ParamSpec::num("skewY", "Skew Y", 0.0, -89.0, 89.0, "degrees"),
            PIVOT,
            ParamSpec::bool("each", "Each Item", false, "transform every item round its own centre"),
        ],
        eval: nodes::modify::transform,
    },
    NodeSpec {
        kind: "modify.jitter",
        label: "Jitter",
        category: "modify",
        doc: "move, turn and scale every item by a random amount",
        inputs: IN1,
        variadic: false,
        params: &[
            ParamSpec::num("move", "Move", 0.0, 0.0, SIZE_MAX, "up to this many pt along each axis"),
            ParamSpec::num("rotate", "Rotate", 0.0, 0.0, 360.0, "up to ± degrees"),
            ParamSpec::num("scale", "Scale", 0.0, 0.0, 0.99, "up to ± this fraction"),
            SEED,
        ],
        eval: nodes::modify::jitter,
    },
    NodeSpec {
        kind: "modify.noise",
        label: "Noise Displace",
        category: "modify",
        doc: "subdivide the paths and push their points by fractal noise",
        inputs: IN1,
        variadic: false,
        params: &[
            ParamSpec::num("amplitude", "Amplitude", 10.0, 0.0, SIZE_MAX, "pt"),
            ParamSpec::num("frequency", "Frequency", 0.02, 0.0, 10.0, "noise features per pt"),
            ParamSpec::int("octaves", "Octaves", 3, 1, 8, "layers of finer detail"),
            ParamSpec::num("detail", "Detail", 4.0, 0.25, 1000.0, "pt between the points the paths are cut into"),
            ParamSpec::bool("smooth", "Smooth", true, "curves through the moved points (else straight segments)"),
            SEED,
        ],
        eval: nodes::modify::noise,
    },
    NodeSpec {
        kind: "modify.roundCorners",
        label: "Round Corners",
        category: "modify",
        doc: "round the sharp corners between straight segments",
        inputs: IN1,
        variadic: false,
        params: &[ParamSpec::num("radius", "Radius", 10.0, 0.0, SIZE_MAX, "pt")],
        eval: nodes::modify::round_corners,
    },
    NodeSpec {
        kind: "modify.smooth",
        label: "Smooth",
        category: "modify",
        doc: "turn corners into smooth curves",
        inputs: IN1,
        variadic: false,
        params: &[ParamSpec::num("amount", "Amount", 1.0, 0.0, 1.0, "0 unchanged, 1 fully smooth")],
        eval: nodes::modify::smooth,
    },
    NodeSpec {
        kind: "modify.offset",
        label: "Offset Path",
        category: "modify",
        doc: "grow or shrink the shapes (Object › Path › Offset Path)",
        inputs: IN1,
        variadic: false,
        params: &[
            ParamSpec::num("distance", "Offset", 5.0, -10_000.0, 10_000.0, "pt (negative shrinks)"),
            ParamSpec::choice("join", "Joins", "miter", &["miter", "round", "bevel"], ""),
            ParamSpec::num("miterLimit", "Miter Limit", 4.0, 1.0, 500.0, ""),
        ],
        eval: nodes::modify::offset,
    },
    NodeSpec {
        kind: "modify.simplify",
        label: "Simplify",
        category: "modify",
        doc: "fewer anchors for the same shape",
        inputs: IN1,
        variadic: false,
        params: &[ParamSpec::num("tolerance", "Tolerance", 1.0, 0.01, 1000.0, "pt the result may stray")],
        eval: nodes::modify::simplify,
    },
    NodeSpec {
        kind: "modify.morph",
        label: "Morph",
        category: "modify",
        doc: "in-between shapes: item i of A blended towards item i of B (the shorter list repeats)",
        inputs: &[PortSpec { name: "a", label: "A", doc: "from" }, PortSpec { name: "b", label: "B", doc: "to" }],
        variadic: false,
        params: &[ParamSpec::num("t", "Amount", 0.5, 0.0, 1.0, "0 = A, 1 = B")],
        eval: nodes::modify::morph,
    },
    NodeSpec {
        kind: "modify.boolean",
        label: "Boolean",
        category: "modify",
        doc: "Pathfinder: with B connected, A (united) combined with B (united); without B, the items of A combined back to front",
        inputs: &[
            PortSpec { name: "a", label: "A", doc: "the shapes" },
            PortSpec { name: "b", label: "B", doc: "optional: the shapes to combine with" },
        ],
        variadic: false,
        params: &[ParamSpec::choice(
            "operation",
            "Operation",
            "union",
            &["union", "subtract", "intersect", "exclude", "divide"],
            "subtract: A minus B (without B: the back item minus the others)",
        )],
        eval: nodes::modify::boolean,
    },
    NodeSpec {
        kind: "modify.merge",
        label: "Merge",
        category: "modify",
        doc: "all inputs in one list, first input at the back",
        inputs: &[PortSpec { name: "in0", label: "In", doc: "connect any number of inputs" }],
        variadic: true,
        params: &[],
        eval: nodes::modify::merge,
    },
    NodeSpec {
        kind: "modify.select",
        label: "Select",
        category: "modify",
        doc: "keep some of the items: every nth, a range, or a random fraction",
        inputs: IN1,
        variadic: false,
        params: &[
            ParamSpec::choice("mode", "Mode", "everyNth", &["everyNth", "range", "random"], ""),
            ParamSpec::int("n", "Every", 2, 1, 20_000, "everyNth: keep one in n"),
            ParamSpec::int("offset", "Offset", 0, 0, 20_000, "everyNth: the first kept"),
            ParamSpec::int("start", "Start", 0, -20_000, 20_000, "range: first index (negative counts from the end)"),
            ParamSpec::int("end", "End", -1, -20_000, 20_000, "range: last index, inclusive (negative counts from the end)"),
            ParamSpec::num("fraction", "Fraction", 0.5, 0.0, 1.0, "random: share kept"),
            ParamSpec::bool("invert", "Invert", false, "keep the others instead"),
            SEED,
        ],
        eval: nodes::modify::select,
    },
    NodeSpec {
        kind: "modify.order",
        label: "Sort",
        category: "modify",
        doc: "reorder the items (paint order: first at the back)",
        inputs: IN1,
        variadic: false,
        params: &[
            ParamSpec::choice(
                "mode",
                "Mode",
                "reverse",
                &["reverse", "shuffle", "x", "y", "size", "radius"],
                "reverse, random, or sort by x, y, area of the bounds, distance from the centre",
            ),
            ParamSpec::bool("descending", "Descending", false, "sorts: largest first"),
            SEED,
        ],
        eval: nodes::modify::order,
    },
    NodeSpec {
        kind: "modify.boundingBox",
        label: "Bounding Box",
        category: "modify",
        doc: "rectangles round the items (one for all, or one each)",
        inputs: IN1,
        variadic: false,
        params: &[
            ParamSpec::bool("each", "Each Item", false, ""),
            ParamSpec::num("padding", "Padding", 0.0, -SIZE_MAX, SIZE_MAX, "pt added on every side"),
        ],
        eval: nodes::modify::bounding_box,
    },
    // ---------- style ----------
    NodeSpec {
        kind: "style.fill",
        label: "Fill",
        category: "style",
        doc: "set the fill colour",
        inputs: IN1,
        variadic: false,
        params: &[ParamSpec::color("color", "Color", Some("#3366cc"), "\"none\" removes the fill")],
        eval: nodes::style::fill,
    },
    NodeSpec {
        kind: "style.stroke",
        label: "Stroke",
        category: "style",
        doc: "set the stroke colour and weight",
        inputs: IN1,
        variadic: false,
        params: &[
            ParamSpec::color("color", "Color", Some("#000000"), "\"none\" removes the stroke"),
            ParamSpec::num("width", "Weight", 1.0, 0.0, 1000.0, "pt"),
        ],
        eval: nodes::style::stroke,
    },
    NodeSpec {
        kind: "style.colorRamp",
        label: "Color Ramp",
        category: "style",
        doc: "colour the items from one colour to another across the list",
        inputs: IN1,
        variadic: false,
        params: &[
            ParamSpec::color("from", "From", Some("#ff6a3d"), ""),
            ParamSpec::color("to", "To", Some("#3d5afe"), ""),
            BY,
            ParamSpec::choice("space", "Blend", "rgb", &["rgb", "hsl"], "hsl blends round the colour wheel (the hue turns)"),
            TARGET,
        ],
        eval: nodes::style::color_ramp,
    },
    NodeSpec {
        kind: "style.hueShift",
        label: "Hue Shift",
        category: "style",
        doc: "turn every item's colour round the colour wheel, more for later items",
        inputs: IN1,
        variadic: false,
        params: &[ParamSpec::num("degrees", "Degrees", 180.0, -3600.0, 3600.0, "the shift of the last item"), BY, TARGET],
        eval: nodes::style::hue_shift,
    },
    NodeSpec {
        kind: "style.opacityRamp",
        label: "Opacity Ramp",
        category: "style",
        doc: "fade the items from one opacity to another",
        inputs: IN1,
        variadic: false,
        params: &[ParamSpec::num("from", "From", 1.0, 0.0, 1.0, ""), ParamSpec::num("to", "To", 0.2, 0.0, 1.0, ""), BY],
        eval: nodes::style::opacity_ramp,
    },
    NodeSpec {
        kind: "style.randomColor",
        label: "Random Color",
        category: "style",
        doc: "give every item a colour picked at random from a palette",
        inputs: IN1,
        variadic: false,
        params: &[ParamSpec::palette("palette", "Palette", &["#ef476f", "#ffd166", "#06d6a0", "#118ab2", "#073b4c"], ""), TARGET, SEED],
        eval: nodes::style::random_color,
    },
    // ---------- output ----------
    NodeSpec {
        kind: "output",
        label: "Output",
        category: "output",
        doc: "the object's art: what reaches this node becomes the group's contents",
        inputs: IN1,
        variadic: false,
        params: &[],
        eval: nodes::modify::passthrough,
    },
];

/// The spec of a node kind.
pub fn spec(kind: &str) -> Option<&'static NodeSpec> {
    CATALOGUE.iter().find(|s| s.kind == kind)
}

/// The catalogue as JSON (`procedural.catalogue`).
pub fn catalogue_json() -> Value {
    json!({
        "version": vectorcraft_doc::PROC_GRAPH_VERSION,
        "categories": ["generate", "source", "points", "instance", "modify", "style", "output"],
        "kinds": CATALOGUE.iter().map(NodeSpec::to_json).collect::<Vec<_>>(),
    })
}
