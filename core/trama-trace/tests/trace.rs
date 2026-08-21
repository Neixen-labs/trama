// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! What the trace solver guarantees over a container it did not build.

use serde_json::{Value, json};
use trama_format::{Graph, compile, edge_lengths, parse_graph, read_sections};
use trama_trace::{Cost, Direction, Operation, Parameters, Turns, components, solve, trace};

fn no_turns() -> Turns {
    Turns::new()
}

/// The index of the edge declared under `name`, so a test can forbid a turn by name.
fn edge_at(container: &[u8], name: &str) -> usize {
    let id = trama_format::edge_id(name);
    graph_of(container).edges.iter().position(|edge| edge.id == id).expect("the edge is in the graph")
}

/// The node nearest a coordinate, so a test can name a junction by where it drew it.
fn node_at(container: &[u8], longitude: f64, latitude: f64) -> usize {
    let exported = trama_format::export(container).unwrap();
    let id = exported.nodes["features"]
        .as_array()
        .unwrap()
        .iter()
        .min_by(|left, right| {
            let distance = |feature: &Value| {
                let point = feature["geometry"]["coordinates"].as_array().unwrap();
                (point[0].as_f64().unwrap() - longitude).hypot(point[1].as_f64().unwrap() - latitude)
            };
            distance(left).partial_cmp(&distance(right)).unwrap()
        })
        .map(|feature| feature["properties"]["_trama_id"].as_str().unwrap().parse::<u64>().unwrap())
        .unwrap();
    graph_of(container).nodes.iter().position(|node| node.id == id).unwrap()
}

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

    let reached = trace(&graph, &hops(&graph), &no_turns(), &head, Direction::Forward, None).unwrap();

    assert_eq!(reached.len(), 3, "everything below the head");
}

#[test]
fn downstream_from_a_tail_reaches_nothing() {
    let container = container_of(&fork()[..3]);
    let graph = graph_of(&container);

    // The one rule the whole crate rests on: a directed edge has a single CSR entry, at its
    // source, so walking the adjacency as written cannot cross it backwards.
    let reached = trace(&graph, &hops(&graph), &no_turns(), &[sinks(&graph)[0]], Direction::Forward, None).unwrap();

    assert!(reached.is_empty(), "nothing flows out of a tail, and nothing may be crossed against its arrow");
}

#[test]
fn upstream_from_a_tail_finds_what_feeds_it() {
    let container = container_of(&fork()[..3]);
    let graph = graph_of(&container);
    let tail = sinks(&graph);
    assert_eq!(tail.len(), 2, "the fork has two tails");

    let reached = trace(&graph, &hops(&graph), &no_turns(), &tail[..1], Direction::Backward, None).unwrap();

    // Its own branch and the trunk above it, never the sibling branch.
    assert_eq!(reached.len(), 2);
}

#[test]
fn ignoring_direction_connects_the_whole_fork() {
    let container = container_of(&fork()[..3]);
    let graph = graph_of(&container);

    let reached = trace(&graph, &hops(&graph), &no_turns(), &[sinks(&graph)[0]], Direction::Both, None).unwrap();

    assert_eq!(reached.len(), 3, "connectivity does not care which way the arrows point");
}

