// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! What a network reaches, and what it stops reaching when you cut it.
//!
//! Upstream, downstream, reach and isochrone are not four algorithms. They are one search from a
//! set of seeds with three knobs — which way arcs may be crossed, what a crossing costs, and how
//! much cost is allowed — and writing them separately is how a domain word ends up in a core that
//! claims not to have any. "Upstream" is water's name for `Direction::Backward`; a road network
//! calls the same answer "what feeds this junction".
//!
//! Nothing here knows what an edge is. It knows edges have a cost, that some may only be crossed
//! one way, and that the CSR already says which — section 4 gives a directed edge one adjacency
//! entry instead of two, so the direction rule needs no code at all in the forward case.

use std::collections::{BinaryHeap, VecDeque};

use serde_json::Value;
use trama_format::{Graph, edge_lengths, edge_properties, parse_graph, read_sections};
use trama_solver::server::{Rejection, Request, Solver};
use trama_solver::{declared, pack};

/// Which way an arc may be crossed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    /// With the network: what this point feeds. Water calls it downstream.
    Forward,
    /// Against it: what feeds this point.
    Backward,
    /// Ignoring direction entirely, which is the question "what is connected to this".
    Both,
}

/// What crossing an edge costs, and therefore what a budget is measured in.
#[derive(Clone, Debug)]
pub enum Cost {
    /// One per edge. A budget is then a number of steps away from the seeds.
    Hops,
    /// Metres, from the geometry the container already carries.
    Length,
    /// Seconds: length over speed, from a `PROP` column where one is named and usable, and from
    /// the fallback everywhere else. The same rule `trama-routing` costs a route by.
    Seconds { metres_per_second: f64, speed_property: Option<String> },
}

/// The question being asked of the network.
#[derive(Clone, Debug)]
pub enum Operation {
    /// Everything reachable from `seeds`. Downstream, upstream, connected reach and isochrone,
    /// depending on `direction` and whether there is a budget.
    Trace { seeds: Vec<usize>, direction: Direction, budget: Option<f64> },
    /// Every edge labelled with the connected component it belongs to, direction ignored. The
    /// answer to "is this one network or twelve", which a rendered map hides.
    Components,
    /// What loses service when `cut` is removed: every edge no longer reachable from `seeds`,
    /// the cut edges included. Closing a valve, closing a street — the same question.
    Isolation { cut: Vec<usize>, seeds: Vec<usize>, direction: Direction },
    /// The edges that are the only way through: cutting one splits the network in two. Bridges,
    /// and the shortest answer to "where is this network fragile".
    Critical,
    /// Which of `sources` serves each edge, by cost. The label is a position in `sources`, so a
    /// network with two reservoirs or two depots comes back split between them.
    Allocation { sources: Vec<usize>, direction: Direction },
}

pub struct Parameters {
    pub channel: String,
    pub operation: Operation,
    pub cost: Cost,
    /// The scrub's resolution. A trace is emitted as a progression so the spread can be watched,
    /// in whatever unit `cost` is measured in.
    pub step_seconds: f32,
}

impl Default for Parameters {
    fn default() -> Self {
        Self {
            channel: "reach".into(),
            operation: Operation::Trace { seeds: Vec::new(), direction: Direction::Forward, budget: None },
            cost: Cost::Length,
            step_seconds: 60.0,
        }
    }
}

/// An edge the search reached, and what it had spent on arrival.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Reached {
    pub edge_index: usize,
    pub at: f64,
}

