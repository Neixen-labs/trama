// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! What the routing solver guarantees over a container it did not build.

use serde_json::{Value, json};
use trama_format::{compile, parse_graph, read_sections};
use trama_routing::Turns;
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

/// The core's own measurement, not a second copy of it: a test that measures differently from
/// the code under test is comparing two answers and calling it a check.
fn lengths_of(container: &[u8]) -> Vec<f64> {
    trama_format::edge_lengths(container).unwrap()
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

    let route = plan(&graph, &lengths, &no_turns(), &[from, to]).unwrap();

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

    let with_the_flow = plan(&graph, &lengths, &no_turns(), &[a, d]).unwrap();
    let against_it = plan(&graph, &lengths, &no_turns(), &[d, a]).unwrap();

    assert_eq!(with_the_flow.edges.len(), 1, "the one-way side is still the shortest way there");
    assert_eq!(against_it.edges.len(), 3, "coming back has to go round");
}

#[test]
fn an_undirected_block_lets_the_short_side_serve_both_ways() {
    let container = compile(&block(false), &[CHANNEL()], &[]).unwrap();
    let graph = graph_of(&container);
    let lengths = lengths_of(&container);
    let (a, d) = (node_at(&container, -3.7040, 40.4160), node_at(&container, -3.7000, 40.4160));

    assert_eq!(plan(&graph, &lengths, &no_turns(), &[d, a]).unwrap().edges.len(), 1);
}

#[test]
fn waypoints_are_visited_in_the_order_given() {
    let container = compile(&block(false), &[CHANNEL()], &[]).unwrap();
    let graph = graph_of(&container);
    let lengths = lengths_of(&container);
    let a = node_at(&container, -3.7040, 40.4160);
    let c = node_at(&container, -3.7000, 40.4200);
    let d = node_at(&container, -3.7000, 40.4160);

    let direct = plan(&graph, &lengths, &no_turns(), &[a, d]).unwrap();
    let via_c = plan(&graph, &lengths, &no_turns(), &[a, c, d]).unwrap();

    assert_eq!(direct.edges.len(), 1);
    assert!(via_c.edges.len() > direct.edges.len(), "a detour is longer than the direct route");
    assert!(via_c.reached_at.windows(2).all(|pair| pair[1] >= pair[0]), "distance never goes backwards");
}

#[test]
fn a_route_needs_at_least_two_waypoints() {
    let container = compile(&block(false), &[CHANNEL()], &[]).unwrap();

    let outcome = plan(&graph_of(&container), &lengths_of(&container), &no_turns(), &[0]);

    assert!(outcome.err().unwrap().contains("at least two waypoints"));
}

#[test]
fn a_waypoint_outside_the_graph_is_rejected() {
    let container = compile(&block(false), &[CHANNEL()], &[]).unwrap();

    let outcome = plan(&graph_of(&container), &lengths_of(&container), &no_turns(), &[0, 999]);

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

    let outcome = plan(&graph, &lengths_of(&container), &no_turns(), &[a, island]);

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
    let values: Vec<f32> = stream
        .as_chunks::<18>()
        .0
        .iter()
        .map(|record| f32::from_le_bytes(record[14..18].try_into().unwrap()))
        .collect();
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
    for record in stream.as_chunks::<18>().0.iter() {
        let t = f32::from_le_bytes(record[10..14].try_into().unwrap());
        let value = f32::from_le_bytes(record[14..18].try_into().unwrap());
        *reached_by_time.entry(t as u32).or_default() += (value == 1.0) as usize;
    }
    let counts: Vec<usize> = reached_by_time.values().copied().collect();
    // Every instant carries one delta per route edge, so the first instant's count is the route.
    let route_edges = stream
        .as_chunks::<18>()
        .0
        .iter()
        .filter(|record| f32::from_le_bytes(record[10..14].try_into().unwrap()) == 0.0)
        .count();
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
    stream.as_chunks::<18>().0.iter().map(|record| u64::from_le_bytes(record[0..8].try_into().unwrap())).collect()
}