#[test]
fn a_budget_stops_the_search_where_it_says() {
    let container = container_of(&fork()[..3]);
    let graph = graph_of(&container);
    let head = sources(&graph);

    let one = trace(&graph, &hops(&graph), &no_turns(), &head, Direction::Forward, Some(1.0)).unwrap();
    let two = trace(&graph, &hops(&graph), &no_turns(), &head, Direction::Forward, Some(2.0)).unwrap();

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

    let short = trace(&graph, &seconds, &no_turns(), &[head], Direction::Forward, Some(trunk)).unwrap();
    let long = trace(&graph, &seconds, &no_turns(), &[head], Direction::Forward, Some(trunk * 10.0)).unwrap();

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

    let refused = trace(&graph, &hops(&graph), &no_turns(), &[graph.nodes.len()], Direction::Forward, None);

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
        restriction_property: None,
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

    let cut_ring =
        trama_trace::isolation(&graph, &costs, &no_turns(), &[ring_edge], &[source], Direction::Both).unwrap();
    let cut_tail = trama_trace::isolation(&graph, &costs, &no_turns(), &[tail], &[source], Direction::Both).unwrap();

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

    let lost = trama_trace::isolation(&graph, &costs, &no_turns(), &[trunk], &[head], Direction::Forward).unwrap();

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

    let owners =
        trama_trace::allocation(&graph, &costs, &no_turns(), &[corners[0], corners[1]], Direction::Both).unwrap();

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

    let owners =
        trama_trace::allocation(&graph, &costs, &no_turns(), &[sources(&graph)[0]], Direction::Forward).unwrap();

    assert!(owners.iter().any(Option::is_none), "the separate network is served by nothing");
}

#[test]
fn isolation_refuses_a_cut_that_is_not_in_the_graph() {
    let container = container_of(&fork());
    let graph = graph_of(&container);

    let refused =
        trama_trace::isolation(&graph, &hops(&graph), &no_turns(), &[graph.edges.len()], &[0], Direction::Both);

    assert!(refused.is_err());
}

/// The junction where a spread has to remember how it arrived.
///
/// ```text
///   s --------- v --- t     `sv` is the cheap way to v, and may not continue onto `vt`
///    \         /
///     -- q ---              `qv` is dearer, and may
/// ```
///
/// The same shape `trama-routing` settles arcs for, asked as an isochrone instead of a route.
/// A search that settled nodes would reach `v` along `sv` at one hop, record that as the way to
/// `v`, and then leave along `vt` — a movement `sv` was told it could not make. Arriving along
/// `qv` costs two hops and is the only arrival `vt` is open to, so within two hops `vt` is not
/// reached at all. Node-settling puts it inside the isochrone; arc-settling does not, and that
/// one edge is the whole difference between a spread that obeys the restrictions and one that
/// only looks like it does.
fn junction() -> Vec<Value> {
    let s = [-3.7080, 40.4160];
    let v = [-3.7000, 40.4160];
    let q = [-3.7040, 40.4100];
    let t = [-3.6960, 40.4160];
    vec![
        line("sv", json!([s, v]), false),
        line("sq", json!([s, q]), false),
        line("qv", json!([q, v]), false),
        line("vt", json!([v, t]), false),
    ]
}

/// The edge ids the spread reached, so an assertion can name streets rather than indices.
fn streets(container: &[u8], reached: &[trama_trace::Reached]) -> Vec<u64> {
    let graph = graph_of(container);
    let mut ids: Vec<u64> = reached.iter().map(|reached| graph.edges[reached.edge_index].id).collect();
    ids.sort_unstable();
    ids
}

fn ids(names: &[&str]) -> Vec<u64> {
    let mut ids: Vec<u64> = names.iter().map(|name| trama_format::edge_id(name)).collect();
    ids.sort_unstable();
    ids
}

#[test]
fn a_forbidden_turn_keeps_the_street_beyond_it_out_of_the_isochrone() {
    let container = container_of(&junction());
    let graph = graph_of(&container);
    let start = node_at(&container, -3.7080, 40.4160);
    let forbidden = Turns::from_sequences([vec![edge_at(&container, "sv"), edge_at(&container, "vt")]]);

    let open = trace(&graph, &hops(&graph), &no_turns(), &[start], Direction::Forward, Some(2.0)).unwrap();
    let shut = trace(&graph, &hops(&graph), &forbidden, &[start], Direction::Forward, Some(2.0)).unwrap();

    assert_eq!(streets(&container, &open), ids(&["sv", "sq", "qv", "vt"]), "with nothing forbidden two hops reach t");
    assert_eq!(
        streets(&container, &shut),
        ids(&["sv", "sq", "qv"]),
        "the cheap arrival at v cannot leave along vt, and the dear one is a hop too far"
    );

    // And the part only an arc-settling search gets right. Lift the budget and `vt` is reachable
    // again — along `qv`, at three hops instead of two. A search that settled nodes would have
    // fixed `v` at one hop by way of `sv`, refused the turn, and never reconsidered the junction
    // when the dearer arrival showed up: `vt` would drop out of the isochrone entirely, at every
    // budget, which is a street the spread can plainly reach reported as unreachable.
    let far = trace(&graph, &hops(&graph), &forbidden, &[start], Direction::Forward, None).unwrap();
    let arrival = |street: &str| {
        let edge = edge_at(&container, street);
        far.iter().find(|reached| reached.edge_index == edge).map(|reached| reached.at)
    };

    assert_eq!(arrival("vt"), Some(3.0), "reached the long way round, not lost with the turn that was refused");
    assert_eq!(arrival("qv"), Some(2.0), "the dear arrival at v is what opens it");
}

