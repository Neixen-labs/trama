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
use trama_routing::Turns;
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
                        let leg = trama_routing::plan(graph, costs, &no_turns(), &[*from, *to]).unwrap();
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
        windows: Vec::new(),
        service: Vec::new(),
    };
    let plan = fleet::plan(&graph, &costs, &no_turns(), &fleet).unwrap_or_else(|error| panic!("{error}"));

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
    let fleet = Fleet {
        depot: nodes[0],
        stops: stops.clone(),
        demands: demands.clone(),
        capacity: 10.0,
        vehicles: 3,
        windows: Vec::new(),
        service: Vec::new(),
    };
    let plan = fleet::plan(&graph, &costs, &no_turns(), &fleet).unwrap();
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
        windows: Vec::new(),
        service: Vec::new(),
    };
    match fleet::plan(&graph, &costs, &no_turns(), &fleet) {
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
        windows: Vec::new(),
        service: Vec::new(),
    };
    match fleet::plan(&graph, &costs, &no_turns(), &fleet) {
        Err(message) => {
            // Which constraint is named matters more than that one is. 32 units against a 10-unit
            // van needs four vans however the stops are arranged, so this is a bound and the
            // message is entitled to state it as one.
            assert!(message.contains("at least 4 vehicles"), "{message}");
            assert!(
                !message.contains("could not fit"),
                "this is a real impossibility, not the planner giving up: {message}"
            );
        }
        Ok(plan) => panic!("32 units of demand cannot fit in one 10-unit van: {} rounds", plan.len()),
    }
}

/// The other refusal, and the one that used to lie. Where capacity is ample and the windows are
/// what stops the rounds combining, the planner says it could not find a plan rather than claiming
/// none exists — because it has not proved that, and measurement says such plans usually do exist.
///
/// Reproducing that state end to end takes an instance the consolidation pass also fails on, which
/// is now rare by design. What is pinned here instead is the property that matters and that the
/// old message got wrong: a refusal never blames capacity for a shortfall capacity does not cause.
#[test]
fn a_refusal_names_the_constraint_that_actually_bites() {
    let container = ladder(4);
    let graph = graph_of(&container);
    let nodes = nodes_along(&graph);
    let costs = trama_format::edge_lengths(&container).unwrap();

    // One unit of demand each against a capacity of ten: capacity could not possibly be the
    // reason for any refusal here, whatever the windows do.
    let fleet = Fleet {
        depot: nodes[0],
        stops: nodes[1..5].to_vec(),
        demands: vec![1.0; 4],
        capacity: 10.0,
        vehicles: 1,
        windows: vec![(0.0, 1e9); 4],
        service: Vec::new(),
    };
    match fleet::plan(&graph, &costs, &no_turns(), &fleet) {
        Ok(plan) => assert_eq!(plan.len(), 1, "one van serves all four stops"),
        Err(message) => {
            assert!(message.contains("could not fit"), "capacity is ample, so no refusal may blame it: {message}");
            assert!(!message.contains("at least"), "{message}");
        }
    }
}

/// No turn restrictions: what every test here but the restriction ones asks for.
fn no_turns() -> Turns {
    Turns::new()
}

/// When each stop on a round is served, worked out here rather than asked of the planner.
///
/// Deliberately a second implementation of the rule, not a call into the first: a test that asked
/// `fleet::plan` whether its own answer was feasible would agree with itself no matter what the
/// rule was. Early is a wait, late is a refusal — that is the whole statement, and this is the
/// place it gets written down twice on purpose.
fn arrivals(costs: &[Vec<f64>], windows: &[(f64, f64)], service: &[f64], order: &[usize]) -> Option<Vec<f64>> {
    let mut served = Vec::new();
    let mut clock = 0.0;
    let mut previous = 0;
    for stop in order {
        clock += costs[previous][stop + 1];
        clock = clock.max(windows[*stop].0);
        if clock > windows[*stop].1 {
            return None;
        }
        served.push(clock);
        clock += service.get(*stop).copied().unwrap_or(0.0);
        previous = stop + 1;
    }
    Some(served)
}