/// Packed deltas per the contract: 18 bytes of `(entity_id, channel, t, value)` and nothing else.
pub fn solve(container: &[u8], parameters: &Parameters, t0: f32, t1: f32) -> Result<Vec<u8>, String> {
    if t1 < t0 {
        return Err("t1_seconds must not precede t0_seconds".into());
    }
    if parameters.step_seconds <= 0.0 {
        return Err("step_seconds must be positive".into());
    }
    let channel = declared(container, &parameters.channel, 2)?;
    let sections = read_sections(container)?;
    let graph = parse_graph(
        &sections.iter().find(|s| &s.kind == b"GRPH").ok_or("container is missing a GRPH section")?.payload,
    )?;

    let values: Vec<(usize, f64)> = match &parameters.operation {
        Operation::Trace { seeds, direction, budget } => {
            let costs = costs_of(container, &graph, &parameters.cost)?;
            trace(&graph, &costs, seeds, *direction, *budget)?
                .into_iter()
                .map(|reached| (reached.edge_index, reached.at))
                .collect()
        }
        // A label is not a progression: every edge carries its answer from the start.
        Operation::Components => {
            components(&graph).into_iter().enumerate().map(|(edge, label)| (edge, label as f64)).collect()
        }
        Operation::Isolation { cut, seeds, direction } => {
            let costs = costs_of(container, &graph, &parameters.cost)?;
            isolation(&graph, &costs, cut, seeds, *direction)?
                .into_iter()
                .enumerate()
                .map(|(edge, lost)| (edge, if lost { 1.0 } else { 0.0 }))
                .collect()
        }
        Operation::Critical => critical(&graph)
            .into_iter()
            .enumerate()
            .map(|(edge, bridge)| (edge, if bridge { 1.0 } else { 0.0 }))
            .collect(),
        Operation::Allocation { sources, direction } => {
            let costs = costs_of(container, &graph, &parameters.cost)?;
            allocation(&graph, &costs, sources, *direction)?
                .into_iter()
                .enumerate()
                .filter_map(|(edge, owner)| owner.map(|owner| (edge, owner as f64)))
                .collect()
        }
    };

    let progressive = matches!(parameters.operation, Operation::Trace { .. });
    let steps = ((t1 - t0) / parameters.step_seconds).floor() as i64 + 1;
    let mut records = Vec::new();
    for step in 0..steps {
        let t = t0 + step as f32 * parameters.step_seconds;
        let elapsed = (t - t0) as f64;
        for (edge_index, value) in &values {
            // Reached rather than occupied, as in `trama-routing`: scrubbing backwards unwinds
            // the spread instead of losing it.
            let written = if !progressive {
                *value
            } else if elapsed >= *value {
                1.0
            } else {
                0.0
            };
            records.extend_from_slice(&pack(graph.edges[*edge_index].id, channel, t, written as f32));
        }
    }
    Ok(records)
}

/// Every edge the search can cross, with the cost standing on it when it does.
///
/// Multi-seed Dijkstra, which degenerates to a breadth-first search when every edge costs the
/// same. One search covers all four questions: the seeds say where from, `direction` which arcs
/// exist, and `budget` where to stop.
pub fn trace(
    graph: &Graph,
    costs: &[f64],
    seeds: &[usize],
    direction: Direction,
    budget: Option<f64>,
) -> Result<Vec<Reached>, String> {
    search(graph, costs, seeds, direction, budget, &vec![false; graph.edges.len()])
}