#[test]
fn running_the_network_backwards_reads_the_same_restriction_from_the_other_end() {
    let container = container_of(&junction());
    let graph = graph_of(&container);
    let end = node_at(&container, -3.6960, 40.4160);
    let forbidden = Turns::from_sequences([vec![edge_at(&container, "sv"), edge_at(&container, "vt")]]);

    let backward = trace(&graph, &hops(&graph), &forbidden, &[end], Direction::Backward, Some(2.0)).unwrap();

    // Having crossed `vt`, `sv` is what may not be taken: the same movement, met from the far end.
    assert_eq!(
        streets(&container, &backward),
        ids(&["vt", "qv"]),
        "sv is shut to an arrival along vt, so two hops back from t stop at qv"
    );
}

#[test]
fn ignoring_direction_ignores_the_turns_with_it() {
    let container = container_of(&junction());
    let graph = graph_of(&container);
    let start = node_at(&container, -3.7080, 40.4160);
    let forbidden = Turns::from_sequences([vec![edge_at(&container, "sv"), edge_at(&container, "vt")]]);

    let both = trace(&graph, &hops(&graph), &forbidden, &[start], Direction::Both, Some(2.0)).unwrap();

    // "What is connected to this" is not a movement, so there is no turn in it to forbid.
    assert_eq!(streets(&container, &both), ids(&["sv", "sq", "qv", "vt"]), "connectivity is not a journey");
}

/// The whole path, from a column in the file to a spread that honours it: this is what the road
/// importer writes, and naming the column is what makes an isochrone agree with a route over the
/// same container.
#[test]
fn the_solver_reads_the_restriction_column_the_importer_wrote() {
    let mut features = junction();
    features[0]["properties"]["roads:no_turn"] = json!(trama_format::edge_id("vt").to_string());
    let container = compile(&features, &[CHANNEL()], &[]).unwrap();
    let start = node_at(&container, -3.7080, 40.4160);
    let spread = |restriction_property: Option<String>| {
        let parameters = Parameters {
            operation: Operation::Trace { seeds: vec![start], direction: Direction::Forward, budget: Some(2.0) },
            cost: Cost::Hops,
            restriction_property,
            ..Default::default()
        };
        solve(&container, &parameters, 0.0, 0.0).unwrap().len()
    };

    // Without naming the column the file's restriction is inert: a container carries the fact,
    // and a caller decides whether this question is one it applies to.
    assert_eq!(spread(None) / 18, 4, "four streets within two hops when the column is not read");
    assert_eq!(spread(Some("roads:no_turn".into())) / 18, 3, "three when it is");
}

