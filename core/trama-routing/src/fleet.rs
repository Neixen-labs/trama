// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Several vehicles, one depot, a load at every stop: the capacitated vehicle routing problem.
//!
//! Routing one vehicle through waypoints in a given order is a shortest path. Deciding *which*
//! vehicle serves *which* stops, and in what order, is not: it is NP-hard, and the number of ways
//! to split twenty stops between four vans is past counting. So this does not claim an optimum. It
//! builds the Clarke-Wright savings solution and improves each route by 2-opt until neither moves,
//! which is the classical construction — old, deterministic, and understood well enough that its
//! failure modes are known rather than surprising.
//!
//! What it does guarantee is checkable, and `tests/fleet.rs` checks it: every stop served exactly
//! once, no vehicle over its capacity, every route leaving and returning to the depot, and a total
//! never worse than serving each stop on its own trip. On instances small enough to enumerate, it
//! is also compared against the true optimum — which is how the gap stops being a matter of faith.

use std::collections::BTreeMap;

use serde_json::Value;
use trama_format::{Graph, parse_graph, read_sections};
use trama_solver::declared;
use trama_solver::pack;
use trama_solver::server::{Rejection, Request, Solver};

use crate::{Parameters, Turns, explore};

pub struct Fleet {
    /// The node every vehicle starts and ends at.
    pub depot: usize,
    /// Node indices to serve. Each appears in exactly one route.
    pub stops: Vec<usize>,
    /// What each stop takes out of a vehicle, in the caller's own unit — kilograms, parcels,
    /// minutes of work. The solver never interprets it beyond adding it up.
    pub demands: Vec<f64>,
    pub capacity: f64,
    /// The most vehicles available. Fewer are used when fewer suffice.
    pub vehicles: usize,
}

/// One vehicle's work: the stops it serves in order, and the edges it crosses to do it.
pub struct Assignment {
    pub stops: Vec<usize>,
    pub edges: Vec<usize>,
    /// The running total of `costs` at the far end of each edge.
    pub reached_at: Vec<f64>,
    pub load: f64,
}

/// Distances between every pair of interesting nodes, and the paths behind them.
///
/// The depot is index 0 and stop `k` is index `k + 1`, so the matrix is one bigger than the stop
/// list in each direction. Asymmetric on purpose: a one-way street makes the trip out and the trip
/// back different journeys, and a matrix that assumed otherwise would quietly plan the impossible.
struct Distances {
    cost: Vec<Vec<f64>>,
    path: Vec<Vec<Vec<usize>>>,
}

impl Distances {
    fn build(graph: &Graph, costs: &[f64], forbidden: &Turns, points: &[usize]) -> Result<Distances, String> {
        let mut cost = vec![vec![f64::INFINITY; points.len()]; points.len()];
        let mut path = vec![vec![Vec::new(); points.len()]; points.len()];
        for (from, origin) in points.iter().enumerate() {
            let search = explore(graph, costs, forbidden, *origin, None);
            for (to, destination) in points.iter().enumerate() {
                if from == to {
                    cost[from][to] = 0.0;
                    continue;
                }
                if !search.cost.contains_key(&(*destination as u32)) {
                    return Err(format!(
                        "no route from node {origin} to node {destination}; a fleet cannot serve a \
                         stop it cannot reach"
                    ));
                }
                let legs = search.retrace(*destination);
                // Summed from the costs themselves, not read off the queue: the search orders its
                // frontier by a value scaled to thousandths, and a distance rebuilt from that is
                // out by the rounding of every edge it crossed. Small, and enough to make this
                // matrix disagree with the single-route planner over the same path.
                cost[from][to] = legs.iter().map(|edge| costs[*edge]).sum();
                path[from][to] = legs;
            }
        }
        Ok(Distances { cost, path })
    }
}

/// Assign the stops to vehicles and order each one's round.
pub fn plan(graph: &Graph, costs: &[f64], forbidden: &Turns, fleet: &Fleet) -> Result<Vec<Assignment>, String> {
    if fleet.stops.is_empty() {
        return Err("a fleet needs at least one stop to serve".into());
    }
    if fleet.demands.len() != fleet.stops.len() {
        return Err(format!(
            "{} stops carry {} demands; there must be one for each",
            fleet.stops.len(),
            fleet.demands.len()
        ));
    }
    if fleet.vehicles == 0 {
        return Err("a fleet needs at least one vehicle".into());
    }
    if let Some(over) = fleet.demands.iter().position(|demand| *demand > fleet.capacity) {
        return Err(format!(
            "stop {} demands {} against a capacity of {}, so no vehicle can serve it",
            over, fleet.demands[over], fleet.capacity
        ));
    }
    if fleet.demands.iter().any(|demand| *demand < 0.0 || !demand.is_finite()) {
        return Err("a demand must be a finite, non-negative number".into());
    }

    let mut points = vec![fleet.depot];
    points.extend(&fleet.stops);
    let distances = Distances::build(graph, costs, forbidden, &points)?;

    let mut routes = savings(&distances, fleet);
    if routes.len() > fleet.vehicles {
        return Err(format!(
            "these stops need {} vehicles at this capacity and the fleet has {}",
            routes.len(),
            fleet.vehicles
        ));
    }
    for route in routes.iter_mut() {
        two_opt(&distances, route);
    }
    Ok(routes.into_iter().map(|stops| assemble(&distances, costs, fleet, stops)).collect())
}

