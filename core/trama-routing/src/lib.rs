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

/// Which edges each edge may not be followed by, read from a `PROP` column of stable ids.
///
/// The column holds ids rather than indices because an id is what the file is addressed by and
/// what survives a recompilation; an index is an artefact of this reader's own ordering. So the
/// translation happens once, here, and the search works in indices from then on.
///
/// An id naming no edge in this container is skipped rather than refused. A restriction can point
/// at a street outside the extract — the relation is one object and the bounding box cut through
/// it — and that is a fact about the clip, not a corrupt file. It costs nothing to ignore: an
/// edge that is not here is one the search could never have taken anyway.
fn forbidden_turns(container: &[u8], graph: &Graph, key: Option<&str>) -> Result<Turns, String> {
    let Some(key) = key else {
        return Ok(Turns::new());
    };
    let rows = edge_properties(container)?;
    let by_id: BTreeMap<u64, usize> = graph.edges.iter().enumerate().map(|(index, edge)| (edge.id, index)).collect();
    let mut turns = Turns::new();
    for (index, edge) in graph.edges.iter().enumerate() {
        let Some(listed) = rows.get(edge.property_row as usize).and_then(|row| row.get(key)).and_then(|v| v.as_str())
        else {
            continue;
        };
        let closed: Vec<usize> = listed
            .split_whitespace()
            .filter_map(|id| id.parse().ok())
            .filter_map(|id| by_id.get(&id))
            .copied()
            .collect();
        if !closed.is_empty() {
            turns.insert(index, closed);
        }
    }
    Ok(turns)
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

/// Which edges an edge forbids continuing onto, by edge index. Empty when nobody asked.
///
/// A turn restriction is a fact about a *pair* of edges, which is why it cannot live on either
/// one alone as a cost or a flag. This crate learns it as a relation between indices and nothing
/// more: what a left turn is, and which tag spells it, stays with whoever produced the container.
pub type Turns = BTreeMap<usize, Vec<usize>>;

/// A completed search: what each node cost to reach, and the arc it was reached by.
///
/// "Arc" is one entry of the CSR adjacency — an edge together with the direction it was crossed
/// in — and it, not the node, is what the search settles. With turn restrictions a node is no
/// longer a state: arriving at a junction along one street and along another are different
/// situations, because they permit different exits. Two searches that agree on the cheapest way
/// to a node can disagree on whether the road out of it is open.
pub(crate) struct Search {
    /// Cheapest cost to each node, over every arc that reaches it.
    pub cost: BTreeMap<u32, u64>,
    /// The arc each node was most cheaply reached by.
    pub arrival: BTreeMap<u32, u32>,
    /// For each arc: the edge crossed, and the arc crossed before it. `u32::MAX` is the start.
    came_from: BTreeMap<u32, (usize, u32)>,
}

impl Search {
    /// The edges of the path to `to`, in order.
    ///
    /// Walked backwards along arcs rather than nodes. Retracing by node would be free to pick a
    /// different arc into each junction than the search actually used, and under a turn
    /// restriction that is not merely a different path of the same cost — it is a path through a
    /// turn the search refused to take.
    pub fn retrace(&self, to: usize) -> Vec<usize> {
        let mut path = Vec::new();
        let mut arc = self.arrival.get(&(to as u32)).copied();
        while let Some(current) = arc.filter(|arc| *arc != u32::MAX) {
            let (edge_index, previous) = self.came_from[&current];
            path.push(edge_index);
            arc = Some(previous);
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
/// — the topology states the restriction and the search cannot violate it. A turn restriction is
/// the opposite: nothing in the topology forbids it, because both edges are perfectly crossable
/// and it is only the pair that is not. So it is checked on the step between them.
pub(crate) fn explore(graph: &Graph, costs: &[f64], forbidden: &Turns, from: usize, until: Option<usize>) -> Search {
    // Scaled to integers so the queue can order them: a thousandth of the unit, which for
    // seconds is a millisecond and for metres a millimetre. The geometry is quantized to about
    // 4 cm, so neither reading discards anything that was ever there.
    let mut search =
        Search { cost: BTreeMap::from([(from as u32, 0)]), arrival: BTreeMap::new(), came_from: BTreeMap::new() };
    let mut best_arc: BTreeMap<u32, u64> = BTreeMap::new();
    let mut queue = BinaryHeap::new();

    // The arcs leaving the origin. There is no arc before them, so no turn to check.
    let leaving = |node: usize| graph.csr_offsets[node] as usize..graph.csr_offsets[node + 1] as usize;
    for arc in leaving(from) {
        let entry = &graph.adjacency[arc];
        let step = (costs[entry.edge_index as usize] * 1000.0).round() as u64;
        if step < *best_arc.get(&(arc as u32)).unwrap_or(&u64::MAX) {
            best_arc.insert(arc as u32, step);
            search.came_from.insert(arc as u32, (entry.edge_index as usize, u32::MAX));
            queue.push((std::cmp::Reverse(step), arc as u32));
        }
    }

    while let Some((std::cmp::Reverse(cost), arc)) = queue.pop() {
        // A stale queue entry for an arc already settled more cheaply.
        if cost > *best_arc.get(&arc).unwrap_or(&u64::MAX) {
            continue;
        }
        let entry = &graph.adjacency[arc as usize];
        let edge = &graph.edges[entry.edge_index as usize];
        let node = if entry.traversal_direction > 0 { edge.target } else { edge.source };
        // The cheapest arc into a node is the cheapest way to the node. The origin sits at zero
        // and stays there: no arc can return to it for less than nothing.
        if cost < *search.cost.get(&node).unwrap_or(&u64::MAX) {
            search.cost.insert(node, cost);
            search.arrival.insert(node, arc);
        }
        if Some(node as usize) == until {
            break;
        }
        let closed = forbidden.get(&(entry.edge_index as usize));
        for next in leaving(node as usize) {
            let onward = &graph.adjacency[next];
            // The turn itself: crossing this edge and then that one is what a restriction names.
            if closed.is_some_and(|closed| closed.contains(&(onward.edge_index as usize))) {
                continue;
            }
            let step = (costs[onward.edge_index as usize] * 1000.0).round() as u64;
            let candidate = cost + step;
            if candidate < *best_arc.get(&(next as u32)).unwrap_or(&u64::MAX) {
                best_arc.insert(next as u32, candidate);
                search.came_from.insert(next as u32, (onward.edge_index as usize, arc));
                queue.push((std::cmp::Reverse(candidate), next as u32));
            }
        }
    }

    search
}
