// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! What the road importer promises the core, which never learns what `oneway` means.

use serde_json::{Value, json};
use trama_roads::import;

/// An Overpass `out geom;` element: node references and positions, in step with each other.
fn way(id: u64, nodes: Vec<u64>, points: Vec<[f64; 2]>, tags: Value) -> Value {
    json!({
        "type": "way",
        "id": id,
        "nodes": nodes,
        "geometry": points.iter().map(|p| json!({"lat": p[1], "lon": p[0]})).collect::<Vec<Value>>(),
        "tags": tags,
    })
}

fn extract(ways: Vec<Value>) -> String {
    json!({"elements": ways}).to_string()
}

fn coordinates(feature: &Value) -> Vec<[f64; 2]> {
    feature["geometry"]["coordinates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pair| [pair[0].as_f64().unwrap(), pair[1].as_f64().unwrap()])
        .collect()
}

const A: [f64; 2] = [-3.704, 40.416];
const B: [f64; 2] = [-3.703, 40.417];
const C: [f64; 2] = [-3.702, 40.418];

#[test]
fn a_one_way_street_is_marked_directed_for_the_core() {
    let imported = import(&extract(vec![way(1, vec![10, 11], vec![A, B], json!({"oneway": "yes"}))])).unwrap();

    assert_eq!(imported.features[0]["properties"]["_trama_directed"], json!(true));
}

#[test]
fn every_spelling_osm_uses_for_one_way_counts() {
    for spelling in ["yes", "true", "1", "-1"] {
        let imported = import(&extract(vec![way(1, vec![10, 11], vec![A, B], json!({"oneway": spelling}))])).unwrap();

        assert_eq!(imported.features[0]["properties"]["_trama_directed"], json!(true), "oneway={spelling}");
    }
}

#[test]
fn a_two_way_street_carries_no_direction_key_at_all() {
    for spelling in [json!({}), json!({"oneway": "no"}), json!({"oneway": "false"})] {
        let imported = import(&extract(vec![way(1, vec![10, 11], vec![A, B], spelling.clone())])).unwrap();

        assert!(
            imported.features[0]["properties"].get("_trama_directed").is_none(),
            "absence means undirected, and an explicit false would be a non-boolean argument to have"
        );
    }
}

#[test]
fn a_reversible_street_is_not_treated_as_one_way() {
    // It changes direction on a schedule, and the format has nowhere to put a schedule.
    let imported = import(&extract(vec![way(1, vec![10, 11], vec![A, B], json!({"oneway": "reversible"}))])).unwrap();

    assert!(imported.features[0]["properties"].get("_trama_directed").is_none());
}

#[test]
fn a_street_running_against_its_geometry_is_stored_reversed() {
    // SPEC 9 gives an edge no sign, so `-1` has to become vertex order.
    let forward = import(&extract(vec![way(1, vec![10, 11], vec![A, B], json!({"oneway": "yes"}))])).unwrap();
    let backward = import(&extract(vec![way(1, vec![10, 11], vec![A, B], json!({"oneway": "-1"}))])).unwrap();

    assert_eq!(coordinates(&forward.features[0]), vec![A, B]);
    assert_eq!(coordinates(&backward.features[0]), vec![B, A]);
    assert_eq!(backward.features[0]["properties"]["_trama_directed"], json!(true));
}

// --- junctions ---

#[test]
fn a_way_is_cut_where_another_way_crosses_it_mid_span() {
    // Node 11 sits in the middle of the first way and at the end of the second. Handed over
    // whole, the two would cross on the map and never meet in the graph.
    let imported = import(&extract(vec![
        way(1, vec![10, 11, 12], vec![A, B, C], json!({})),
        way(2, vec![20, 11], vec![[-3.705, 40.418], B], json!({})),
    ]))
    .unwrap();

    let first: Vec<Vec<[f64; 2]>> = imported
        .features
        .iter()
        .filter(|f| f["id"].as_str().unwrap().starts_with("osm:way/1/"))
        .map(coordinates)
        .collect();
    assert_eq!(first.len(), 2, "the crossed way is split in two");
    assert_eq!(first[0], vec![A, B]);
    assert_eq!(first[1], vec![B, C], "both pieces keep the shared vertex, which is what joins them");
}

#[test]
fn a_way_nothing_crosses_stays_whole() {
    let imported = import(&extract(vec![way(1, vec![10, 11, 12], vec![A, B, C], json!({}))])).unwrap();

    assert_eq!(imported.features.len(), 1);
    assert_eq!(coordinates(&imported.features[0]), vec![A, B, C]);
}

#[test]
fn splitting_keeps_each_piece_pointing_the_way_traffic_goes() {
    let imported = import(&extract(vec![
        way(1, vec![10, 11, 12], vec![A, B, C], json!({"oneway": "-1"})),
        way(2, vec![20, 11], vec![[-3.705, 40.418], B], json!({})),
    ]))
    .unwrap();

    let pieces: Vec<Vec<[f64; 2]>> = imported
        .features
        .iter()
        .filter(|f| f["id"].as_str().unwrap().starts_with("osm:way/1/"))
        .map(coordinates)
        .collect();
    // Reversed first, then split: traffic runs C to B to A, and each piece says so.
    assert_eq!(pieces[0], vec![C, B]);
    assert_eq!(pieces[1], vec![B, A]);
}

#[test]
fn a_way_whose_node_list_does_not_match_its_geometry_is_left_whole() {
    // Post-processed extracts do this. One long edge is a worse graph, but a true one.
    let imported = import(&extract(vec![
        way(1, vec![10], vec![A, B, C], json!({})),
        way(2, vec![20, 10], vec![[-3.705, 40.418], A], json!({})),
    ]))
    .unwrap();

    let first: Vec<&Value> =
        imported.features.iter().filter(|f| f["id"].as_str().unwrap().starts_with("osm:way/1/")).collect();
    assert_eq!(first.len(), 1);
}

// --- what reaches the compiler ---

#[test]
fn tags_arrive_namespaced_so_they_cannot_collide_with_a_reserved_key() {
    let imported =
        import(&extract(vec![way(1, vec![10, 11], vec![A, B], json!({"highway": "residential", "maxspeed": 30}))]))
            .unwrap();

    let properties = &imported.features[0]["properties"];
    assert_eq!(properties["osm:highway"], json!("residential"));
    assert_eq!(properties["osm:maxspeed"], json!("30"), "a tag value stays the text OSM stores");
}

#[test]
fn the_importer_declares_every_channel_a_solver_may_write() {
    let imported = import(&extract(vec![way(1, vec![10, 11], vec![A, B], json!({}))])).unwrap();

    // A solver may only write where the file says it may, so an undeclared channel is a
    // calculation refused. Three of these are topological and say nothing about roads.
    let names: Vec<&str> = imported.channels.iter().map(|channel| channel["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["on_route", "reach", "isolated", "critical", "vehicle"]);
    assert!(imported.channels.iter().all(|channel| channel["entity_kind"] == json!("edge")));

    // The first four are readings between nothing and everything; the fifth is an identity, and a
    // fleet's size is not something the file can know. Declaring a range on it would have the host
    // reject the fifth van's deltas as invalid.
    let ranged: Vec<bool> = imported.channels.iter().map(|channel| channel.get("max").is_some()).collect();
    assert_eq!(ranged, [true, true, true, true, false]);
}

#[test]
fn ids_are_stable_and_name_the_way_they_came_from() {
    let imported = import(&extract(vec![way(4328790, vec![10, 11], vec![A, B], json!({}))])).unwrap();

    assert_eq!(imported.features[0]["id"], json!("osm:way/4328790/0"));
}

#[test]
fn an_extract_without_geometry_says_which_query_to_run() {
    let outcome = import(&json!({"elements": [{"type": "way", "id": 1, "nodes": [10, 11]}]}).to_string());

    assert!(outcome.err().unwrap().contains("out geom"));
}

#[test]
fn something_that_is_not_an_overpass_response_is_rejected() {
    assert!(import("{\"type\":\"FeatureCollection\"}").err().unwrap().contains("elements"));
    assert!(import("not json at all").err().unwrap().contains("not JSON"));
}

#[test]
fn nodes_and_relations_in_the_extract_are_ignored_rather_than_failing_it() {
    let imported = import(&extract(vec![
        json!({"type": "node", "id": 10, "lat": 40.416, "lon": -3.704}),
        way(1, vec![10, 11], vec![A, B], json!({})),
        json!({"type": "relation", "id": 5, "members": []}),
    ]))
    .unwrap();

    assert_eq!(imported.features.len(), 1);
}

// --- travel speed ---

fn speed_of(tags: Value) -> f64 {
    let imported = import(&extract(vec![way(1, vec![10, 11], vec![A, B], tags)])).unwrap();
    imported.features[0]["properties"]["roads:speed_ms"].as_f64().unwrap()
}

#[test]
fn a_tagged_speed_limit_arrives_in_metres_per_second() {
    // A solver must not have to know what `maxspeed` is, nor that OSM counts in km/h.
    assert!((speed_of(json!({"maxspeed": "50"})) - 13.888).abs() < 0.01);
    assert!((speed_of(json!({"maxspeed": "30"})) - 8.333).abs() < 0.01);
}

#[test]
fn miles_per_hour_are_converted_rather_than_read_as_kilometres() {
    // 30 mph is 48.3 km/h. Read as km/h it would be 30, and every British road would be slow.
    assert!((speed_of(json!({"maxspeed": "30 mph"})) - 13.411).abs() < 0.01);
}

#[test]
fn osms_named_speeds_are_understood() {
    assert!((speed_of(json!({"maxspeed": "walk"})) - 1.944).abs() < 0.01);
    // `none` is a derestricted motorway, not a missing limit.
    assert!((speed_of(json!({"maxspeed": "none"})) - 36.111).abs() < 0.01);
}

#[test]
fn a_street_with_no_limit_falls_back_to_its_road_class() {
    // 43% of the sample extract carries `maxspeed`; without this most of a city has no cost.
    assert!((speed_of(json!({"highway": "living_street"})) - 5.555).abs() < 0.01);
    assert!((speed_of(json!({"highway": "residential"})) - 8.333).abs() < 0.01);
    assert!((speed_of(json!({"highway": "primary"})) - 13.888).abs() < 0.01);
}

/// The classes a fast road is actually tagged with, which the fallback used to miss.
///
/// It mattered rather than being tidiness: of the 265 `trunk` ways in the published Teruel
/// extract only 29 declare a `maxspeed`, so 236 stretches of dual carriageway were costed at the
/// 30 km/h a residential street gets. Every route and every isochrone that used one was wrong in
/// the same direction, and nothing in the output said so — a plausible number is the hardest kind
/// of error to see.
#[test]
fn a_fast_road_with_no_limit_is_not_costed_as_a_residential_street() {
    assert!((speed_of(json!({"highway": "motorway"})) - 33.333).abs() < 0.01, "120 km/h");
    assert!((speed_of(json!({"highway": "trunk"})) - 25.0).abs() < 0.01, "90 km/h");
}

/// A slip road is slower than the road it joins and faster than the street it lands on.
///
/// Giving a link its parent's speed would make every interchange look like a shortcut; leaving it
/// on the 30 km/h default makes the only legal way onto a motorway look like a back street. Both
/// distort the same junction, in opposite directions.
#[test]
fn a_slip_road_is_costed_between_the_roads_it_joins() {
    let (motorway, link, residential) = (
        speed_of(json!({"highway": "motorway"})),
        speed_of(json!({"highway": "motorway_link"})),
        speed_of(json!({"highway": "residential"})),
    );

    assert!(link < motorway, "a slip road is not the motorway: {link} against {motorway}");
    assert!(link > residential, "nor a back street: {link} against {residential}");
    for class in ["trunk_link", "primary_link", "secondary_link", "tertiary_link"] {
        let speed = speed_of(json!({"highway": class}));
        assert!(speed > residential, "{class} fell through to the residential default: {speed}");
    }
    // A declared limit still wins over the class, on a link like anywhere else.
    assert!((speed_of(json!({"highway": "motorway_link", "maxspeed": "40"})) - 11.111).abs() < 0.01);
}

#[test]
fn an_unparseable_limit_falls_back_instead_of_reading_as_zero() {
    // `signals` and `variable` appear in real data. A zero would make the edge infinitely slow.
    for nonsense in ["signals", "variable", "", "RO:urban"] {
        let speed = speed_of(json!({"maxspeed": nonsense, "highway": "residential"}));
        assert!((speed - 8.333).abs() < 0.01, "maxspeed={nonsense} gave {speed}");
    }
}

/// An OSM turn restriction: come in along `from`, and at the `via` node you may not take `to`.
fn restriction(id: u64, from: u64, via: u64, to: u64, kind: &str) -> Value {
    json!({
        "type": "relation",
        "id": id,
        "members": [
            {"type": "way", "ref": from, "role": "from"},
            {"type": "node", "ref": via, "role": "via"},
            {"type": "way", "ref": to, "role": "to"},
        ],
        "tags": {"type": "restriction", "restriction": kind},
    })
}

/// Three ways meeting at node 11: the way in, and two ways out.
fn junction() -> Vec<Value> {
    vec![
        way(1, vec![10, 11], vec![A, B], json!({"highway": "residential"})),
        way(2, vec![11, 12], vec![B, C], json!({"highway": "residential"})),
        way(3, vec![11, 13], vec![B, [-3.701, 40.416]], json!({"highway": "residential"})),
    ]
}

fn no_turn(imported: &trama_format::Import, id: &str) -> Vec<u64> {
    let feature = imported.features.iter().find(|f| f["id"] == json!(id)).expect("the piece is there");
    match feature["properties"].get("roads:no_turn") {
        None => Vec::new(),
        Some(value) => value.as_str().unwrap().split(' ').map(|id| id.parse().unwrap()).collect(),
    }
}

#[test]
fn a_forbidden_turn_names_the_edge_it_forbids() {
    let mut elements = junction();
    elements.push(restriction(100, 1, 11, 2, "no_left_turn"));
    let imported = import(&extract(elements)).unwrap();

    // The way in carries the id of the way it may not turn into, and nothing else.
    assert_eq!(no_turn(&imported, "osm:way/1/0"), vec![trama_format::edge_id("osm:way/2/0")]);
    // The other exit is untouched, and so is every edge the relation did not name.
    assert!(no_turn(&imported, "osm:way/2/0").is_empty());
    assert!(no_turn(&imported, "osm:way/3/0").is_empty());
}

/// `only_straight_on` is the same statement inverted, and inverting it is the importer's job
/// rather than the router's: one question — may I go from here to there — with one answer.
#[test]
fn a_mandatory_turn_forbids_every_other_exit() {
    let mut elements = junction();
    elements.push(restriction(100, 1, 11, 2, "only_straight_on"));
    let imported = import(&extract(elements)).unwrap();

    assert_eq!(no_turn(&imported, "osm:way/1/0"), vec![trama_format::edge_id("osm:way/3/0")]);
}

/// A restriction is about one junction, not about a way everywhere it goes. Way 1 runs 9-10-11
/// and is split at 10 by another street; the `via` is 11, so only the half that reaches 11 may
/// forbid anything. The other half ends a junction away and a driver on it has not arrived yet.
#[test]
fn only_the_piece_that_reaches_the_junction_carries_the_restriction() {
    let mut elements = vec![
        way(1, vec![9, 10, 11], vec![[-3.706, 40.414], A, B], json!({"highway": "residential"})),
        way(2, vec![11, 12], vec![B, C], json!({"highway": "residential"})),
        // Touches node 10, which is what makes way 1 split there.
        way(4, vec![10, 15], vec![A, [-3.705, 40.410]], json!({"highway": "residential"})),
    ];
    elements.push(restriction(100, 1, 11, 2, "no_right_turn"));
    let imported = import(&extract(elements)).unwrap();

    assert_eq!(no_turn(&imported, "osm:way/1/1"), vec![trama_format::edge_id("osm:way/2/0")]);
    assert!(no_turn(&imported, "osm:way/1/0").is_empty(), "a piece that stops short of the junction forbids nothing");
}

/// A `from` way that runs *through* the `via` node rather than ending at it is ambiguous — OSM
/// asks for it to be split, and both halves are equally "arriving along way 1". Both halves get
/// the restriction: forbidding a turn that was allowed costs a longer route, and allowing one that
/// was forbidden sends a driver the wrong way up a junction.
#[test]
fn a_way_running_through_the_via_node_forbids_the_turn_from_both_sides() {
    let mut elements = vec![
        way(1, vec![10, 11, 14], vec![A, B, [-3.700, 40.420]], json!({"highway": "residential"})),
        way(2, vec![11, 12], vec![B, C], json!({"highway": "residential"})),
    ];
    elements.push(restriction(100, 1, 11, 2, "no_right_turn"));
    let imported = import(&extract(elements)).unwrap();

    let forbidden = vec![trama_format::edge_id("osm:way/2/0")];
    assert_eq!(no_turn(&imported, "osm:way/1/0"), forbidden);
    assert_eq!(no_turn(&imported, "osm:way/1/1"), forbidden, "both halves arrive along way 1");
}

/// A restriction whose `via` is a way spans two junctions and cannot be a property of one edge.
/// It is skipped rather than half-applied — applying it to either junction alone would forbid a
/// movement the relation does not forbid.
#[test]
fn a_restriction_across_a_via_way_is_skipped_rather_than_guessed() {
    let mut elements = junction();
    elements.push(json!({
        "type": "relation",
        "id": 100,
        "members": [
            {"type": "way", "ref": 1, "role": "from"},
            {"type": "way", "ref": 2, "role": "via"},
            {"type": "way", "ref": 3, "role": "to"},
        ],
        "tags": {"type": "restriction", "restriction": "no_u_turn"},
    }));
    let imported = import(&extract(elements)).unwrap();

    for id in ["osm:way/1/0", "osm:way/2/0", "osm:way/3/0"] {
        assert!(no_turn(&imported, id).is_empty(), "{id} carries a restriction nobody could place");
    }
}

/// `restriction=give_way` describes priority, not permission, and nothing may be forbidden by it.
#[test]
fn a_relation_that_is_not_a_prohibition_forbids_nothing() {
    let mut elements = junction();
    elements.push(restriction(100, 1, 11, 2, "give_way"));
    let imported = import(&extract(elements)).unwrap();

    assert!(no_turn(&imported, "osm:way/1/0").is_empty());
}

/// `no_u_turn` names the same way as both `from` and `to`: do not come in here and go back out
/// the way you came. It is the one restriction whose forbidden exit is the arriving edge itself,
/// and dropping that case leaves a quarter of a real city's restrictions forbidding nothing.
#[test]
fn a_no_u_turn_forbids_going_back_out_the_way_you_came() {
    let mut elements = junction();
    elements.push(restriction(100, 1, 11, 1, "no_u_turn"));
    let imported = import(&extract(elements)).unwrap();

    assert_eq!(no_turn(&imported, "osm:way/1/0"), vec![trama_format::edge_id("osm:way/1/0")]);
}

/// An `only_*` names the single movement allowed and forbids the other exits. Turning back is not
/// an exit it named, and OSM has `no_u_turn` for that — so the two spellings stay distinct.
#[test]
fn a_mandatory_turn_does_not_quietly_forbid_the_u_turn_as_well() {
    let mut elements = junction();
    elements.push(restriction(100, 1, 11, 2, "only_straight_on"));
    let imported = import(&extract(elements)).unwrap();

    let forbidden = no_turn(&imported, "osm:way/1/0");
    assert_eq!(forbidden, vec![trama_format::edge_id("osm:way/3/0")]);
    assert!(!forbidden.contains(&trama_format::edge_id("osm:way/1/0")), "the U-turn is no_u_turn's to forbid");
}