/// The search itself, with a set of edges it may not cross.
fn search(
    graph: &Graph,
    costs: &[f64],
    seeds: &[usize],
    direction: Direction,
    budget: Option<f64>,
    blocked: &[bool],
) -> Result<Vec<Reached>, String> {
    if seeds.is_empty() {
        return Err("a search needs at least one seed node".into());
    }
    if let Some(node) = seeds.iter().find(|node| **node >= graph.nodes.len()) {
        return Err(format!("seed node {node} is outside the graph"));
    }
    if budget.is_some_and(|limit| !(limit.is_finite() && limit >= 0.0)) {
        return Err("a budget must be a finite, non-negative number".into());
    }
    let steps = steps_from(graph, direction);
    // Integer keys, scaled, because a BinaryHeap needs Ord and f64 has none. Millimetres of a
    // metre or milliseconds of a second: below anything a network is measured to.
    let scale = 1000.0;
    let ceiling = budget.map(|limit| (limit * scale).round() as i64);
    let mut best = vec![i64::MAX; graph.nodes.len()];
    let mut reached: Vec<Option<i64>> = vec![None; graph.edges.len()];
    let mut queue = BinaryHeap::new();
    for seed in seeds {
        best[*seed] = 0;
        queue.push((0i64, *seed));
    }
    while let Some((negated, node)) = queue.pop() {
        let spent = -negated;
        if spent > best[node] {
            continue;
        }
        for (edge_index, next) in &steps[node] {
            if blocked[*edge_index] {
                continue;
            }
            let crossing = (costs[*edge_index] * scale).round() as i64;
            let arrival = spent.saturating_add(crossing);
            if ceiling.is_some_and(|limit| arrival > limit) {
                continue;
            }
            // The edge is reached even if the node beyond it was already cheaper by another way:
            // the question is which edges the search can cross, not which nodes it settles.
            if reached[*edge_index].is_none_or(|previous| arrival < previous) {
                reached[*edge_index] = Some(arrival);
            }
            if arrival < best[*next] {
                best[*next] = arrival;
                queue.push((-arrival, *next));
            }
        }
    }
    Ok(reached
        .into_iter()
        .enumerate()
        .filter_map(|(edge_index, at)| at.map(|at| Reached { edge_index, at: at as f64 / scale }))
        .collect())
}

/// Which edges lose service when `cut` is removed, the cut edges among them.
///
/// The same search as `trace`, with a set of edges that may not be crossed, and the answer read
/// the other way round: what the search failed to reach. A network with no seeds has nothing to
/// lose service from, which is why they are required rather than defaulted.
pub fn isolation(
    graph: &Graph,
    costs: &[f64],
    cut: &[usize],
    seeds: &[usize],
    direction: Direction,
) -> Result<Vec<bool>, String> {
    if let Some(edge) = cut.iter().find(|edge| **edge >= graph.edges.len()) {
        return Err(format!("cut edge {edge} is outside the graph"));
    }
    let mut blocked = vec![false; graph.edges.len()];
    for edge in cut {
        blocked[*edge] = true;
    }
    let mut survives = vec![false; graph.edges.len()];
    for reached in search(graph, costs, seeds, direction, None, &blocked)? {
        survives[reached.edge_index] = true;
    }
    // A cut edge is out of service by construction: the search refuses to cross it, so it is
    // never reached, and `!survives` already covers it.
    Ok(survives.into_iter().map(|reached| !reached).collect())
}

/// The edges whose removal splits the network: bridges, found in one pass.
///
/// Tarjan's low-link, on the undirected view, because "is this the only way through" is a
/// question about connectivity and not about which way the arrows point. Parallel edges are
/// handled by refusing to return along the *edge* just used rather than to the node it came
/// from — two pipes between the same pair are each other's spare, and neither is critical.
pub fn critical(graph: &Graph) -> Vec<bool> {
    let steps = steps_from(graph, Direction::Both);
    let mut bridge = vec![false; graph.edges.len()];
    let mut discovered = vec![usize::MAX; graph.nodes.len()];
    let mut low = vec![usize::MAX; graph.nodes.len()];
    let mut clock = 0;
    for start in 0..graph.nodes.len() {
        if discovered[start] != usize::MAX {
            continue;
        }
        discovered[start] = clock;
        low[start] = clock;
        clock += 1;
        // An explicit stack of (node, edge arrived by, how far through its neighbours): a city
        // network is deeper than a recursive descent is willing to go.
        let mut stack = vec![(start, usize::MAX, 0usize)];
        while let Some(&(node, arrived_by, cursor)) = stack.last() {
            if cursor == steps[node].len() {
                stack.pop();
                if let Some(&(parent, _, _)) = stack.last() {
                    low[parent] = low[parent].min(low[node]);
                    // Nothing below `node` reaches back past its parent, so the edge between
                    // them is the only way through.
                    if low[node] > discovered[parent] {
                        bridge[arrived_by] = true;
                    }
                }
                continue;
            }
            stack.last_mut().expect("just read").2 += 1;
            let (edge_index, next) = steps[node][cursor];
            // Refusing the edge just used, not the node it came from: two edges between the same
            // pair are each other's spare, and neither is critical.
            if edge_index == arrived_by {
                continue;
            }
            if discovered[next] == usize::MAX {
                discovered[next] = clock;
                low[next] = clock;
                clock += 1;
                stack.push((next, edge_index, 0));
            } else {
                low[node] = low[node].min(discovered[next]);
            }
        }
    }
    bridge
}

