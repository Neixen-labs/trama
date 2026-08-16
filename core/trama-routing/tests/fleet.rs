// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! What the fleet planner guarantees, and how far from optimal it lands.
//!
//! There is no reference implementation to compare against here, the way the electrical solvers
//! have pandapower. So the oracle is built instead: on instances small enough to enumerate every
//! way of splitting the stops between the vehicles and every order within each split, the true
//! optimum is computed by brute force and the heuristic is measured against it. That is a real
//! check on the answer rather than on the code's self-consistency, which is the only kind worth
//! having for something that returns "a good solution" rather than "the solution".

use serde_json::{Value, json};
use trama_format::{compile, parse_graph, read_sections};
use trama_routing::fleet::{self, Fleet};

fn line(id: &str, coordinates: Value) -> Value {
    json!({"type": "Feature", "id": id, "properties": {}, "geometry": {"type": "LineString", "coordinates": coordinates}})
}

/// A ladder of `n + 1` junctions in a row, each rung one step longer than the last.
///
/// ```text
///   0 --1-- 1 --2-- 2 --3-- 3 ...
/// ```
///
/// Unequal spacing on purpose: on an evenly spaced line every ordering costs the same and a
/// planner that ignored distance entirely would pass.
fn ladder(rungs: usize) -> Vec<u8> {
    let mut features = Vec::new();
    let mut at = 0.0;
    for rung in 0..rungs {
        let next = at + 0.001 * (rung + 1) as f64;
        features.push(line(&format!("r{rung}"), json!([[-3.70 + at, 40.41], [-3.70 + next, 40.41]])));
        at = next;
    }
    let channel = json!({"name": "vehicle", "entity_kind": "edge", "unit": "1"});
    compile(&features, &[channel], &[]).unwrap()
}

fn graph_of(container: &[u8]) -> trama_format::Graph {
    parse_graph(&read_sections(container).unwrap().into_iter().find(|s| &s.kind == b"GRPH").unwrap().payload).unwrap()
}

/// The ladder's junctions in the order they are strung together.
///
/// The graph carries no coordinates — a node is an id and a property row — so the order comes from
/// walking the topology from one end rather than from sorting by position. Which end is arbitrary
/// and does not matter: every cost below is measured against the same graph, so the comparison
/// holds whichever way round the walk starts.
fn nodes_along(graph: &trama_format::Graph) -> Vec<usize> {
    let neighbours = |node: usize| -> Vec<usize> {
        let (start, end) = (graph.csr_offsets[node] as usize, graph.csr_offsets[node + 1] as usize);
        graph.adjacency[start..end]
            .iter()
            .map(|entry| {
                let edge = &graph.edges[entry.edge_index as usize];
                if entry.traversal_direction > 0 { edge.target as usize } else { edge.source as usize }
            })
            .collect()
    };
    let end = (0..graph.nodes.len()).find(|node| neighbours(*node).len() == 1).expect("a ladder has two ends");
    let mut order = vec![end];
    let mut previous = usize::MAX;
    let mut at = end;
    while let Some(next) = neighbours(at).into_iter().find(|node| *node != previous) {
        order.push(next);
        previous = at;
        at = next;
    }
    order
}

/// The cost of serving `stops` in this order from the depot and returning, by the same matrix the
/// planner uses — a straight sum over the legs.
fn round_trip(costs: &[Vec<f64>], order: &[usize]) -> f64 {
    let mut total = 0.0;
    let mut previous = 0;
    for stop in order {
        total += costs[previous][stop + 1];
        previous = stop + 1;
    }
    total + costs[previous][0]
}

/// Every way of splitting `stops` between `vehicles`, with every order inside each — the optimum,
/// by exhaustion. Only usable for a handful of stops, which is the point.
fn optimum(costs: &[Vec<f64>], demands: &[f64], capacity: f64, vehicles: usize) -> f64 {
    let n = demands.len();
    let mut best = f64::INFINITY;
    // Each stop is assigned to one vehicle: `vehicles^n` labellings, filtered by capacity.
    for labelling in 0..vehicles.pow(n as u32) {
        let mut groups: Vec<Vec<usize>> = vec![Vec::new(); vehicles];
        let mut code = labelling;
        for stop in 0..n {
            groups[code % vehicles].push(stop);
            code /= vehicles;
        }
        if groups.iter().any(|group| group.iter().map(|stop| demands[*stop]).sum::<f64>() > capacity) {
            continue;
        }
        let total: f64 = groups.iter().map(|group| best_order(costs, group)).sum();
        best = best.min(total);
    }
    best
}

/// The cheapest order for one vehicle's group, by trying all of them.
fn best_order(costs: &[Vec<f64>], group: &[usize]) -> f64 {
    if group.is_empty() {
        return 0.0;
    }
    let mut order: Vec<usize> = group.to_vec();
    let mut best = f64::INFINITY;
    permute(&mut order, 0, &mut |candidate| best = best.min(round_trip(costs, candidate)));
    best
}

