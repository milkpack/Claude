//! Hard caps on everything an untrusted graph can ask for. A graph that asks for more gets a
//! clamped result and a warning, never a hang or an allocation without bound.

/// Items in one node's output (and so in the generated object).
pub const MAX_ITEMS: usize = 20_000;
/// Document nodes one output can turn into (embedded art counts every node inside it).
pub const MAX_WEIGHT: usize = 100_000;
/// Anchors in one node's output, all items together.
pub const MAX_TOTAL_ANCHORS: usize = 400_000;
/// Anchors in one path (subdivision and resampling stop here).
pub const MAX_ANCHORS_PER_PATH: usize = 20_000;
/// Anchors a boolean, offset or simplify node accepts.
pub const EXPENSIVE_ANCHORS: usize = 10_000;
/// Shapes a boolean node accepts.
pub const MAX_BOOLEAN_ITEMS: usize = 2_000;
/// Nodes in a graph (more are ignored, with an error).
pub const MAX_NODES: usize = 256;
/// Input ports of a variadic node.
pub const MAX_PORTS: usize = 32;
/// Elementary steps (point-in-shape segment tests, sampling attempts) one node may spend.
pub const WORK_BUDGET: usize = 5_000_000;
/// Length of names and kind ids kept from untrusted graphs.
pub const MAX_NAME: usize = 200;