/// Clarke-Wright: start with one round per stop, then merge the pairs that save the most.
///
/// The saving of putting `i` and `j` on one round is `d(depot,i) + d(j,depot) − d(i,j)`: the two
/// legs to the depot that stop being driven, less the leg that starts being. Merging in descending
/// order of that is the whole algorithm. Only the ends of routes may be joined, because a stop in
/// the middle of a round already has both its neighbours.
fn savings(distances: &Distances, fleet: &Fleet) -> Vec<Vec<usize>> {
    let n = fleet.stops.len();
    // Routes are held as stop indices, 0-based into `fleet.stops`; the matrix offsets by one.
    let mut routes: Vec<Vec<usize>> = (0..n).map(|stop| vec![stop]).collect();
    let mut load: Vec<f64> = fleet.demands.clone();
    // Which route each stop is in, so a merge can find them without scanning.
    let mut owner: Vec<usize> = (0..n).collect();

    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for i in 0..n {
        for j in 0..n {
            if i != j {
                let saving = distances.cost[0][i + 1] + distances.cost[j + 1][0] - distances.cost[i + 1][j + 1];
                pairs.push((saving, i, j));
            }
        }
    }
    // Descending by saving, then by index: ties must not depend on the order the pairs were
    // generated in, or the same input would plan differently on a different day.
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));

    for (saving, i, j) in pairs {
        if saving <= 0.0 {
            break;
        }
        let (from, to) = (owner[i], owner[j]);
        // Same route already, or a merge that would exceed what one vehicle can carry.
        if from == to || load[from] + load[to] > fleet.capacity {
            continue;
        }
        // `i` must be at the end of its route and `j` at the start of its, since the merge puts
        // the second straight after the first.
        if routes[from].last() != Some(&i) || routes[to].first() != Some(&j) {
            continue;
        }
        let moved = std::mem::take(&mut routes[to]);
        for stop in &moved {
            owner[*stop] = from;
        }
        routes[from].extend(moved);
        load[from] += load[to];
        load[to] = 0.0;
    }
    routes.retain(|route| !route.is_empty());
    routes
}

/// Reverse any run of a route that shortens it, until none does.
///
/// The classical 2-opt move on a round trip. It repairs the crossings the savings pass leaves
/// behind, which are the cheapest kind of mistake to find: a route that crosses itself is always
/// improved by reversing the section between the crossings.
fn two_opt(distances: &Distances, route: &mut Vec<usize>) {
    let leg = |a: usize, b: usize| distances.cost[a][b];
    // The depot at both ends, as matrix indices, so the ends are ordinary moves.
    let at = |route: &Vec<usize>, position: usize| {
        if position == 0 || position > route.len() { 0 } else { route[position - 1] + 1 }
    };
    let mut improved = true;
    while improved {
        improved = false;
        for i in 1..=route.len() {
            for j in i + 1..=route.len() {
                let (a, b, c, d) = (at(route, i - 1), at(route, i), at(route, j), at(route, j + 1));
                // Reversing i..=j replaces the two legs entering and leaving that run. On a
                // directed network the run's own legs reverse too, so the whole thing is priced
                // rather than just the ends: a one-way street makes the two directions differ.
                let before = leg(a, b) + leg(c, d) + (i..j).map(|k| leg(at(route, k), at(route, k + 1))).sum::<f64>();
                let after = leg(a, c) + leg(b, d) + (i..j).map(|k| leg(at(route, k + 1), at(route, k))).sum::<f64>();
                if after + 1e-9 < before {
                    route[i - 1..j].reverse();
                    improved = true;
                }
            }
        }
    }
}

/// One route's stops into the edges a vehicle actually drives, with the clock at each.
fn assemble(distances: &Distances, costs: &[f64], fleet: &Fleet, stops: Vec<usize>) -> Assignment {
    let mut edges = Vec::new();
    let mut reached_at = Vec::new();
    let mut elapsed = 0.0;
    let mut previous = 0;
    for stop in stops.iter().chain(std::iter::once(&usize::MAX)) {
        // The sentinel is the leg home: every round ends where it started.
        let next = if *stop == usize::MAX { 0 } else { stop + 1 };
        for edge in &distances.path[previous][next] {
            // Each edge's own cost, so the clock advances the way the vehicle does rather than
            // jumping from stop to stop.
            elapsed += costs[*edge];
            edges.push(*edge);
            reached_at.push(elapsed);
        }
        previous = next;
    }
    let load = stops.iter().map(|stop| fleet.demands[*stop]).sum();
    Assignment { stops, edges, reached_at, load }
}

/// What a plan costs in total, for comparing one against another.
pub fn total_cost(assignments: &[Assignment]) -> f64 {
    assignments.iter().filter_map(|assignment| assignment.reached_at.last()).sum()
}

