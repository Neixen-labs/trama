// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! What the trace solver guarantees over a container it did not build.

use serde_json::{Value, json};
use trama_format::{Graph, compile, edge_lengths, parse_graph, read_sections};
use trama_trace::{Cost, Direction, Operation, Parameters, components, solve, trace};

const CHANNEL: fn() -> Value = || json!({"name": "reach", "entity_kind": "edge", "unit": "1", "min": 0, "max": 1});

fn line(id: &str, coordinates: Value, directed: bool) -> Value {
    let properties = if directed { json!({"_trama_directed": true}) } else { json!({}) };
    json!({"type": "Feature", "id": id, "properties": properties, "geometry": {"type": "LineString", "coordinates": coordinates}})
}

/// A directed fork, plus one edge nowhere near it.
///
/// ```text
///   a --> b --> c        every arrow one-way
///          \
///           +-> d        e --> f   (a separate network entirely)
/// ```
fn fork() -> Vec<Value> {
    let a = [-3.7040, 40.4160];
    let b = [-3.7000, 40.4160];
    let c = [-3.6960, 40.4180];
    let d = [-3.6960, 40.4140];
    let e = [-3.6000, 40.5000];
    let f = [-3.5960, 40.5000];
    vec![
        line("ab", json!([a, b]), true),
        line("bc", json!([b, c]), true),
        line("bd", json!([b, d]), true),
        line("ef", json!([e, f]), true),
    ]
}

fn container_of(features: &[Value]) -> Vec<u8> {
    compile(features, &[CHANNEL()], &[]).unwrap()
}

fn graph_of(container: &[u8]) -> Graph {
    let sections = read_sections(container).unwrap();
    parse_graph(&sections.iter().find(|s| &s.kind == b"GRPH").unwrap().payload).unwrap()
}

/// Node indices are assigned by stable id, not by input order, so nodes are named by their role:
/// a source has nothing arriving at it, a sink has nothing leaving.
fn sources(graph: &Graph) -> Vec<usize> {
    let arrivals: Vec<u32> = graph.edges.iter().map(|edge| edge.target).collect();
    (0..graph.nodes.len()).filter(|node| !arrivals.contains(&(*node as u32))).collect()
}

fn sinks(graph: &Graph) -> Vec<usize> {
    let departures: Vec<u32> = graph.edges.iter().map(|edge| edge.source).collect();
    (0..graph.nodes.len()).filter(|node| !departures.contains(&(*node as u32))).collect()
}

fn hops(graph: &Graph) -> Vec<f64> {
    vec![1.0; graph.edges.len()]
}

#[test]
fn downstream_from_the_head_covers_the_fork() {
    let container = container_of(&fork()[..3]);
    let graph = graph_of(&container);
    let head = sources(&graph);
    assert_eq!(head.len(), 1, "the fork has one head");

    let reached = trace(&graph, &hops(&graph), &head, Direction::Forward, None).unwrap();

    assert_eq!(reached.len(), 3, "everything below the head");
}

#[test]
fn downstream_from_a_tail_reaches_nothing() {
    let container = container_of(&fork()[..3]);
    let graph = graph_of(&container);

    // The one rule the whole crate rests on: a directed edge has a single CSR entry, at its
    // source, so walking the adjacency as written cannot cross it backwards.
    let reached = trace(&graph, &hops(&graph), &[sinks(&graph)[0]], Direction::Forward, None).unwrap();

    assert!(reached.is_empty(), "nothing flows out of a tail, and nothing may be crossed against its arrow");
}

#[test]
fn upstream_from_a_tail_finds_what_feeds_it() {
    let container = container_of(&fork()[..3]);
    let graph = graph_of(&container);
    let tail = sinks(&graph);
    assert_eq!(tail.len(), 2, "the fork has two tails");

    let reached = trace(&graph, &hops(&graph), &tail[..1], Direction::Backward, None).unwrap();

    // Its own branch and the trunk above it, never the sibling branch.
    assert_eq!(reached.len(), 2);
}

#[test]
fn ignoring_direction_connects_the_whole_fork() {
    let container = container_of(&fork()[..3]);
    let graph = graph_of(&container);

    let reached = trace(&graph, &hops(&graph), &[sinks(&graph)[0]], Direction::Both, None).unwrap();

    assert_eq!(reached.len(), 3, "connectivity does not care which way the arrows point");
}