#[test]
fn naming_a_speed_column_turns_the_shortest_route_into_the_fastest_one() {
    // The south side is a third of the distance at a tenth of the speed, so it is slower.
    let container = compile(&block_with_speeds(1.0, 10.0), &[CHANNEL()], &[]).unwrap();
    let graph = graph_of(&container);
    let (a, d) = (node_at(&container, -3.7040, 40.4160), node_at(&container, -3.7000, 40.4160));

    let by_distance = plan(&graph, &lengths_of(&container), &no_turns(), &[a, d]).unwrap();
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
    let route = plan(&graph_of(&container), &lengths_of(&container), &no_turns(), &[a, d]).unwrap();

    let stream = solve(&container, &with_speed_column(vec![a, d]), 0.0, 1200.0).unwrap();

    let arrival = stream
        .as_chunks::<18>()
        .0
        .iter()
        .filter(|record| f32::from_le_bytes(record[14..18].try_into().unwrap()) == 1.0)
        .map(|record| f32::from_le_bytes(record[10..14].try_into().unwrap()))
        .fold(f32::INFINITY, f32::min);
    let metres = route.reached_at[0];
    assert!((arrival as f64 - metres).abs() <= 60.0, "arrival {arrival} against {metres} m at 1 m/s");
}

/// No turn restrictions: what every test here but the restriction ones asks for.
fn no_turns() -> Turns {
    Turns::new()
}

/// A junction with a detour: `a-b-c` is the short way, `a-d-c` the long way round.
///
/// ```text
///   a --- b --- c      short: ab, bc
///    \         /
///     --- d ---        long:  ad, dc
/// ```
fn detour() -> Vec<Value> {
    let a = [-3.7040, 40.4160];
    let b = [-3.7000, 40.4160];
    let c = [-3.6960, 40.4160];
    let d = [-3.7000, 40.4100];
    vec![
        line("ab", json!([a, b]), false),
        line("bc", json!([b, c]), false),
        line("ad", json!([a, d]), false),
        line("dc", json!([d, c]), false),
    ]
}

/// The index of the edge declared under `name`, so a test can forbid a turn by name.
fn edge_at(container: &[u8], name: &str) -> usize {
    let id = trama_format::edge_id(name);
    graph_of(container).edges.iter().position(|edge| edge.id == id).expect("the edge is in the graph")
}

fn crossed(container: &[u8], route: &trama_routing::Route) -> Vec<u64> {
    let graph = graph_of(container);
    route.edges.iter().map(|index| graph.edges[*index].id).collect()
}

#[test]
fn a_forbidden_turn_sends_the_route_the_long_way_round() {
    let container = compile(&detour(), &[CHANNEL()], &[]).unwrap();
    let graph = graph_of(&container);
    let lengths = lengths_of(&container);
    let (a, c) = (node_at(&container, -3.7040, 40.4160), node_at(&container, -3.6960, 40.4160));

    let direct = plan(&graph, &lengths, &no_turns(), &[a, c]).unwrap();
    assert_eq!(
        crossed(&container, &direct),
        vec![trama_format::edge_id("ab"), trama_format::edge_id("bc")],
        "with nothing forbidden the short way wins"
    );

    // Coming in along `ab`, you may not continue onto `bc`.
    let forbidden = Turns::from_sequences([vec![edge_at(&container, "ab"), edge_at(&container, "bc")]]);
    let round = plan(&graph, &lengths, &forbidden, &[a, c]).unwrap();

    assert_eq!(
        crossed(&container, &round),
        vec![trama_format::edge_id("ad"), trama_format::edge_id("dc")],
        "the turn is refused, so the detour is the only way"
    );
    let cost = |route: &trama_routing::Route| route.reached_at.last().copied().unwrap();
    assert!(cost(&round) > cost(&direct), "and it costs more, which is what a restriction does");
}

