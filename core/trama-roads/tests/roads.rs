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
fn the_importer_declares_the_channel_a_router_writes() {
    let imported = import(&extract(vec![way(1, vec![10, 11], vec![A, B], json!({}))])).unwrap();

    assert_eq!(imported.channels.len(), 1);
    assert_eq!(imported.channels[0]["name"], json!("on_route"));
    assert_eq!(imported.channels[0]["entity_kind"], json!("edge"));
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