#[test]
fn a_budget_stops_the_search_where_it_says() {
    let container = container_of(&fork()[..3]);
    let graph = graph_of(&container);
    let head = sources(&graph);

    let one = trace(&graph, &hops(&graph), &head, Direction::Forward, Some(1.0)).unwrap();
    let two = trace(&graph, &hops(&graph), &head, Direction::Forward, Some(2.0)).unwrap();

    assert_eq!(one.len(), 1, "one hop is the trunk alone");
    assert_eq!(two.len(), 3, "two hops is both branches as well");
}

#[test]
fn an_isochrone_is_the_same_search_costed_in_seconds() {
    let container = container_of(&fork()[..3]);
    let graph = graph_of(&container);
    let lengths = edge_lengths(&container).unwrap();
    let seconds: Vec<f64> = lengths.iter().map(|length| length / 10.0).collect();
    let head = sources(&graph)[0];
    // Edge order follows stable ids too, so the trunk is the one leaving the head, not index 0.
    let trunk_index = graph.edges.iter().position(|edge| edge.source as usize == head).unwrap();
    let trunk = seconds[trunk_index];

    let short = trace(&graph, &seconds, &[head], Direction::Forward, Some(trunk)).unwrap();
    let long = trace(&graph, &seconds, &[head], Direction::Forward, Some(trunk * 10.0)).unwrap();

    assert_eq!(short.len(), 1, "only what fits in the budget");
    assert_eq!(short[0].edge_index, trunk_index);
    assert_eq!(long.len(), 3);
    assert!((short[0].at - trunk).abs() < 0.01, "arrival is the cost spent getting there");
}

#[test]
fn components_count_the_networks_a_map_makes_look_like_one() {
    let container = container_of(&fork());
    let graph = graph_of(&container);

    let labels = components(&graph);

    let mut distinct = labels.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), 2, "the fork and the lone edge are separate networks");
    assert_eq!(labels[0], labels[1], "the fork's edges share a label");
}

#[test]
fn a_seed_outside_the_graph_is_refused() {
    let container = container_of(&fork());
    let graph = graph_of(&container);

    let refused = trace(&graph, &hops(&graph), &[graph.nodes.len()], Direction::Forward, None);

    assert!(refused.is_err());
}

#[test]
fn a_trace_is_emitted_as_a_progression_the_scrub_can_unwind() {
    let container = container_of(&fork()[..3]);
    let graph = graph_of(&container);
    let parameters = Parameters {
        channel: "reach".into(),
        operation: Operation::Trace { seeds: sources(&graph), direction: Direction::Forward, budget: None },
        cost: Cost::Hops,
        step_seconds: 1.0,
    };

    let deltas = solve(&container, &parameters, 0.0, 3.0).unwrap();

    // Three edges over four instants, and the count of edges already reached only grows.
    let mut reached_per_instant: Vec<usize> = Vec::new();
    for step in 0..4 {
        let mut count = 0;
        for record in deltas.chunks_exact(18) {
            let t = f32::from_le_bytes(record[10..14].try_into().unwrap());
            let value = f32::from_le_bytes(record[14..18].try_into().unwrap());
            if t == step as f32 && value == 1.0 {
                count += 1;
            }
        }
        reached_per_instant.push(count);
    }
    assert_eq!(reached_per_instant, vec![0, 1, 3, 3], "the spread arrives hop by hop and stays");
}

#[test]
fn a_channel_the_container_never_declared_is_refused() {
    let container = container_of(&fork());
    let graph = graph_of(&container);
    let parameters = Parameters {
        channel: "pressure".into(),
        operation: Operation::Trace { seeds: sources(&graph), direction: Direction::Forward, budget: None },
        ..Default::default()
    };

    let refused = solve(&container, &parameters, 0.0, 1.0);

    assert!(refused.is_err(), "a solver may only write where the file says it may");
}

/// A ring with a tail, undirected: the ring has a spare route everywhere, the tail has none.
///
/// ```text
///   b ----- c
///   |       |          ring: ab, bc, cd, da
///   a ----- d --- t    tail: dt
/// ```
fn ring_with_tail() -> Vec<Value> {
    let a = [-3.7040, 40.4160];
    let b = [-3.7040, 40.4200];
    let c = [-3.7000, 40.4200];
    let d = [-3.7000, 40.4160];
    let t = [-3.6960, 40.4160];
    vec![
        line("ab", json!([a, b]), false),
        line("bc", json!([b, c]), false),
        line("cd", json!([c, d]), false),
        line("da", json!([d, a]), false),
        line("dt", json!([d, t]), false),
    ]
}