/// Which source serves each edge, as a position in `sources`.
///
/// One search from every source at once, each carrying its own label. An edge belongs to
/// whichever source reaches it more cheaply, and to nobody where nothing reaches it at all.
pub fn allocation(
    graph: &Graph,
    costs: &[f64],
    sources: &[usize],
    direction: Direction,
) -> Result<Vec<Option<usize>>, String> {
    if sources.is_empty() {
        return Err("an allocation needs at least one source".into());
    }
    let mut owner: Vec<Option<usize>> = vec![None; graph.edges.len()];
    let mut best: Vec<f64> = vec![f64::INFINITY; graph.edges.len()];
    for (label, source) in sources.iter().enumerate() {
        for reached in trace(graph, costs, &[*source], direction, None)? {
            if reached.at < best[reached.edge_index] {
                best[reached.edge_index] = reached.at;
                owner[reached.edge_index] = Some(label);
            }
        }
    }
    Ok(owner)
}

/// The connected component of every edge, counting from zero, direction ignored.
pub fn components(graph: &Graph) -> Vec<usize> {
    let steps = steps_from(graph, Direction::Both);
    let mut label = vec![usize::MAX; graph.nodes.len()];
    let mut next_label = 0;
    for start in 0..graph.nodes.len() {
        if label[start] != usize::MAX {
            continue;
        }
        let mut queue = VecDeque::from([start]);
        label[start] = next_label;
        while let Some(node) = queue.pop_front() {
            for (_edge, next) in &steps[node] {
                if label[*next] == usize::MAX {
                    label[*next] = next_label;
                    queue.push_back(*next);
                }
            }
        }
        next_label += 1;
    }
    // An edge belongs to its source's component; both ends share one by construction.
    graph.edges.iter().map(|edge| label[edge.source as usize]).collect()
}

/// For each node, the edges that may be crossed from it and where each one lands.
///
/// Forward needs no rule of its own: SPEC 4 already gives a directed edge a single adjacency
/// entry at its source, so walking the CSR as written is walking with the network. The other two
/// directions are that list inverted, and both lists together.
fn steps_from(graph: &Graph, direction: Direction) -> Vec<Vec<(usize, usize)>> {
    let mut forward: Vec<Vec<(usize, usize)>> = vec![Vec::new(); graph.nodes.len()];
    for (node, steps) in forward.iter_mut().enumerate() {
        let from = graph.csr_offsets[node] as usize;
        let to = graph.csr_offsets[node + 1] as usize;
        for entry in &graph.adjacency[from..to] {
            let edge = &graph.edges[entry.edge_index as usize];
            let landing = if entry.traversal_direction == 1 { edge.target } else { edge.source };
            steps.push((entry.edge_index as usize, landing as usize));
        }
    }
    if direction == Direction::Forward {
        return forward;
    }
    let mut backward: Vec<Vec<(usize, usize)>> = vec![Vec::new(); graph.nodes.len()];
    for (node, steps) in forward.iter().enumerate() {
        for (edge_index, landing) in steps {
            backward[*landing].push((*edge_index, node));
        }
    }
    match direction {
        Direction::Backward => backward,
        _ => {
            for (node, steps) in backward.into_iter().enumerate() {
                forward[node].extend(steps);
            }
            forward
        }
    }
}