/// One vehicle, two stops, and a window that only one order satisfies.
///
/// ```text
///   depot --85m-- near ------1190m------ far
/// ```
///
/// On a line the two orders cost the same to drive, so nothing about distance chooses between
/// them: the round goes out and comes back either way. What chooses is the clock. `near` does not
/// open until 1500, and leaving it that late puts `far` past its own close — so the only round
/// that can be driven serves `far` first and doubles back, which is the order a planner that
/// ignored windows would have no reason to pick.
#[test]
fn a_window_decides_an_order_that_distance_is_indifferent_to() {
    let container = ladder(5);
    let graph = graph_of(&container);
    let nodes = nodes_along(&graph);
    let costs = trama_format::edge_lengths(&container).unwrap();
    let stops = vec![nodes[1], nodes[5]];
    let windows = vec![(1500.0, 3000.0), (0.0, 2000.0)];

    let with = fleet::plan(
        &graph,
        &costs,
        &no_turns(),
        &Fleet {
            depot: nodes[0],
            stops: stops.clone(),
            demands: vec![1.0, 1.0],
            capacity: 10.0,
            vehicles: 1,
            windows: windows.clone(),
            service: Vec::new(),
        },
    )
    .unwrap_or_else(|error| panic!("{error}"));

    let mut points = vec![nodes[0]];
    points.extend(&stops);
    let matrix = matrix(&graph, &costs, &points);

    assert_eq!(with.len(), 1, "one vehicle, one round");
    assert_eq!(with[0].stops, vec![1, 0], "the far stop first, because the near one is not open yet");
    assert!(
        arrivals(&matrix, &windows, &[], &with[0].stops).is_some(),
        "the planned round misses a window it was given"
    );
    // And the order the planner picks when nobody mentions time is the one that breaks them, so
    // the assertion above is not passing by luck.
    let without = fleet::plan(
        &graph,
        &costs,
        &no_turns(),
        &Fleet {
            depot: nodes[0],
            stops,
            demands: vec![1.0, 1.0],
            capacity: 10.0,
            vehicles: 1,
            windows: Vec::new(),
            service: Vec::new(),
        },
    )
    .unwrap();
    assert!(
        arrivals(&matrix, &windows, &[], &without[0].stops).is_none(),
        "the unconstrained order happens to satisfy the windows, so this instance proves nothing"
    );
}

/// The optimum again, with the enumeration filtered by feasibility rather than by capacity alone.
#[test]
fn the_plan_with_windows_is_within_a_tenth_of_the_best_feasible_one() {
    let container = ladder(5);
    let graph = graph_of(&container);
    let nodes = nodes_along(&graph);
    let costs = trama_format::edge_lengths(&container).unwrap();

    let stops = nodes[1..6].to_vec();
    let demands = vec![4.0, 4.0, 4.0, 4.0, 4.0];
    // Wide enough that plenty of orders work and a few do not, which is where a heuristic can
    // still be wrong without being obviously wrong.
    let windows = vec![(0.0, 4000.0), (0.0, 3000.0), (500.0, 5000.0), (0.0, 6000.0), (0.0, 2500.0)];
    let fleet = Fleet {
        depot: nodes[0],
        stops: stops.clone(),
        demands: demands.clone(),
        capacity: 10.0,
        vehicles: 3,
        windows: windows.clone(),
        service: Vec::new(),
    };
    let plan = fleet::plan(&graph, &costs, &no_turns(), &fleet).unwrap_or_else(|error| panic!("{error}"));

    let mut points = vec![nodes[0]];
    points.extend(&stops);
    let matrix = matrix(&graph, &costs, &points);
    for assignment in &plan {
        assert!(
            arrivals(&matrix, &windows, &[], &assignment.stops).is_some(),
            "a planned round misses a window: {:?}",
            assignment.stops
        );
    }

    let best = feasible_optimum(&matrix, &demands, &windows, 10.0, 3);
    let planned = fleet::total_cost(&plan);
    assert!(best.is_finite(), "the instance has no feasible plan at all, so the comparison is empty");
    assert!(planned >= best - 1e-6, "no plan can beat the optimum: {planned} against {best}");
    assert!(
        planned <= best * 1.1,
        "planned {planned}, best feasible {best}, gap {:.1}%",
        (planned / best - 1.0) * 100.0
    );
}