#[test]
fn only_the_tail_is_critical_in_a_ring() {
    let container = container_of(&ring_with_tail());
    let graph = graph_of(&container);

    let bridges = trama_trace::critical(&graph);

    assert_eq!(bridges.iter().filter(|is| **is).count(), 1, "the ring is its own spare; the tail is not");
    // The critical one is the edge touching the node of degree one.
    let lonely = (0..graph.nodes.len())
        .find(|node| {
            graph.edges.iter().filter(|edge| edge.source as usize == *node || edge.target as usize == *node).count()
                == 1
        })
        .unwrap();
    let tail =
        graph.edges.iter().position(|edge| edge.source as usize == lonely || edge.target as usize == lonely).unwrap();
    assert!(bridges[tail]);
}

#[test]
fn cutting_a_ring_edge_costs_nothing_and_cutting_the_tail_costs_the_tail() {
    let container = container_of(&ring_with_tail());
    let graph = graph_of(&container);
    let costs = hops(&graph);
    let bridges = trama_trace::critical(&graph);
    let tail = bridges.iter().position(|is| *is).unwrap();
    let ring_edge = bridges.iter().position(|is| !*is).unwrap();
    // Serve the network from the far end of the ring, away from the tail.
    let source = graph.edges[ring_edge].source as usize;

    let cut_ring = trama_trace::isolation(&graph, &costs, &[ring_edge], &[source], Direction::Both).unwrap();
    let cut_tail = trama_trace::isolation(&graph, &costs, &[tail], &[source], Direction::Both).unwrap();

    assert_eq!(
        cut_ring.iter().filter(|lost| **lost).count(),
        1,
        "only the cut edge itself: the ring goes round the other way"
    );
    assert_eq!(cut_tail.iter().filter(|lost| **lost).count(), 1, "the tail, and nothing beyond it to lose");
    assert!(cut_tail[tail]);
}

#[test]
fn a_cut_that_severs_a_branch_takes_everything_past_it() {
    let container = container_of(&fork()[..3]);
    let graph = graph_of(&container);
    let costs = hops(&graph);
    let head = sources(&graph)[0];
    let trunk = graph.edges.iter().position(|edge| edge.source as usize == head).unwrap();

    let lost = trama_trace::isolation(&graph, &costs, &[trunk], &[head], Direction::Forward).unwrap();

    assert_eq!(lost.iter().filter(|lost| **lost).count(), 3, "cutting the trunk loses the trunk and both branches");
}

#[test]
fn each_edge_goes_to_the_source_that_reaches_it_first() {
    let container = container_of(&ring_with_tail());
    let graph = graph_of(&container);
    let costs = hops(&graph);
    let ends: Vec<usize> = (0..graph.nodes.len())
        .filter(|node| {
            graph.edges.iter().filter(|edge| edge.source as usize == *node || edge.target as usize == *node).count()
                == 1
        })
        .collect();
    let corners: Vec<usize> = (0..graph.nodes.len()).filter(|node| !ends.contains(node)).collect();

    let owners = trama_trace::allocation(&graph, &costs, &[corners[0], corners[1]], Direction::Both).unwrap();

    assert!(owners.iter().all(Option::is_some), "a connected network leaves no edge unserved");
    let first = owners.iter().filter(|owner| **owner == Some(0)).count();
    let second = owners.iter().filter(|owner| **owner == Some(1)).count();
    assert!(first > 0 && second > 0, "two sources split the network between them, {first} and {second}");
}

#[test]
fn an_edge_no_source_reaches_belongs_to_nobody() {
    let container = container_of(&fork());
    let graph = graph_of(&container);
    let costs = hops(&graph);

    let owners = trama_trace::allocation(&graph, &costs, &[sources(&graph)[0]], Direction::Forward).unwrap();

    assert!(owners.iter().any(Option::is_none), "the separate network is served by nothing");
}

#[test]
fn isolation_refuses_a_cut_that_is_not_in_the_graph() {
    let container = container_of(&fork());
    let graph = graph_of(&container);

    let refused = trama_trace::isolation(&graph, &hops(&graph), &[graph.edges.len()], &[0], Direction::Both);

    assert!(refused.is_err());
}
