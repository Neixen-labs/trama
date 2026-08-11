// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! What the routing solver guarantees over a container it did not build.

use serde_json::{Value, json};
use trama_format::{compile, parse_graph, read_sections};
use trama_routing::{Parameters, plan, solve};

const CHANNEL: fn() -> Value = || json!({"name": "on_route", "entity_kind": "edge", "unit": "1", "min": 0, "max": 1});

fn line(id: &str, coordinates: Value, directed: bool) -> Value {
    let properties = if directed { json!({"_trama_directed": true}) } else { json!({}) };
    json!({"type": "Feature", "id": id, "properties": properties, "geometry": {"type": "LineString", "coordinates": coordinates}})
}

/// Two ways round a block from `a` to `d`: north is two long sides, south is one short one.
///
/// ```text
///   b ----- c          north: a-b, b-c, c-d
///   |       |
///   a ----- d          south: a-d
/// ```
fn block(south_directed: bool) -> Vec<Value> {
    let a = [-3.7040, 40.4160];
    let b = [-3.7040, 40.4200];
    let c = [-3.7000, 40.4200];
    let d = [-3.7000, 40.4160];
    vec![
        line("ab", json!([a, b]), false),
        line("bc", json!([b, c]), false),
        line("cd", json!([c, d]), false),
        line("ad", json!([a, d]), south_directed),
    ]
}

fn graph_of(container: &[u8]) -> trama_format::Graph {
    let sections = read_sections(container).unwrap();
    parse_graph(&sections.iter().find(|s| &s.kind == b"GRPH").unwrap().payload).unwrap()
}

fn lengths_of(container: &[u8]) -> Vec<f64> {
    trama_format::edge_paths(container)
        .unwrap()
        .iter()
        .map(|path| path.windows(2).map(|pair| (pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1)).sum())
        .collect()
}

/// The node index nearest a WGS 84 position, so a test can name a corner rather than an index.
fn node_at(container: &[u8], longitude: f64, latitude: f64) -> usize {
    let exported = trama_format::export(container).unwrap();
    let features = exported.nodes["features"].as_array().unwrap();
    let id = features
        .iter()
        .min_by(|left, right| {
            let distance = |feature: &Value| {
                let point = feature["geometry"]["coordinates"].as_array().unwrap();
                let (x, y) = (point[0].as_f64().unwrap(), point[1].as_f64().unwrap());
                (x - longitude).hypot(y - latitude)
            };
            distance(left).partial_cmp(&distance(right)).unwrap()
        })
        .map(|feature| feature["properties"]["_trama_id"].as_str().unwrap().parse::<u64>().unwrap())
        .unwrap();
    let graph = graph_of(container);
    graph.nodes.iter().position(|node| node.id == id).unwrap()
}

fn parameters(waypoints: Vec<usize>) -> Parameters {
    Parameters { waypoints, step_seconds: 60.0, speed_metres_per_second: 10.0, ..Default::default() }
}

#[test]
fn the_route_takes_the_shorter_way_round() {
    let container = compile(&block(false), &[CHANNEL()], &[]).unwrap();
    let graph = graph_of(&container);
    let lengths = lengths_of(&container);
    let (from, to) = (node_at(&container, -3.7040, 40.4160), node_at(&container, -3.7000, 40.4160));

    let route = plan(&graph, &lengths, &[from, to]).unwrap();

    assert_eq!(route.edges.len(), 1, "the single south side beats three north ones");
}

#[test]
fn a_one_way_edge_cannot_be_crossed_against_its_direction() {
    // `ad` runs a to d. Going d to a must take the long way round even though the short side
    // is right there: this is the failure that still draws correctly on a map.
    let container = compile(&block(true), &[CHANNEL()], &[]).unwrap();
    let graph = graph_of(&container);
    let lengths = lengths_of(&container);
    let (a, d) = (node_at(&container, -3.7040, 40.4160), node_at(&container, -3.7000, 40.4160));

    let with_the_flow = plan(&graph, &lengths, &[a, d]).unwrap();
    let against_it = plan(&graph, &lengths, &[d, a]).unwrap();

    assert_eq!(with_the_flow.edges.len(), 1, "the one-way side is still the shortest way there");
    assert_eq!(against_it.edges.len(), 3, "coming back has to go round");
}

#[test]
fn an_undirected_block_lets_the_short_side_serve_both_ways() {
    let container = compile(&block(false), &[CHANNEL()], &[]).unwrap();
    let graph = graph_of(&container);
    let lengths = lengths_of(&container);
    let (a, d) = (node_at(&container, -3.7040, 40.4160), node_at(&container, -3.7000, 40.4160));

    assert_eq!(plan(&graph, &lengths, &[d, a]).unwrap().edges.len(), 1);
}