/// The same three edges, forbidden from one direction of approach and allowed from another.
///
/// ```text
///   a --ab--> b --bl--> l --ld--> d      `ab` then `bl` then `ld` is refused
///   z --zb-->/                           `zb` then `bl` then `ld` is not
/// ```
///
/// This is what a run of three says that no pair can. Forbidding `bl` then `ld` would shut the
/// movement for traffic arriving from `z` as well, which nobody forbade; forbidding `ab` then `bl`
/// would shut the link outright. Only the whole run, matched against how the spread actually
/// arrived, distinguishes the two.
///
/// It is also the case that catches a search which checks the automaton but does not carry it: the
/// restriction bites on the *second* step out of the seed, so a walk that recomputed its progress
/// from scratch at each arc — or dropped it — would refuse nothing here while still passing every
/// test whose restriction bites on the first step.
#[test]
fn a_run_is_refused_by_how_the_spread_arrived_and_not_by_which_edges_it_holds() {
    let a = [-3.7080, 40.4180];
    let z = [-3.7080, 40.4140];
    let b = [-3.7040, 40.4180];
    let l = [-3.7000, 40.4180];
    let d = [-3.6960, 40.4180];
    let features = vec![
        line("ab", json!([a, b]), true),
        line("zb", json!([z, b]), true),
        line("bl", json!([b, l]), true),
        line("ld", json!([l, d]), true),
    ];
    let container = container_of(&features);
    let graph = graph_of(&container);
    let edge = |name: &str| edge_at(&container, name);
    let forbidden = Turns::from_sequences([vec![edge("ab"), edge("bl"), edge("ld")]]);
    let spread = |seed: [f64; 2], turns: &Turns| {
        let from = node_at(&container, seed[0], seed[1]);
        streets(&container, &trace(&graph, &hops(&graph), turns, &[from], Direction::Forward, None).unwrap())
    };

    assert_eq!(spread(a, &no_turns()), ids(&["ab", "bl", "ld"]), "with nothing forbidden the spread runs to d");
    assert_eq!(spread(a, &forbidden), ids(&["ab", "bl"]), "the run is refused on its last edge, not its first");
    assert_eq!(spread(z, &forbidden), ids(&["zb", "bl", "ld"]), "arriving along zb is a different run, and allowed");
}

/// Why the progress belongs in the settled state and not merely in the check.
///
/// ```text
///   s --sa--> a --ab--> b --bl--> l --ld--> d     `ab` then `bl` then `ld` is refused
///    \                 /
///     --sy--> y --yz--> z --zb-->
/// ```
///
/// The spread reaches `bl` twice: cheaply at three hops having come along `ab`, which is partway
/// through the forbidden run, and dearly at four having come along `zb`, which is not. Those are
/// different situations on the same edge — one of them may go on to `ld` and the other may not.
///
/// A search that settled arcs alone would keep the cheap arrival, discard the dear one as
/// redundant, and then find `ld` shut: it would report a street unreachable that the spread can
/// plainly reach, at five hops, by the other road. This is the same shape as the node-settling bug
/// arcs were introduced to fix, one level further in, and it is invisible to any test whose run is
/// reachable only one way.
#[test]
fn arriving_at_one_edge_partway_through_a_run_and_clear_of_it_are_different_states() {
    let s = [-3.7120, 40.4180];
    let a = [-3.7080, 40.4180];
    let b = [-3.7040, 40.4180];
    let l = [-3.7000, 40.4180];
    let d = [-3.6960, 40.4180];
    let y = [-3.7080, 40.4140];
    let z = [-3.7040, 40.4140];
    let features = vec![
        line("sa", json!([s, a]), true),
        line("ab", json!([a, b]), true),
        line("bl", json!([b, l]), true),
        line("ld", json!([l, d]), true),
        line("sy", json!([s, y]), true),
        line("yz", json!([y, z]), true),
        line("zb", json!([z, b]), true),
    ];
    let container = container_of(&features);
    let graph = graph_of(&container);
    let edge = |name: &str| edge_at(&container, name);
    let forbidden = Turns::from_sequences([vec![edge("ab"), edge("bl"), edge("ld")]]);
    let from = node_at(&container, s[0], s[1]);

    let reached = trace(&graph, &hops(&graph), &forbidden, &[from], Direction::Forward, None).unwrap();

    let at = |name: &str| reached.iter().find(|r| r.edge_index == edge(name)).map(|r| r.at);
    assert_eq!(at("bl"), Some(3.0), "the cheap arrival at the link stands");
    assert_eq!(
        at("ld"),
        Some(5.0),
        "and `ld` is still reached, the long way round — a search settling arcs alone loses it entirely"
    );
}