/// The stops each vehicle serves, for a caller that wants the assignment rather than the drawing.
pub fn manifest(assignments: &[Assignment]) -> BTreeMap<usize, Vec<usize>> {
    assignments.iter().enumerate().map(|(vehicle, assignment)| (vehicle, assignment.stops.clone())).collect()
}

/// The fleet planner behind the solver contract.
///
/// Its own solver rather than another parameter of `shortest-path`, and its own manifest beside
/// it: the two take different questions. A route is told where to go and in what order; a fleet is
/// told what must be served and works the order out. Folding them together would mean a manifest
/// whose parameters are mutually exclusive, which is a schema saying "this is really two things".
pub struct FleetSolver;

const KNOWN: [&str; 8] =
    ["channel", "depot", "stops", "demands", "capacity", "vehicles", "speed_property", "restriction_property"];

impl Solver for FleetSolver {
    fn id(&self) -> &'static str {
        "fleet-routing"
    }

    fn contract_versions(&self) -> &'static [&'static str] {
        &["0.1.0", "0.2.0"]
    }

    fn solve(&self, request: &Request) -> Result<Vec<u8>, Rejection> {
        if let Some(unknown) =
            request.params.as_object().and_then(|params| params.keys().find(|key| !KNOWN.contains(&key.as_str())))
        {
            return Err(Rejection::request(format!("unknown parameter: {unknown}")));
        }
        let indices = |key: &str| -> Result<Vec<usize>, Rejection> {
            match &request.params[key] {
                Value::Array(values) => values
                    .iter()
                    .map(|value| {
                        value
                            .as_u64()
                            .map(|index| index as usize)
                            .ok_or_else(|| Rejection::request(format!("{key} holds node indices")))
                    })
                    .collect(),
                Value::Null => Err(Rejection::request(format!("{key} is required"))),
                _ => Err(Rejection::request(format!("{key} is an array"))),
            }
        };
        let stops = indices("stops")?;
        let depot = request.params["depot"]
            .as_u64()
            .map(|index| index as usize)
            .ok_or_else(|| Rejection::request("depot is the node index every vehicle starts from"))?;
        let demands = match &request.params["demands"] {
            // Absent means one unit each, which turns capacity into a count of stops per round —
            // the shape of the problem when what is being delivered is visits rather than weight.
            Value::Null => vec![1.0; stops.len()],
            Value::Array(values) => values
                .iter()
                .map(|value| value.as_f64().ok_or_else(|| Rejection::request("demands holds numbers")))
                .collect::<Result<Vec<f64>, Rejection>>()?,
            _ => return Err(Rejection::request("demands is an array of numbers")),
        };
        let fleet = Fleet {
            depot,
            stops,
            demands,
            capacity: request.params["capacity"].as_f64().unwrap_or(f64::INFINITY),
            vehicles: request.params["vehicles"].as_u64().unwrap_or(1) as usize,
        };

        let channel_name = request.params["channel"].as_str().unwrap_or("vehicle");
        let channel = declared(&request.container, channel_name, 2).map_err(Rejection::input)?;
        let sections = read_sections(&request.container).map_err(Rejection::input)?;
        let graph = parse_graph(
            &sections
                .iter()
                .find(|section| &section.kind == b"GRPH")
                .ok_or_else(|| Rejection::input("container is missing a GRPH section"))?
                .payload,
        )
        .map_err(Rejection::input)?;
        let parameters = Parameters {
            speed_property: request.params["speed_property"].as_str().map(str::to_string),
            restriction_property: request.params["restriction_property"].as_str().map(str::to_string),
            ..Default::default()
        };
        let costs = crate::traversal_seconds(&request.container, &graph, &parameters).map_err(Rejection::input)?;
        // A van turns like anything else: a fleet that ignored the restrictions would plan rounds
        // no driver could drive, which is worse than a longer round.
        let forbidden = crate::forbidden_turns(&request.container, &graph, parameters.restriction_property.as_deref())
            .map_err(Rejection::input)?;
        let assignments = plan(&graph, &costs, &forbidden, &fleet).map_err(Rejection::input)?;

        // One delta per edge per step, carrying which vehicle has reached it: 0 before anyone
        // does, then the vehicle's own number, so the map colours each round differently and
        // scrubbing backwards unwinds every van at once.
        let steps = ((request.t1_seconds - request.t0_seconds) / 60.0).floor() as i64 + 1;
        let mut records = Vec::new();
        for step in 0..steps {
            let t = request.t0_seconds + step as f32 * 60.0;
            let elapsed = (t - request.t0_seconds) as f64;
            for (vehicle, assignment) in assignments.iter().enumerate() {
                for (position, edge) in assignment.edges.iter().enumerate() {
                    let value = if elapsed >= assignment.reached_at[position] { vehicle as f32 + 1.0 } else { 0.0 };
                    records.extend_from_slice(&pack(graph.edges[*edge].id, channel, t, value));
                }
            }
        }
        Ok(records)
    }
}