/// The optimum over the orders a window actually allows.
fn feasible_optimum(
    costs: &[Vec<f64>],
    demands: &[f64],
    windows: &[(f64, f64)],
    capacity: f64,
    vehicles: usize,
) -> f64 {
    let n = demands.len();
    let mut best = f64::INFINITY;
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
        let total: f64 = groups.iter().map(|group| best_feasible_order(costs, windows, group)).sum();
        best = best.min(total);
    }
    best
}

fn best_feasible_order(costs: &[Vec<f64>], windows: &[(f64, f64)], group: &[usize]) -> f64 {
    if group.is_empty() {
        return 0.0;
    }
    let mut order: Vec<usize> = group.to_vec();
    let mut best = f64::INFINITY;
    permute(&mut order, 0, &mut |candidate| {
        if arrivals(costs, windows, &[], candidate).is_some() {
            best = best.min(round_trip(costs, candidate));
        }
    });
    best
}

/// A window nothing can meet is refused, and the message says which stop and by how much.
#[test]
fn a_stop_that_shuts_before_anyone_could_arrive_is_refused_rather_than_missed() {
    let container = ladder(5);
    let graph = graph_of(&container);
    let nodes = nodes_along(&graph);
    let costs = trama_format::edge_lengths(&container).unwrap();

    let fleet = Fleet {
        depot: nodes[0],
        stops: vec![nodes[1], nodes[5]],
        demands: vec![1.0, 1.0],
        capacity: 10.0,
        vehicles: 4,
        // The far stop shuts at 10, and the direct run there is the earliest any vehicle could
        // possibly arrive — so no ordering, no extra van and no capacity saves it.
        windows: vec![(0.0, 5000.0), (0.0, 10.0)],
        service: Vec::new(),
    };

    match fleet::plan(&graph, &costs, &no_turns(), &fleet) {
        Err(message) => {
            assert!(message.contains("stop 1"), "the message names the stop: {message}");
            assert!(message.contains("in time"), "{message}");
        }
        Ok(plan) => panic!("a stop nobody can reach in time has no plan: {} rounds", plan.len()),
    }
}

/// Waiting and serving are on the clock the map draws, not just in the feasibility check.
#[test]
fn a_van_that_waits_shows_the_round_taking_longer() {
    let container = ladder(5);
    let graph = graph_of(&container);
    let nodes = nodes_along(&graph);
    let costs = trama_format::edge_lengths(&container).unwrap();
    let round = |windows: Vec<(f64, f64)>, service: Vec<f64>| {
        let fleet = Fleet {
            depot: nodes[0],
            stops: vec![nodes[1]],
            demands: vec![1.0],
            capacity: 10.0,
            vehicles: 1,
            windows,
            service,
        };
        fleet::total_cost(&fleet::plan(&graph, &costs, &no_turns(), &fleet).unwrap())
    };

    let driving = round(Vec::new(), Vec::new());
    let waiting = round(vec![(5000.0, 9000.0)], Vec::new());
    let serving = round(vec![(0.0, 9000.0)], vec![600.0]);

    // The stop is a couple of hundred metres out, so a window opening at 5000 is a long idle.
    assert!(waiting > driving + 4000.0, "waiting {waiting} against driving {driving}");
    assert!(
        (serving - driving - 600.0).abs() < 1e-6,
        "serving should add exactly its own 600: {serving} against {driving}"
    );
}
