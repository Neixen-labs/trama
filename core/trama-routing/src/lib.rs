// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Shortest paths over a network, written into a state channel as a vehicle's progress.
//!
//! The second domain, and the first evidence that `GRPH`, `PROP` and state channels carry
//! something that is not water. Nothing here is hydraulic and nothing is road-specific either:
//! the solver knows edges have length and that some may only be crossed one way. What a road
//! is stays with whoever produced the container.

use std::collections::{BTreeMap, BinaryHeap};

pub mod fleet;

use trama_format::{Graph, edge_lengths, edge_properties, parse_graph, read_sections};
use trama_solver::server::{Rejection, Request, Solver};
use trama_solver::{declared, pack};

pub struct Parameters {
    pub channel: String,
    /// Node indices the route must visit, in order. At least two.
    pub waypoints: Vec<usize>,
    /// The travelling speed, and the fallback where `speed_property` names no usable value.
    pub speed_metres_per_second: f32,
    /// A `PROP` column holding each edge's own speed in metres per second. Naming one turns the
    /// search from shortest into fastest: cost becomes time, so a longer fast road can win.
    pub speed_property: Option<String>,
    /// A `PROP` column holding, for each edge, the ids of the edges it may not be followed by:
    /// a turn restriction, space-separated. Naming one makes the search refuse those movements.
    pub restriction_property: Option<String>,
    pub step_seconds: f32,
}

impl Default for Parameters {
    fn default() -> Self {
        Self {
            channel: "on_route".into(),
            waypoints: Vec::new(),
            // 50 km/h: a number that has to be something, and is a parameter for that reason.
            speed_metres_per_second: 13.9,
            speed_property: None,
            restriction_property: None,
            step_seconds: 60.0,
        }
    }
}

pub struct RoutingSolver;

const KNOWN: [&str; 6] =
    ["channel", "waypoints", "speed_metres_per_second", "speed_property", "restriction_property", "step_seconds"];

impl Solver for RoutingSolver {
    fn id(&self) -> &'static str {
        "shortest-path"
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
        let waypoints = match &request.params["waypoints"] {
            serde_json::Value::Array(values) => values
                .iter()
                .map(|value| value.as_u64().map(|index| index as usize).ok_or("waypoints must be node indices"))
                .collect::<Result<Vec<usize>, &str>>()
                .map_err(Rejection::request)?,
            serde_json::Value::Null => return Err(Rejection::request("waypoints is required".to_string())),
            _ => return Err(Rejection::request("waypoints must be an array".to_string())),
        };
        let parameters = Parameters {
            channel: request.params["channel"].as_str().unwrap_or(&defaults.channel).to_string(),
            waypoints,
            speed_metres_per_second: request.params["speed_metres_per_second"]
                .as_f64()
                .unwrap_or(defaults.speed_metres_per_second as f64) as f32,
            speed_property: request.params["speed_property"].as_str().map(str::to_string),
            restriction_property: request.params["restriction_property"].as_str().map(str::to_string),
            step_seconds: request.params["step_seconds"].as_f64().unwrap_or(defaults.step_seconds as f64) as f32,
        };
        solve(&request.container, &parameters, request.t0_seconds, request.t1_seconds).map_err(Rejection::input)
    }
}

/// The route: the edges crossed, in order, with the cost accumulated by the end of each.
pub struct Route {
    pub edges: Vec<usize>,
    /// The running total of `costs` at the far end of `edges[i]`, in whatever unit those were.
    pub reached_at: Vec<f64>,
}

/// The packed delta stream for the closed interval [t0, t1].
pub fn solve(container: &[u8], parameters: &Parameters, t0_seconds: f32, t1_seconds: f32) -> Result<Vec<u8>, String> {
    if t1_seconds < t0_seconds {
        return Err("t1_seconds must not precede t0_seconds".into());
    }
    if parameters.step_seconds <= 0.0 {
        return Err("step_seconds must be positive".into());
    }
    // NaN fails this too, which is the point: a speed that is not a positive number stalls silently.
    if !matches!(parameters.speed_metres_per_second.partial_cmp(&0.0), Some(std::cmp::Ordering::Greater)) {
        return Err("speed_metres_per_second must be positive".into());
    }
    let channel = declared(container, &parameters.channel, 2)?;
    let sections = read_sections(container)?;
    let graph = parse_graph(
        &sections.iter().find(|s| &s.kind == b"GRPH").ok_or("container is missing a GRPH section")?.payload,
    )?;
    // Seconds, so the search minimises travel time and `reached_at` is already a clock reading.
    // With one speed for every edge that is the same order as distance; with a speed column it
    // is not, and a longer fast road can win.
    let costs = traversal_seconds(container, &graph, parameters)?;
    let forbidden = forbidden_turns(container, &graph, parameters.restriction_property.as_deref())?;
    let route = plan(&graph, &costs, &forbidden, &parameters.waypoints)?;

    let steps = ((t1_seconds - t0_seconds) / parameters.step_seconds).floor() as i64 + 1;
    let mut records = Vec::new();
    for step in 0..steps {
        let t = t0_seconds + step as f32 * parameters.step_seconds;
        let elapsed = (t - t0_seconds) as f64;
        for (position, edge_index) in route.edges.iter().enumerate() {
            // Reached, not occupied: the channel shows how far the vehicle has got, so scrubbing
            // backwards unwinds the route rather than losing it.
            let value = if elapsed >= route.reached_at[position] { 1.0 } else { 0.0 };
            records.extend_from_slice(&pack(graph.edges[*edge_index].id, channel, t, value));
        }
    }
    Ok(records)
}