/// Why the search settles arcs and not nodes.
///
/// ```text
///   s --------- v --- t     `sv` is the cheap way to v, and may not continue onto `vt`
///    \         /
///     -- q ---              `qv` is dearer, and may
/// ```
///
/// A search that settled nodes would reach `v` by `sv`, record that as the way to `v`, and then
/// find the only road out closed — reporting either no route at all or a path through a turn it
/// was told not to take. Arriving along `qv` costs more and is the only way through, so the
/// cheapest route to `t` does not contain the cheapest route to `v`. That is exactly the property
/// Dijkstra over nodes assumes, and turn restrictions are where it stops holding.
#[test]
fn the_cheapest_way_to_a_junction_is_not_always_part_of_the_cheapest_way_through_it() {
    let s = [-3.7080, 40.4160];
    let v = [-3.7000, 40.4160];
    let q = [-3.7040, 40.4100];
    let t = [-3.6960, 40.4160];
    let features = vec![
        line("sv", json!([s, v]), false),
        line("sq", json!([s, q]), false),
        line("qv", json!([q, v]), false),
        line("vt", json!([v, t]), false),
    ];
    let container = compile(&features, &[CHANNEL()], &[]).unwrap();
    let graph = graph_of(&container);
    let lengths = lengths_of(&container);
    let (start, end) = (node_at(&container, -3.7080, 40.4160), node_at(&container, -3.6960, 40.4160));

    let forbidden = Turns::from_sequences([vec![edge_at(&container, "sv"), edge_at(&container, "vt")]]);
    let route = plan(&graph, &lengths, &forbidden, &[start, end]).unwrap();

    assert_eq!(
        crossed(&container, &route),
        vec![trama_format::edge_id("sq"), trama_format::edge_id("qv"), trama_format::edge_id("vt")],
        "the route reaches v the dear way, because the cheap way cannot leave it"
    );
}

/// The whole path, from a column in the file to a route that honours it: this is what the road
/// importer writes and what a caller gets by naming the column.
#[test]
fn the_solver_reads_the_restriction_column_the_importer_wrote() {
    let mut features = detour();
    features[0]["properties"]["roads:no_turn"] = json!(trama_format::edge_id("bc").to_string());
    let container = compile(&features, &[CHANNEL()], &[]).unwrap();
    let (a, c) = (node_at(&container, -3.7040, 40.4160), node_at(&container, -3.6960, 40.4160));

    // Without naming the column the file's restriction is inert: a container carries the fact,
    // and a caller decides whether this question is one it applies to.
    let ignored = solve(&container, &parameters(vec![a, c]), 0.0, 0.0).unwrap();
    let honoured = solve(
        &container,
        &Parameters { restriction_property: Some("roads:no_turn".into()), ..parameters(vec![a, c]) },
        0.0,
        0.0,
    )
    .unwrap();

    // The detour has as many edges as the short way, so counting deltas would prove nothing: it
    // is *which* edges were written that changed.
    assert_eq!(
        routed_edges(&ignored),
        std::collections::BTreeSet::from([trama_format::edge_id("ab"), trama_format::edge_id("bc")])
    );
    assert_eq!(
        routed_edges(&honoured),
        std::collections::BTreeSet::from([trama_format::edge_id("ad"), trama_format::edge_id("dc")]),
        "naming the column is what makes the file's own restriction bite"
    );
}