#[test]
fn waypoints_are_visited_in_the_order_given() {
    let container = compile(&block(false), &[CHANNEL()], &[]).unwrap();
    let graph = graph_of(&container);
    let lengths = lengths_of(&container);
    let a = node_at(&container, -3.7040, 40.4160);
    let c = node_at(&container, -3.7000, 40.4200);
    let d = node_at(&container, -3.7000, 40.4160);

    let direct = plan(&graph, &lengths, &[a, d]).unwrap();
    let via_c = plan(&graph, &lengths, &[a, c, d]).unwrap();

    assert_eq!(direct.edges.len(), 1);
    assert!(via_c.edges.len() > direct.edges.len(), "a detour is longer than the direct route");
    assert!(via_c.reached_at.windows(2).all(|pair| pair[1] >= pair[0]), "distance never goes backwards");
}

#[test]
fn a_route_needs_at_least_two_waypoints() {
    let container = compile(&block(false), &[CHANNEL()], &[]).unwrap();

    let outcome = plan(&graph_of(&container), &lengths_of(&container), &[0]);

    assert!(outcome.err().unwrap().contains("at least two waypoints"));
}

#[test]
fn a_waypoint_outside_the_graph_is_rejected() {
    let container = compile(&block(false), &[CHANNEL()], &[]).unwrap();

    let outcome = plan(&graph_of(&container), &lengths_of(&container), &[0, 999]);

    assert!(outcome.err().unwrap().contains("names no node"));
}

#[test]
fn an_unreachable_waypoint_is_an_error_rather_than_an_empty_route() {
    // A second block sharing no node with the first.
    let mut features = block(false);
    features.push(line("far", json!([[-3.600, 40.500], [-3.599, 40.501]]), false));
    let container = compile(&features, &[CHANNEL()], &[]).unwrap();
    let graph = graph_of(&container);
    let a = node_at(&container, -3.7040, 40.4160);
    let island = node_at(&container, -3.600, 40.500);

    let outcome = plan(&graph, &lengths_of(&container), &[a, island]);

    assert!(outcome.err().unwrap().contains("no route"));
}

// --- the delta stream ---

#[test]
fn the_stream_is_whole_deltas_marking_progress_along_the_route() {
    let container = compile(&block(false), &[CHANNEL()], &[]).unwrap();
    let a = node_at(&container, -3.7040, 40.4160);
    let c = node_at(&container, -3.7000, 40.4200);

    let stream = solve(&container, &parameters(vec![a, c]), 0.0, 600.0).unwrap();

    assert_eq!(stream.len() % 18, 0, "a delta stream is a whole number of 18-byte records");
    let values: Vec<f32> =
        stream.chunks_exact(18).map(|record| f32::from_le_bytes(record[14..18].try_into().unwrap())).collect();
    assert!(values.iter().all(|value| *value == 0.0 || *value == 1.0), "the channel is reached or not");
    assert!(values.contains(&1.0), "something is reached");
}

#[test]
fn progress_only_grows_as_time_advances() {
    let container = compile(&block(false), &[CHANNEL()], &[]).unwrap();
    let a = node_at(&container, -3.7040, 40.4160);
    let c = node_at(&container, -3.7000, 40.4200);

    let stream = solve(&container, &parameters(vec![a, c]), 0.0, 600.0).unwrap();

    let mut reached_by_time: std::collections::BTreeMap<u32, usize> = std::collections::BTreeMap::new();
    for record in stream.chunks_exact(18) {
        let t = f32::from_le_bytes(record[10..14].try_into().unwrap());
        let value = f32::from_le_bytes(record[14..18].try_into().unwrap());
        *reached_by_time.entry(t as u32).or_default() += (value == 1.0) as usize;
    }
    let counts: Vec<usize> = reached_by_time.values().copied().collect();
    // Every instant carries one delta per route edge, so the first instant's count is the route.
    let route_edges =
        stream.chunks_exact(18).filter(|record| f32::from_le_bytes(record[10..14].try_into().unwrap()) == 0.0).count();
    assert!(counts.windows(2).all(|pair| pair[1] >= pair[0]), "a scrub backwards unwinds, never skips");
    assert_eq!(counts[0], 0, "nothing is behind the vehicle before it sets off");
    assert_eq!(*counts.last().unwrap(), route_edges, "by the end the whole route is behind it");
}

#[test]
fn a_container_not_declaring_the_channel_is_rejected_before_any_work() {
    let container = compile(&block(false), &[], &[]).unwrap();

    let outcome = solve(&container, &parameters(vec![0, 1]), 0.0, 600.0);

    assert!(outcome.err().unwrap().contains("declares no edge channel named 'on_route'"));
}