/// How long each edge takes to cross, in seconds.
///
/// Without `speed_property` every edge moves at the same speed, so this is distance in disguise
/// and the fastest route is the shortest one. With it, each edge uses its own column and the two
/// stop agreeing. A row with no usable number falls back to the parameter rather than failing:
/// half a real city has no speed limit tagged, and refusing to route it would be useless.
fn traversal_seconds(container: &[u8], graph: &Graph, parameters: &Parameters) -> Result<Vec<f64>, String> {
    let lengths = edge_lengths(container)?;
    let fallback = parameters.speed_metres_per_second as f64;
    let Some(key) = &parameters.speed_property else {
        return Ok(lengths.iter().map(|length| length / fallback).collect());
    };
    let rows = edge_properties(container)?;
    Ok(graph
        .edges
        .iter()
        .zip(&lengths)
        .map(|(edge, length)| {
            let speed = rows
                .get(edge.property_row as usize)
                .and_then(|row| row.get(key))
                .and_then(serde_json::Value::as_f64)
                .filter(|speed| speed.is_finite() && *speed > 0.0)
                .unwrap_or(fallback);
            length / speed
        })
        .collect())
}

/// The forbidden runs of edges named by a `PROP` column, ready for the search to walk.
///
/// The reading lives in `trama_format::Turns` rather than here: `trama-trace` asks the same
/// question of the same column, and a run automaton copied into two crates is one that can come to
/// disagree about which movements exist over a single file.
pub(crate) fn forbidden_turns(container: &[u8], graph: &Graph, key: Option<&str>) -> Result<Turns, String> {
    Turns::read(container, graph, key)
}

/// Every edge's length in metres, from the geometry the container already stores.
///
/// The cheapest walk visiting every waypoint in order, under whatever `costs` measures.
pub fn plan(graph: &Graph, costs: &[f64], forbidden: &Turns, waypoints: &[usize]) -> Result<Route, String> {
    if waypoints.len() < 2 {
        return Err("a route needs at least two waypoints".into());
    }
    if let Some(missing) = waypoints.iter().find(|index| **index >= graph.nodes.len()) {
        return Err(format!("waypoint {missing} names no node"));
    }
    let mut edges = Vec::new();
    let mut reached_at = Vec::new();
    let mut covered = 0.0;
    for leg in waypoints.windows(2) {
        for edge_index in shortest_path(graph, costs, forbidden, leg[0], leg[1])? {
            covered += costs[edge_index];
            edges.push(edge_index);
            reached_at.push(covered);
        }
    }
    Ok(Route { edges, reached_at })
}

/// Dijkstra over the CSR adjacency, returning the edges crossed from `from` to `to`.
///
/// Direction needs no special case here. SPEC 4 gives a directed edge one CSR entry, at its
/// source, so an edge that may not be crossed backwards simply does not appear among what leaves
/// its target — the topology states the restriction and the search cannot violate it.
fn shortest_path(
    graph: &Graph,
    costs: &[f64],
    forbidden: &Turns,
    from: usize,
    to: usize,
) -> Result<Vec<usize>, String> {
    let search = explore(graph, costs, forbidden, from, Some(to));
    if !search.cost.contains_key(&(to as u32)) {
        return Err(format!("no route from node {from} to node {to}"));
    }
    Ok(search.retrace(to))
}

pub use trama_format::Turns;

/// A completed search: what each node cost to reach, and the arc it was reached by.
///
/// "Arc" is one entry of the CSR adjacency — an edge together with the direction it was crossed
/// in — and it, not the node, is what the search settles. With turn restrictions a node is no
/// longer a state: arriving at a junction along one street and along another are different
/// situations, because they permit different exits. Two searches that agree on the cheapest way
/// to a node can disagree on whether the road out of it is open.
pub(crate) struct Search {
    /// Cheapest cost to each node, over every walk that reaches it.
    pub cost: BTreeMap<u32, u64>,
    /// The walk each node was most cheaply reached by.
    pub arrival: BTreeMap<u32, Walk>,
    /// For each walk: the edge crossed, and the walk it continued. `NOWHERE` is the start.
    came_from: BTreeMap<Walk, (usize, Walk)>,
}

