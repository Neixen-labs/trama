// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Shortest paths over a network, written into a state channel as a vehicle's progress.
//!
//! The second domain, and the first evidence that `GRPH`, `PROP` and state channels carry
//! something that is not water. Nothing here is hydraulic and nothing is road-specific either:
//! the solver knows edges have length and that some may only be crossed one way. What a road
//! is stays with whoever produced the container.

use std::collections::{BTreeMap, BinaryHeap};

use trama_format::{Graph, edge_paths, parse_graph, read_sections};
use trama_solver::server::{Rejection, Request, Solver};
use trama_solver::{declared, pack};

pub struct Parameters {
    pub channel: String,
    /// Node indices the route must visit, in order. At least two.
    pub waypoints: Vec<usize>,
    pub speed_metres_per_second: f32,
    pub step_seconds: f32,
}

impl Default for Parameters {
    fn default() -> Self {
        Self {
            channel: "on_route".into(),
            waypoints: Vec::new(),
            // 50 km/h: a number that has to be something, and is a parameter for that reason.
            speed_metres_per_second: 13.9,
            step_seconds: 60.0,
        }
    }
}

pub struct RoutingSolver;

const KNOWN: [&str; 4] = ["channel", "waypoints", "speed_metres_per_second", "step_seconds"];

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
            step_seconds: request.params["step_seconds"].as_f64().unwrap_or(defaults.step_seconds as f64) as f32,
        };
        solve(&request.container, &parameters, request.t0_seconds, request.t1_seconds).map_err(Rejection::input)
    }
}

/// One leg of the route: the edges crossed, in order, with the distance covered by the end of each.
pub struct Route {
    pub edges: Vec<usize>,
    /// Metres from the start of the route to the far end of `edges[i]`.
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
    let lengths = lengths_of(container)?;
    let route = plan(&graph, &lengths, &parameters.waypoints)?;

    let steps = ((t1_seconds - t0_seconds) / parameters.step_seconds).floor() as i64 + 1;
    let mut records = Vec::new();
    for step in 0..steps {
        let t = t0_seconds + step as f32 * parameters.step_seconds;
        let travelled = (t - t0_seconds) as f64 * parameters.speed_metres_per_second as f64;
        for (position, edge_index) in route.edges.iter().enumerate() {
            // Reached, not occupied: the channel shows how far the vehicle has got, so scrubbing
            // backwards unwinds the route rather than losing it.
            let value = if travelled >= route.reached_at[position] { 1.0 } else { 0.0 };
            records.extend_from_slice(&pack(graph.edges[*edge_index].id, channel, t, value));
        }
    }
    Ok(records)
}

/// Every edge's length in metres, from the geometry the container already stores.
///
/// ponytail: length is the only cost. A road network wanting travel time wants a `PROP` column
/// and a parameter naming it; that needs a container carrying one to be worth designing against.
fn lengths_of(container: &[u8]) -> Result<Vec<f64>, String> {
    Ok(edge_paths(container)?
        .iter()
        .map(|path| path.windows(2).map(|pair| (pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1)).sum())
        .collect())
}

/// The shortest walk visiting every waypoint in order.
pub fn plan(graph: &Graph, lengths: &[f64], waypoints: &[usize]) -> Result<Route, String> {
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
        for edge_index in shortest_path(graph, lengths, leg[0], leg[1])? {
            covered += lengths[edge_index];
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
fn shortest_path(graph: &Graph, lengths: &[f64], from: usize, to: usize) -> Result<Vec<usize>, String> {
    // Costs are millimetres so the queue can order them as integers. The geometry is quantized to
    // about 4 cm, so this discards nothing that was ever there.
    let mut best: BTreeMap<u32, u64> = BTreeMap::from([(from as u32, 0)]);
    let mut came_from: BTreeMap<u32, (usize, u32)> = BTreeMap::new();
    let mut queue = BinaryHeap::from([(std::cmp::Reverse(0u64), from as u32)]);

    while let Some((std::cmp::Reverse(cost), node)) = queue.pop() {
        if node as usize == to {
            break;
        }
        // A stale queue entry for a node already settled more cheaply.
        if cost > *best.get(&node).unwrap_or(&u64::MAX) {
            continue;
        }
        let start = graph.csr_offsets[node as usize] as usize;
        let end = graph.csr_offsets[node as usize + 1] as usize;
        for entry in &graph.adjacency[start..end] {
            let edge = &graph.edges[entry.edge_index as usize];
            let neighbour = if entry.traversal_direction > 0 { edge.target } else { edge.source };
            let step = (lengths[entry.edge_index as usize] * 1000.0).round() as u64;
            let candidate = cost + step;
            if candidate < *best.get(&neighbour).unwrap_or(&u64::MAX) {
                best.insert(neighbour, candidate);
                came_from.insert(neighbour, (entry.edge_index as usize, node));
                queue.push((std::cmp::Reverse(candidate), neighbour));
            }
        }
    }

    if !best.contains_key(&(to as u32)) {
        return Err(format!("no route from node {from} to node {to}"));
    }
    let mut path = Vec::new();
    let mut node = to as u32;
    while node as usize != from {
        let (edge_index, previous) = came_from[&node];
        path.push(edge_index);
        node = previous;
    }
    path.reverse();
    Ok(path)
}
