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
        // A component label is not a progression: every edge carries its answer from the start.
        Operation::Components => {
            components(&graph).into_iter().enumerate().map(|(edge, label)| (edge, label as f64)).collect()
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
    if seeds.is_empty() {
        return Err("a trace needs at least one seed node".into());
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

const KNOWN: [&str; 7] = ["channel", "operation", "seeds", "direction", "budget", "cost", "step_seconds"];

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
        let operation = match request.params["operation"].as_str().unwrap_or("trace") {
            "components" => Operation::Components,
            "trace" => {
                let seeds = match &request.params["seeds"] {
                    Value::Array(values) => values
                        .iter()
                        .map(|value| value.as_u64().map(|index| index as usize).ok_or("seeds must be node indices"))
                        .collect::<Result<Vec<usize>, &str>>()
                        .map_err(Rejection::request)?,
                    Value::Null => return Err(Rejection::request("seeds is required for a trace".to_string())),
                    _ => return Err(Rejection::request("seeds must be an array".to_string())),
                };
                let direction = match request.params["direction"].as_str().unwrap_or("forward") {
                    "forward" => Direction::Forward,
                    "backward" => Direction::Backward,
                    "both" => Direction::Both,
                    other => return Err(Rejection::request(format!("unknown direction '{other}'"))),
                };
                Operation::Trace { seeds, direction, budget: request.params["budget"].as_f64() }
            }
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