/// Where a search stands: the arc it is on, and how far along the forbidden runs it has got.
///
/// The second half is what makes a run longer than a turn expressible. Arriving on one arc having
/// just crossed the link between two carriageways is a different situation from arriving on the
/// same arc off an ordinary street, because one of them may not turn back and the other may. With
/// no runs declared the automaton has a single state, every walk carries the same zero, and this
/// is the arc-settling search it was before — the general case costs the common case nothing.
pub(crate) type Walk = (u32, trama_format::Progress);

/// The walk before the first one, which no arc continues.
const NOWHERE: Walk = (u32::MAX, 0);

impl Search {
    /// The edges of the path to `to`, in order.
    ///
    /// Walked backwards along walks rather than nodes. Retracing by node would be free to pick a
    /// different arrival at each junction than the search actually used, and under a restriction
    /// that is not merely a different path of the same cost — it is a path through a movement the
    /// search refused to make.
    pub fn retrace(&self, to: usize) -> Vec<usize> {
        let mut path = Vec::new();
        let mut walk = self.arrival.get(&(to as u32)).copied();
        while let Some(current) = walk.filter(|walk| *walk != NOWHERE) {
            let (edge_index, previous) = self.came_from[&current];
            path.push(edge_index);
            walk = Some(previous);
        }
        path.reverse();
        path
    }
}

/// Dijkstra over the CSR adjacency from `from`, stopping early at `until` when one is given.
///
/// Two callers want different things from the same search: a single route wants the path to one
/// node and can stop as soon as it settles, while a fleet wants the distance to every stop and
/// must run the queue dry. `until` is that difference and nothing else is.
///
/// One-way needs no special case. SPEC 4 gives a directed edge one CSR entry, at its source, so
/// an edge that may not be crossed backwards simply does not appear among what leaves its target
/// — the topology states the restriction and the search cannot violate it. A restriction is the
/// opposite: nothing in the topology forbids it, because every edge in the run is perfectly
/// crossable and it is only the succession that is not. So it is checked on each step, against the
/// automaton the walk carries.
pub(crate) fn explore(graph: &Graph, costs: &[f64], forbidden: &Turns, from: usize, until: Option<usize>) -> Search {
    // Scaled to integers so the queue can order them: a thousandth of the unit, which for
    // seconds is a millisecond and for metres a millimetre. The geometry is quantized to about
    // 4 cm, so neither reading discards anything that was ever there.
    let mut search =
        Search { cost: BTreeMap::from([(from as u32, 0)]), arrival: BTreeMap::new(), came_from: BTreeMap::new() };
    let mut best: BTreeMap<Walk, u64> = BTreeMap::new();
    let mut queue = BinaryHeap::new();

    // The arcs leaving the origin. Nothing was crossed before them, so each starts the automaton
    // from scratch: a vehicle that begins here has not driven the edge a run would begin with.
    let leaving = |node: usize| graph.csr_offsets[node] as usize..graph.csr_offsets[node + 1] as usize;
    for arc in leaving(from) {
        let entry = &graph.adjacency[arc];
        let Some(progress) = forbidden.advance(Turns::START, entry.edge_index as usize) else {
            continue;
        };
        let walk = (arc as u32, progress);
        let step = (costs[entry.edge_index as usize] * 1000.0).round() as u64;
        if step < *best.get(&walk).unwrap_or(&u64::MAX) {
            best.insert(walk, step);
            search.came_from.insert(walk, (entry.edge_index as usize, NOWHERE));
            queue.push((std::cmp::Reverse(step), walk));
        }
    }

    while let Some((std::cmp::Reverse(cost), walk)) = queue.pop() {
        // A stale queue entry for a walk already settled more cheaply.
        if cost > *best.get(&walk).unwrap_or(&u64::MAX) {
            continue;
        }
        let (arc, progress) = walk;
        let entry = &graph.adjacency[arc as usize];
        let edge = &graph.edges[entry.edge_index as usize];
        let node = if entry.traversal_direction > 0 { edge.target } else { edge.source };
        // The cheapest walk into a node is the cheapest way to the node. The origin sits at zero
        // and stays there: no walk can return to it for less than nothing.
        if cost < *search.cost.get(&node).unwrap_or(&u64::MAX) {
            search.cost.insert(node, cost);
            search.arrival.insert(node, walk);
        }
        if Some(node as usize) == until {
            break;
        }
        for next in leaving(node as usize) {
            let onward = &graph.adjacency[next];
            // The movement itself: this run of edges, ending with that one, is what a restriction
            // names. `None` is the step the run forbids.
            let Some(onward_progress) = forbidden.advance(progress, onward.edge_index as usize) else {
                continue;
            };
            let onward_walk = (next as u32, onward_progress);
            let step = (costs[onward.edge_index as usize] * 1000.0).round() as u64;
            let candidate = cost + step;
            if candidate < *best.get(&onward_walk).unwrap_or(&u64::MAX) {
                best.insert(onward_walk, candidate);
                search.came_from.insert(onward_walk, (onward.edge_index as usize, walk));
                queue.push((std::cmp::Reverse(candidate), onward_walk));
            }
        }
    }

    search
}
