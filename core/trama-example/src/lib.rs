// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! A pulse diffusing outward over a graph's own topology.
//!
//! Deliberately domain-agnostic: the only thing it knows is that edges connect nodes. It
//! exists to put `docs/SOLVER_CONTRACT.md` under a real implementation, not to model anything.

use std::collections::{BTreeMap, VecDeque};

use trama_format::{parse_graph, read_sections};
use trama_solver::server::{Rejection, Request, Solver};
use trama_solver::{declared, pack};

pub struct Parameters {
    pub channel: String,
    pub seed_node_index: usize,
    pub step_seconds: f32,
    pub speed_hops_per_step: f32,
    pub amplitude: f32,
}

impl Default for Parameters {
    fn default() -> Self {
        Self {
            channel: "flow".into(),
            seed_node_index: 0,
            step_seconds: 60.0,
            speed_hops_per_step: 1.5,
            amplitude: 40.0,
        }
    }
}

pub struct ExampleSolver;

impl Solver for ExampleSolver {
    fn id(&self) -> &'static str {
        "example-diffusion"
    }

    fn contract_versions(&self) -> &'static [&'static str] {
        &["0.1.0", "0.2.0"]
    }

    fn solve(&self, request: &Request) -> Result<Vec<u8>, Rejection> {
        let defaults = Parameters::default();
        if let Some(unknown) = request.params.as_object().and_then(|params| {
            params
                .keys()
                .find(|key| {
                    !["channel", "seed_node_index", "step_seconds", "speed_hops_per_step", "amplitude"]
                        .contains(&key.as_str())
                })
                .cloned()
        }) {
            return Err(Rejection::request(format!("unknown parameters: {unknown}")));
        }
        let parameters = Parameters {
            channel: request.params["channel"].as_str().unwrap_or(&defaults.channel).to_string(),
            seed_node_index: request.params["seed_node_index"].as_u64().unwrap_or(0) as usize,
            step_seconds: request.params["step_seconds"].as_f64().unwrap_or(defaults.step_seconds as f64) as f32,
            speed_hops_per_step: request.params["speed_hops_per_step"]
                .as_f64()
                .unwrap_or(defaults.speed_hops_per_step as f64) as f32,
            amplitude: request.params["amplitude"].as_f64().unwrap_or(defaults.amplitude as f64) as f32,
        };
        solve(&request.container, &parameters, request.t0_seconds, request.t1_seconds).map_err(Rejection::input)
    }
}

/// The packed delta stream for the closed interval [t0, t1].
pub fn solve(container: &[u8], parameters: &Parameters, t0_seconds: f32, t1_seconds: f32) -> Result<Vec<u8>, String> {
    if t1_seconds < t0_seconds {
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
    if graph.nodes.is_empty() || graph.edges.is_empty() {
        return Err("solver requires a container with nodes and edges".into());
    }
    if parameters.seed_node_index >= graph.nodes.len() {
        return Err(format!("seed_node_index {} names no node", parameters.seed_node_index));
    }

    let hops = hops_from_seed(graph.nodes.len(), &graph.edges, parameters.seed_node_index);
    let steps = ((t1_seconds - t0_seconds) / parameters.step_seconds).floor() as i64 + 1;
    let mut records = Vec::new();
    for step in 0..steps {
        let t = t0_seconds + step as f32 * parameters.step_seconds;
        let front = step as f32 * parameters.speed_hops_per_step;
        for edge in &graph.edges {
            // A component the pulse never reaches contributes nothing rather than a zero.
            let reachable = [edge.source, edge.target].iter().filter_map(|index| hops.get(index)).min().copied();
            let Some(distance) = reachable else { continue };
            records.extend_from_slice(&pack(
                edge.id,
                channel,
                t,
                parameters.amplitude * pulse(front - distance as f32),
            ));
        }
    }
    Ok(records)
}

/// A unit-height bump; a wave crest passing an edge rather than a step change.
fn pulse(offset: f32) -> f32 {
    (-(offset * offset) / 2.0).exp()
}

fn hops_from_seed(node_count: usize, edges: &[trama_format::Edge], seed: usize) -> BTreeMap<u32, u32> {
    let mut neighbours: Vec<Vec<u32>> = vec![Vec::new(); node_count];
    for edge in edges {
        neighbours[edge.source as usize].push(edge.target);
        neighbours[edge.target as usize].push(edge.source);
    }
    let mut hops: BTreeMap<u32, u32> = BTreeMap::new();
    hops.insert(seed as u32, 0);
    let mut queue = VecDeque::from([seed as u32]);
    while let Some(index) = queue.pop_front() {
        let distance = hops[&index];
        for neighbour in &neighbours[index as usize] {
            if !hops.contains_key(neighbour) {
                hops.insert(*neighbour, distance + 1);
                queue.push_back(*neighbour);
            }
        }
    }
    hops
}
