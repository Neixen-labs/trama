// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! What the published example has to be true of, checked rather than assumed.
//!
//! The first OpenStreetMap extract shipped in twelve hundred fragments and rendered perfectly:
//! a map cannot show you that a network is not connected, and every route across it failed for
//! a reason no screenshot explained. `components` is the check that costs nothing and would
//! have caught it on the first run.

use trama_format::{Graph, edge_lengths, parse_graph, read_sections};
use trama_trace::{Direction, allocation, components, critical, trace};

const TERUEL: &[u8] = include_bytes!("../../../fixtures/teruel.trama");

fn graph_of(container: &[u8]) -> Graph {
    let sections = read_sections(container).unwrap();
    parse_graph(&sections.iter().find(|s| &s.kind == b"GRPH").unwrap().payload).unwrap()
}

/// The largest component's share of the edges, and how many components there are.
fn spread(labels: &[usize]) -> (f64, usize) {
    let mut counts: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    for label in labels {
        *counts.entry(*label).or_default() += 1;
    }
    let largest = counts.values().copied().max().unwrap_or(0);
    (largest as f64 / labels.len() as f64, counts.len())
}

#[test]
fn the_published_city_is_one_network_and_not_a_pile_of_fragments() {
    let graph = graph_of(TERUEL);

    let (share, pieces) = spread(&components(&graph));

    assert!(share > 0.9, "the main component holds {share} of the streets, in {pieces} pieces");
}

#[test]
fn a_route_can_cross_the_published_city() {
    let graph = graph_of(TERUEL);
    let lengths = edge_lengths(TERUEL).unwrap();
    let labels = components(&graph);
    // Two nodes in the main component, as far apart in index as it holds.
    let main =
        labels.iter().copied().max_by_key(|label| labels.iter().filter(|other| *other == label).count()).unwrap();
    let inside: Vec<usize> = graph
        .edges
        .iter()
        .enumerate()
        .filter(|(edge, _)| labels[*edge] == main)
        .map(|(_, edge)| edge.source as usize)
        .collect();

    let reached = trace(&graph, &lengths, &inside[..1], Direction::Both, None).unwrap();

    assert!(
        reached.len() as f64 / graph.edges.len() as f64 > 0.9,
        "one point reaches {} of {} streets",
        reached.len(),
        graph.edges.len()
    );
}

#[test]
fn the_city_has_streets_that_are_the_only_way_through() {
    let graph = graph_of(TERUEL);

    let bridges = critical(&graph);
    let count = bridges.iter().filter(|is| **is).count();

    // A real street network is neither a tree nor a perfect mesh: some of both, or the demo has
    // nothing to show and the algorithm is not being exercised by it.
    assert!(count > 0, "no critical street at all");
    assert!(
        (count as f64) < graph.edges.len() as f64 * 0.8,
        "{count} of {} streets critical, which would mean the city is a tree",
        graph.edges.len()
    );
}

#[test]
fn two_depots_divide_the_city_between_them() {
    let graph = graph_of(TERUEL);
    let lengths = edge_lengths(TERUEL).unwrap();
    let labels = components(&graph);
    let main =
        labels.iter().copied().max_by_key(|label| labels.iter().filter(|other| *other == label).count()).unwrap();
    let inside: Vec<usize> = graph
        .edges
        .iter()
        .enumerate()
        .filter(|(edge, _)| labels[*edge] == main)
        .map(|(_, edge)| edge.source as usize)
        .collect();

    let owners = allocation(&graph, &lengths, &[inside[0], inside[inside.len() - 1]], Direction::Both).unwrap();

    let first = owners.iter().filter(|owner| **owner == Some(0)).count();
    let second = owners.iter().filter(|owner| **owner == Some(1)).count();
    assert!(first > 0 && second > 0, "two depots serve {first} and {second} streets");
}

#[test]
#[ignore = "reports the fixture's shape; run with --ignored when the extract changes"]
fn what_the_published_city_looks_like() {
    let graph = graph_of(TERUEL);
    let labels = components(&graph);
    let (share, pieces) = spread(&labels);
    let bridges = critical(&graph);
    let lengths = edge_lengths(TERUEL).unwrap();
    println!(
        "teruel.trama: {} bytes, {} nodes, {} edges, {:.1} km of street",
        TERUEL.len(),
        graph.nodes.len(),
        graph.edges.len(),
        lengths.iter().sum::<f64>() / 1000.0
    );
    println!("components: {pieces}, largest holds {:.1}%", share * 100.0);
    println!("critical edges: {} of {}", bridges.iter().filter(|is| **is).count(), graph.edges.len());

    let water = graph_of(NET3);
    let costs = vec![1.0; water.edges.len()];
    let water_bridges = critical(&water);
    let feed = water
        .edges
        .iter()
        .enumerate()
        .find(|(edge, _)| !water_bridges[*edge])
        .map(|(_, edge)| edge.source as usize)
        .unwrap();
    let worst = water_bridges
        .iter()
        .enumerate()
        .filter(|(_, is)| **is)
        .map(|(edge, _)| (edge, trama_trace::isolation(&water, &costs, &[edge], &[feed], Direction::Both).unwrap()))
        .map(|(edge, lost)| (lost.iter().filter(|lost| **lost).count(), edge))
        .max()
        .unwrap();
    println!(
        "net3: {} pipes, {} critical, worst single closure loses {} of them",
        water.edges.len(),
        water_bridges.iter().filter(|is| **is).count(),
        worst.0
    );
}

const NET3: &[u8] = include_bytes!("../../../fixtures/net3.trama");

#[test]
fn closing_a_critical_pipe_takes_more_than_the_pipe_itself() {
    let graph = graph_of(NET3);
    let costs = vec![1.0; graph.edges.len()];
    let bridges = critical(&graph);
    let feed = graph
        .edges
        .iter()
        .enumerate()
        .find(|(edge, _)| !bridges[*edge])
        .map(|(_, edge)| edge.source as usize)
        .expect("a meshed pipe to serve from");

    // Every critical pipe, tried one at a time: at least one of them has customers behind it.
    let worst = bridges
        .iter()
        .enumerate()
        .filter(|(_, is)| **is)
        .map(|(edge, _)| trama_trace::isolation(&graph, &costs, &[edge], &[feed], Direction::Both).unwrap())
        .map(|lost| lost.iter().filter(|lost| **lost).count())
        .max()
        .expect("a critical pipe");

    assert!(worst > 1, "the worst single closure loses {worst} pipes, so nothing is ever behind anything");
}

#[test]
fn closing_a_meshed_pipe_loses_only_that_pipe() {
    let graph = graph_of(NET3);
    let costs = vec![1.0; graph.edges.len()];
    let bridges = critical(&graph);
    let (meshed, feed) = graph
        .edges
        .iter()
        .enumerate()
        .find(|(edge, _)| !bridges[*edge])
        .map(|(index, edge)| (index, edge.source as usize))
        .expect("a meshed pipe");

    let lost = trama_trace::isolation(&graph, &costs, &[meshed], &[feed], Direction::Both).unwrap();

    // The point of a ring main: close one pipe and the water arrives the other way round.
    assert_eq!(lost.iter().filter(|lost| **lost).count(), 1, "a meshed closure should lose only itself");
}