/// What crossing each edge costs, in the unit the caller asked for.
fn costs_of(container: &[u8], graph: &Graph, cost: &Cost) -> Result<Vec<f64>, String> {
    match cost {
        Cost::Hops => Ok(vec![1.0; graph.edges.len()]),
        Cost::Length => edge_lengths(container),
        Cost::Seconds { metres_per_second, speed_property } => {
            if !(metres_per_second.is_finite() && *metres_per_second > 0.0) {
                return Err("metres_per_second must be a positive, finite number".into());
            }
            let lengths = edge_lengths(container)?;
            let Some(key) = speed_property else {
                return Ok(lengths.iter().map(|length| length / metres_per_second).collect());
            };
            let rows = edge_properties(container)?;
            Ok(lengths
                .iter()
                .enumerate()
                .map(|(edge, length)| {
                    let speed = rows
                        .get(edge)
                        .and_then(|row| row.get(key))
                        .and_then(serde_json::Value::as_f64)
                        .filter(|speed| speed.is_finite() && *speed > 0.0)
                        .unwrap_or(*metres_per_second);
                    length / speed
                })
                .collect())
        }
    }
}

pub struct TraceSolver;

const KNOWN: [&str; 9] =
    ["channel", "operation", "seeds", "direction", "budget", "cost", "step_seconds", "cut", "sources"];

impl Solver for TraceSolver {
    fn id(&self) -> &'static str {
        "trace"
    }

    fn contract_versions(&self) -> &'static [&'static str] {
        &["0.1.0", "0.2.0"]
    }

    fn solve(&self, request: &Request) -> Result<Vec<u8>, Rejection> {
        let defaults = Parameters::default();
        if let Some(unknown) = request
            .params
            .as_object()
            .and_then(|params| params.keys().find(|key| !KNOWN.contains(&key.as_str())).cloned())
        {
            return Err(Rejection::request(format!("unknown parameters: {unknown}")));
        }
        let indices = |key: &str| -> Result<Vec<usize>, Rejection> {
            match &request.params[key] {
                Value::Array(values) => values
                    .iter()
                    .map(|value| value.as_u64().map(|index| index as usize).ok_or("must be non-negative indices"))
                    .collect::<Result<Vec<usize>, &str>>()
                    .map_err(|error| Rejection::request(format!("{key} {error}"))),
                Value::Null => Err(Rejection::request(format!("{key} is required"))),
                _ => Err(Rejection::request(format!("{key} must be an array"))),
            }
        };
        let direction = |default: &str| match request.params["direction"].as_str().unwrap_or(default) {
            "forward" => Ok(Direction::Forward),
            "backward" => Ok(Direction::Backward),
            "both" => Ok(Direction::Both),
            other => Err(Rejection::request(format!("unknown direction '{other}'"))),
        };
        let operation = match request.params["operation"].as_str().unwrap_or("trace") {
            "components" => Operation::Components,
            "critical" => Operation::Critical,
            "isolation" => {
                Operation::Isolation { cut: indices("cut")?, seeds: indices("seeds")?, direction: direction("both")? }
            }
            "allocation" => Operation::Allocation { sources: indices("sources")?, direction: direction("both")? },
            "trace" => Operation::Trace {
                seeds: indices("seeds")?,
                direction: direction("forward")?,
                budget: request.params["budget"].as_f64(),
            },
            other => return Err(Rejection::request(format!("unknown operation '{other}'"))),
        };
        let cost = match &request.params["cost"] {
            Value::Null => defaults.cost.clone(),
            Value::String(name) if name == "hops" => Cost::Hops,
            Value::String(name) if name == "length" => Cost::Length,
            Value::Object(fields) if fields.contains_key("seconds") => Cost::Seconds {
                metres_per_second: fields["seconds"]["metres_per_second"].as_f64().unwrap_or(13.9),
                speed_property: fields["seconds"]["speed_property"].as_str().map(str::to_string),
            },
            _ => return Err(Rejection::request("cost must be 'hops', 'length', or {seconds:{...}}".to_string())),
        };
        let parameters = Parameters {
            channel: request.params["channel"].as_str().unwrap_or(&defaults.channel).to_string(),
            operation,
            cost,
            step_seconds: request.params["step_seconds"].as_f64().unwrap_or(defaults.step_seconds as f64) as f32,
        };
        solve(&request.container, &parameters, request.t0_seconds, request.t1_seconds).map_err(Rejection::input)
    }
}