#[test]
fn a_zero_speed_is_rejected_rather_than_producing_a_stalled_route() {
    let container = compile(&block(false), &[CHANNEL()], &[]).unwrap();
    let stalled = Parameters { speed_metres_per_second: 0.0, ..parameters(vec![0, 1]) };

    assert!(solve(&container, &stalled, 0.0, 600.0).err().unwrap().contains("speed"));
}

// --- travel time ---

/// The same block, with a speed column: the short south side slow, the long north way fast.
fn block_with_speeds(south: f64, north: f64) -> Vec<Value> {
    let a = [-3.7040, 40.4160];
    let b = [-3.7040, 40.4200];
    let c = [-3.7000, 40.4200];
    let d = [-3.7000, 40.4160];
    let road = |id: &str, points: Value, speed: f64| {
        json!({"type": "Feature", "id": id, "properties": {"speed_ms": speed},
               "geometry": {"type": "LineString", "coordinates": points}})
    };
    vec![
        road("ab", json!([a, b]), north),
        road("bc", json!([b, c]), north),
        road("cd", json!([c, d]), north),
        road("ad", json!([a, d]), south),
    ]
}

fn with_speed_column(waypoints: Vec<usize>) -> Parameters {
    Parameters { speed_property: Some("speed_ms".into()), ..parameters(waypoints) }
}

fn routed_edges(stream: &[u8]) -> std::collections::BTreeSet<u64> {
    stream.chunks_exact(18).map(|record| u64::from_le_bytes(record[0..8].try_into().unwrap())).collect()
}

#[test]
fn naming_a_speed_column_turns_the_shortest_route_into_the_fastest_one() {
    // The south side is a third of the distance at a tenth of the speed, so it is slower.
    let container = compile(&block_with_speeds(1.0, 10.0), &[CHANNEL()], &[]).unwrap();
    let graph = graph_of(&container);
    let (a, d) = (node_at(&container, -3.7040, 40.4160), node_at(&container, -3.7000, 40.4160));

    let by_distance = plan(&graph, &lengths_of(&container), &[a, d]).unwrap();
    let stream = solve(&container, &with_speed_column(vec![a, d]), 0.0, 600.0).unwrap();

    assert_eq!(by_distance.edges.len(), 1, "by distance the short side wins");
    assert_eq!(routed_edges(&stream).len(), 3, "by time the long fast way wins");
}

#[test]
fn without_a_speed_column_the_shortest_route_still_wins() {
    let container = compile(&block_with_speeds(1.0, 10.0), &[CHANNEL()], &[]).unwrap();
    let (a, d) = (node_at(&container, -3.7040, 40.4160), node_at(&container, -3.7000, 40.4160));

    let stream = solve(&container, &parameters(vec![a, d]), 0.0, 600.0).unwrap();

    assert_eq!(routed_edges(&stream).len(), 1, "the column is only read when the caller names it");
}

#[test]
fn an_edge_with_no_usable_speed_falls_back_rather_than_failing() {
    // Half a real city has no speed limit tagged; refusing to route it would be useless.
    let mut features = block_with_speeds(10.0, 10.0);
    features[3]["properties"] = json!({"speed_ms": 0.0});
    features[0]["properties"] = json!({});
    let container = compile(&features, &[CHANNEL()], &[]).unwrap();
    let (a, d) = (node_at(&container, -3.7040, 40.4160), node_at(&container, -3.7000, 40.4160));

    let stream = solve(&container, &with_speed_column(vec![a, d]), 0.0, 600.0).unwrap();

    assert!(!stream.is_empty(), "a zero speed and a missing column both fall back to the parameter");
}

#[test]
fn the_arrival_time_is_a_clock_reading_not_a_distance() {
    // At 1 m/s the seconds and the metres coincide numerically, which is what makes this
    // checkable: the first instant an edge reads as reached is its metre count, to the step.
    let container = compile(&block_with_speeds(1.0, 1.0), &[CHANNEL()], &[]).unwrap();
    let (a, d) = (node_at(&container, -3.7040, 40.4160), node_at(&container, -3.7000, 40.4160));
    let route = plan(&graph_of(&container), &lengths_of(&container), &[a, d]).unwrap();

    let stream = solve(&container, &with_speed_column(vec![a, d]), 0.0, 1200.0).unwrap();

    let arrival = stream
        .chunks_exact(18)
        .filter(|record| f32::from_le_bytes(record[14..18].try_into().unwrap()) == 1.0)
        .map(|record| f32::from_le_bytes(record[10..14].try_into().unwrap()))
        .fold(f32::INFINITY, f32::min);
    let metres = route.reached_at[0];
    assert!((arrival as f64 - metres).abs() <= 60.0, "arrival {arrival} against {metres} m at 1 m/s");
}