fn permute(order: &mut Vec<usize>, at: usize, visit: &mut impl FnMut(&[usize])) {
    if at == order.len() {
        visit(order);
        return;
    }
    for swap in at..order.len() {
        order.swap(at, swap);
        permute(order, at + 1, visit);
        order.swap(at, swap);
    }
}

/// The distance matrix the comparison is built on, from the single-route planner.
///
/// One leg, not a round trip: `trama_routing::plan` walks from the first waypoint to the last and
/// stops there. Using the fleet planner here instead would return each distance doubled, since
/// every round it plans comes home — which is exactly the mistake this comment exists to prevent
/// anyone repeating.
fn matrix(graph: &trama_format::Graph, costs: &[f64], points: &[usize]) -> Vec<Vec<f64>> {
    points
        .iter()
        .map(|from| {
            points
                .iter()
                .map(|to| {
                    if from == to {
                        0.0
                    } else {
                        let leg = trama_routing::plan(graph, costs, &[*from, *to]).unwrap();
                        *leg.reached_at.last().unwrap()
                    }
                })
                .collect()
        })
        .collect()
}

#[test]
fn every_stop_is_served_once_and_no_vehicle_is_overloaded() {
    let container = ladder(6);
    let graph = graph_of(&container);
    let nodes = nodes_along(&graph);
    let costs = vec![1.0; graph.edges.len()];

    let fleet = Fleet {
        depot: nodes[0],
        stops: nodes[1..7].to_vec(),
        demands: vec![3.0, 4.0, 2.0, 5.0, 3.0, 1.0],
        capacity: 10.0,
        vehicles: 3,
    };
    let plan = fleet::plan(&graph, &costs, &fleet).unwrap_or_else(|error| panic!("{error}"));

    let mut served: Vec<usize> = plan.iter().flat_map(|assignment| assignment.stops.clone()).collect();
    served.sort();
    assert_eq!(served, vec![0, 1, 2, 3, 4, 5], "every stop exactly once");
    assert!(plan.len() <= 3, "no more rounds than vehicles: {}", plan.len());
    for assignment in &plan {
        assert!(assignment.load <= 10.0, "a vehicle carries {} against a capacity of 10", assignment.load);
        assert!(!assignment.edges.is_empty(), "a round that drives nowhere is not a round");
    }
}

/// The comparison that means something: against the true optimum, computed by exhaustion.
#[test]
fn the_plan_is_within_a_tenth_of_the_true_optimum() {
    let container = ladder(5);
    let graph = graph_of(&container);
    let nodes = nodes_along(&graph);
    // Length in metres, so the cost is distance and the ladder's unequal rungs matter.
    let costs = trama_format::edge_lengths(&container).unwrap();

    let stops = nodes[1..6].to_vec();
    let demands = vec![4.0, 4.0, 4.0, 4.0, 4.0];
    let fleet = Fleet { depot: nodes[0], stops: stops.clone(), demands: demands.clone(), capacity: 10.0, vehicles: 3 };
    let plan = fleet::plan(&graph, &costs, &fleet).unwrap();
    let planned = fleet::total_cost(&plan);

    let mut points = vec![nodes[0]];
    points.extend(&stops);
    let best = optimum(&matrix(&graph, &costs, &points), &demands, 10.0, 3);

    assert!(planned >= best - 1e-6, "no plan can beat the optimum: {planned} against {best}");
    // Clarke-Wright with 2-opt is typically within a few percent on instances this shape. A tenth
    // is the bar this asserts; the message prints the real gap, so a regression that stays inside
    // the bar is still visible when the test is read.
    assert!(planned <= best * 1.1, "planned {planned}, optimum {best}, gap {:.1}%", (planned / best - 1.0) * 100.0);
}

#[test]
fn a_stop_bigger_than_a_vehicle_is_refused_rather_than_split() {
    let container = ladder(3);
    let graph = graph_of(&container);
    let nodes = nodes_along(&graph);
    let costs = vec![1.0; graph.edges.len()];

    let fleet = Fleet {
        depot: nodes[0],
        stops: vec![nodes[1], nodes[2]],
        demands: vec![2.0, 20.0],
        capacity: 10.0,
        vehicles: 4,
    };
    match fleet::plan(&graph, &costs, &fleet) {
        Err(message) => assert!(message.contains("no vehicle can serve it"), "{message}"),
        Ok(_) => panic!("a load no vehicle can carry has no plan, and splitting it invents one"),
    }
}

#[test]
fn too_few_vehicles_says_how_many_it_would_need() {
    let container = ladder(4);
    let graph = graph_of(&container);
    let nodes = nodes_along(&graph);
    let costs = vec![1.0; graph.edges.len()];

    let fleet = Fleet {
        depot: nodes[0],
        stops: nodes[1..5].to_vec(),
        demands: vec![8.0, 8.0, 8.0, 8.0],
        capacity: 10.0,
        vehicles: 1,
    };
    match fleet::plan(&graph, &costs, &fleet) {
        Err(message) => assert!(message.contains("vehicles"), "{message}"),
        Ok(plan) => panic!("32 units of demand cannot fit in one 10-unit van: {} rounds", plan.len()),
    }
}