/// The dual carriageway, end to end: a run of three edges the route may not make.
///
/// ```text
///        a ----ab----> b ----bc----> c        `ab` then `bl` then `ld` is the U-turn
///                      |
///                     bl  (the link)
///                      |
///        e <---le----- l <---dl----- d
/// ```
///
/// Every edge here is crossable and every pair of them is a legal movement. Only the three in that
/// order are refused, which is why nothing shorter than a run can say it: forbidding `ab` then
/// `bl` would close the link, and forbidding `bl` then `ld` would also close it to traffic coming
/// the other way, which the sign does not.
#[test]
fn a_run_of_three_edges_is_refused_where_every_pair_in_it_is_allowed() {
    let a = [-3.7080, 40.4180];
    let b = [-3.7040, 40.4180];
    let c = [-3.7000, 40.4180];
    let l = [-3.7040, 40.4160];
    let d = [-3.7000, 40.4160];
    let e = [-3.7080, 40.4160];
    let features = vec![
        line("ab", json!([a, b]), false),
        line("bc", json!([b, c]), false),
        line("bl", json!([b, l]), false),
        line("ld", json!([l, d]), false),
        line("le", json!([l, e]), false),
    ];
    let container = compile(&features, &[CHANNEL()], &[]).unwrap();
    let graph = graph_of(&container);
    let lengths = lengths_of(&container);
    let (start, target) = (node_at(&container, -3.7080, 40.4180), node_at(&container, -3.7000, 40.4160));
    let edge = |name: &str| edge_at(&container, name);

    // Unrestricted, the U-turn is the short way: down the link and straight along.
    let direct = plan(&graph, &lengths, &no_turns(), &[start, target]).unwrap();
    assert_eq!(
        crossed(&container, &direct),
        vec![trama_format::edge_id("ab"), trama_format::edge_id("bl"), trama_format::edge_id("ld")]
    );

    let forbidden = Turns::from_sequences([vec![edge("ab"), edge("bl"), edge("ld")]]);
    let round = plan(&graph, &lengths, &forbidden, &[start, target]).unwrap();

    // The run is refused, so the route has to reach the far carriageway some other way.
    assert_ne!(crossed(&container, &round), crossed(&container, &direct), "the forbidden run was driven anyway");
    let ids = crossed(&container, &round);
    let consecutive = ids.windows(3).any(|window| {
        window == [trama_format::edge_id("ab"), trama_format::edge_id("bl"), trama_format::edge_id("ld")]
    });
    assert!(!consecutive, "the three edges appear in the forbidden order: {ids:?}");

    // And the link itself is still open — reaching `e` across it is untouched, which is the whole
    // reason a run cannot be flattened into a pair.
    let across = plan(&graph, &lengths, &forbidden, &[start, node_at(&container, -3.7080, 40.4160)]).unwrap();
    assert!(
        crossed(&container, &across).contains(&trama_format::edge_id("bl")),
        "forbidding the run closed the link, which forbids a movement nobody forbade"
    );
}

/// Why the progress belongs in the settled state, not merely in the check on each step.
///
/// ```text
///   s --sa--> a --ab--> b --bl--> l --ld--> d     `ab` then `bl` then `ld` is refused
///    \                 /
///     --sy--> y --yz--> z --zb-->
/// ```
///
/// The search reaches `bl` twice: cheaply along `ab`, partway through the forbidden run, and
/// dearly along `zb`, clear of it. Those are different situations on one edge — the first may not
/// go on to `ld` and the second may. Keeping only the cheaper one, as a search settling arcs alone
/// would, leaves `d` unreachable and reports no route to a place a driver can plainly get to.
///
/// It is the node-settling bug of the turn restrictions one level further in, and it is invisible
/// to any test whose forbidden run can be reached only one way.
#[test]
fn the_cheapest_way_onto_an_edge_is_not_always_part_of_the_cheapest_way_past_it() {
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
    let container = compile(&features, &[CHANNEL()], &[]).unwrap();
    let graph = graph_of(&container);
    let lengths = lengths_of(&container);
    let (start, target) = (node_at(&container, s[0], s[1]), node_at(&container, d[0], d[1]));
    let edge = |name: &str| edge_at(&container, name);

    let direct = plan(&graph, &lengths, &no_turns(), &[start, target]).unwrap();
    assert_eq!(
        crossed(&container, &direct),
        ["sa", "ab", "bl", "ld"].map(trama_format::edge_id).to_vec(),
        "unrestricted, the short way is the one through the run"
    );

    let forbidden = Turns::from_sequences([vec![edge("ab"), edge("bl"), edge("ld")]]);
    let round = plan(&graph, &lengths, &forbidden, &[start, target])
        .unwrap_or_else(|error| panic!("the long way exists and the search lost it: {error}"));

    assert_eq!(
        crossed(&container, &round),
        ["sy", "yz", "zb", "bl", "ld"].map(trama_format::edge_id).to_vec(),
        "the route reaches the link the dear way, because the cheap way may not leave it"
    );
}
