// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! What the format guarantees, independent of the implementation that wrote it.

use serde_json::{Value, json};
use trama_format::{Extra, compile, parse_graph, read_sections};

fn line(id: &str, coordinates: Value, properties: Value) -> Value {
    json!({"type": "Feature", "id": id, "properties": properties, "geometry": {"type": "LineString", "coordinates": coordinates}})
}

fn point(coordinates: Value, properties: Value) -> Value {
    json!({"type": "Feature", "properties": properties, "geometry": {"type": "Point", "coordinates": coordinates}})
}

fn graph_of(container: &[u8]) -> trama_format::Graph {
    let sections = read_sections(container).unwrap();
    parse_graph(&sections.iter().find(|s| &s.kind == b"GRPH").unwrap().payload).unwrap()
}

fn columns(container: &[u8]) -> (u32, u32) {
    let sections = read_sections(container).unwrap();
    let payload = &sections.iter().find(|s| &s.kind == b"PROP").unwrap().payload;
    let at = |offset: usize| u32::from_le_bytes(payload[offset..offset + 4].try_into().unwrap());
    (at(12), at(16))
}

// --- node identity, SPEC 4.2 ---

#[test]
fn endpoints_differing_in_their_last_bit_are_one_node() {
    let shared = -3.704f64;
    let drifted = f64::from_bits(shared.to_bits() - 1);

    let container = compile(
        &[
            line("a", json!([[-3.705, 40.415], [shared, 40.416]]), json!({})),
            line("b", json!([[drifted, 40.416], [-3.703, 40.417]]), json!({})),
        ],
        &[],
        &[],
    )
    .unwrap();

    assert_eq!(graph_of(&container).nodes.len(), 3);
}

#[test]
fn endpoints_a_metre_apart_stay_two_nodes() {
    // A cell is about 4 cm, so a metre must survive as two nodes: the grid joins what the file
    // cannot tell apart, and nothing coarser than that.
    let metre = 1.0 / 111_320.0;

    let container = compile(
        &[
            line("a", json!([[-3.705, 40.415], [-3.704, 40.416]]), json!({})),
            line("b", json!([[-3.704, 40.416 + metre], [-3.703, 40.417]]), json!({})),
        ],
        &[],
        &[],
    )
    .unwrap();

    assert_eq!(graph_of(&container).nodes.len(), 4);
}

#[test]
fn a_node_on_a_tile_boundary_joins_across_the_boundary() {
    // Each edge quantizes this endpoint inside a different tile, one as qx = 65535 and the
    // other as qx = 0. SPEC 4.2 makes those one cell.
    let boundary = -3.66943359375;

    let container = compile(
        &[
            line("a", json!([[boundary - 0.01, 40.415], [boundary, 40.416]]), json!({})),
            line("b", json!([[boundary, 40.416], [boundary + 0.01, 40.417]]), json!({})),
        ],
        &[],
        &[],
    )
    .unwrap();

    assert_eq!(graph_of(&container).nodes.len(), 3);
}

#[test]
fn a_declared_id_wins_over_a_derived_one() {
    let container = compile(
        &[line("a", json!([[-3.704, 40.416], [-3.703, 40.417]]), json!({"_trama_id": "42", "loss": 1.5}))],
        &[],
        &[],
    )
    .unwrap();

    assert_eq!(graph_of(&container).edges[0].id, 42);
    let sections = read_sections(&container).unwrap();
    let properties = &sections.iter().find(|s| &s.kind == b"PROP").unwrap().payload;
    assert!(!properties.windows(9).any(|window| window == b"_trama_id"));
}

#[test]
fn a_malformed_id_is_rejected_rather_than_silently_replaced() {
    for declared in ["not-a-number", "18446744073709551616", "-1"] {
        let outcome = compile(
            &[line("a", json!([[-3.704, 40.416], [-3.703, 40.417]]), json!({"_trama_id": declared}))],
            &[],
            &[],
        );
        assert!(outcome.is_err(), "{declared} was accepted");
    }
}

// --- typed properties, SPEC 5 ---

#[test]
fn point_properties_become_node_columns() {
    let container = compile(
        &[
            line("a", json!([[-3.704, 40.416], [-3.703, 40.417]]), json!({"loss": 1.5})),
            point(json!([-3.704, 40.416]), json!({"elevation": 710.0, "name": "J-10"})),
        ],
        &[],
        &[],
    )
    .unwrap();

    // A key gets a column only for the kind that uses it: an all-absent edge column would
    // claim edges have an elevation and merely never said which.
    assert_eq!(columns(&container), (2, 1));
}

#[test]
fn a_property_mixing_types_across_features_is_rejected() {
    let outcome = compile(
        &[
            line("a", json!([[-3.704, 40.416], [-3.703, 40.417]]), json!({"label": "north"})),
            line("b", json!([[-3.703, 40.417], [-3.702, 40.418]]), json!({"label": 3})),
        ],
        &[],
        &[],
    );

    assert!(outcome.unwrap_err().contains("mixes conflicting types"));
}

// --- opaque records, SPEC 7 ---

#[test]
fn two_records_with_the_same_owner_and_media_type_are_rejected() {
    let outcome = compile(
        &[line("a", json!([[-3.704, 40.416], [-3.703, 40.417]]), json!({}))],
        &[],
        &[
            Extra { owner: "epanet".into(), media_type: "text/plain".into(), payload: b"one".to_vec() },
            Extra { owner: "epanet".into(), media_type: "text/plain".into(), payload: b"two".to_vec() },
        ],
    );

    assert!(outcome.unwrap_err().contains("owner and media type"));
}

#[test]
fn an_owner_that_is_not_a_solver_id_is_rejected() {
    let outcome = compile(
        &[line("a", json!([[-3.704, 40.416], [-3.703, 40.417]]), json!({}))],
        &[],
        &[Extra { owner: "EPANET 2.2".into(), media_type: "text/plain".into(), payload: b"x".to_vec() }],
    );

    assert!(outcome.unwrap_err().contains("owner"));
}

#[test]
fn extras_are_ordered_so_the_file_stays_reproducible() {
    let features = [line("a", json!([[-3.704, 40.416], [-3.703, 40.417]]), json!({}))];
    let one = || Extra { owner: "a-owner".into(), media_type: "text/plain".into(), payload: b"one".to_vec() };
    let two = || Extra { owner: "b-owner".into(), media_type: "text/plain".into(), payload: b"two".to_vec() };

    let first = compile(&features, &[], &[two(), one()]).unwrap();
    let second = compile(&features, &[], &[one(), two()]).unwrap();

    assert_eq!(first, second);
}

#[test]
fn an_extra_is_written_optional_so_an_older_reader_skips_it() {
    let container = compile(
        &[line("a", json!([[-3.704, 40.416], [-3.703, 40.417]]), json!({}))],
        &[],
        &[Extra { owner: "epanet".into(), media_type: "text/plain".into(), payload: b"x".to_vec() }],
    )
    .unwrap();

    let count = u32::from_le_bytes(container[0x20..0x24].try_into().unwrap()) as usize;
    for index in 0..count {
        let record = 64 + index * 64;
        let flags = u32::from_le_bytes(container[record + 4..record + 8].try_into().unwrap());
        let required = &container[record..record + 4] != b"XTRA";
        assert_eq!(flags & 1 != 0, required, "record {index} carries the wrong required bit");
    }
}

// --- declared channels, SPEC 6 ---

#[test]
fn a_channel_declaring_half_a_range_is_rejected() {
    let outcome = compile(
        &[line("a", json!([[-3.704, 40.416], [-3.703, 40.417]]), json!({}))],
        &[json!({"name": "flow", "entity_kind": "edge", "unit": "1", "min": -50})],
        &[],
    );

    assert!(outcome.unwrap_err().contains("half a range"));
}

#[test]
fn a_channel_for_neither_a_node_nor_an_edge_is_rejected() {
    let outcome = compile(
        &[line("a", json!([[-3.704, 40.416], [-3.703, 40.417]]), json!({}))],
        &[json!({"name": "flow", "entity_kind": "tile", "unit": "1"})],
        &[],
    );

    assert!(outcome.unwrap_err().contains("node or an edge"));
}

// --- framing ---

#[test]
fn a_line_with_one_coordinate_is_rejected() {
    let outcome = compile(&[line("a", json!([[-3.704, 40.416]]), json!({}))], &[], &[]);

    assert!(outcome.unwrap_err().contains("at least two coordinates"));
}

#[test]
fn duplicate_feature_ids_are_rejected() {
    let outcome = compile(
        &[
            line("a", json!([[-3.704, 40.416], [-3.703, 40.417]]), json!({})),
            line("a", json!([[-3.703, 40.417], [-3.702, 40.418]]), json!({})),
        ],
        &[],
        &[],
    );

    assert!(outcome.unwrap_err().contains("unique"));
}

// --- directed edges, SPEC 4 and 9 ---

/// Two segments meeting at a shared node, so adjacency has something to be asymmetric about.
fn pair(directed: Value) -> Vec<Value> {
    vec![
        line("a", json!([[-3.704, 40.416], [-3.703, 40.417]]), json!({"_trama_directed": directed})),
        line("b", json!([[-3.703, 40.417], [-3.702, 40.418]]), json!({})),
    ]
}

#[test]
fn a_directed_edge_is_reachable_from_its_source_only() {
    let undirected = graph_of(&compile(&pair(json!(false)), &[], &[]).unwrap());
    let directed = graph_of(&compile(&pair(json!(true)), &[], &[]).unwrap());

    // SPEC 4: one CSR entry rather than two, which is the whole of what the flag does to topology.
    assert_eq!(directed.adjacency.len(), undirected.adjacency.len() - 1);
    let flagged = directed.edges.iter().filter(|edge| edge.directed).count();
    assert_eq!(flagged, 1);
    assert_eq!(undirected.edges.iter().filter(|edge| edge.directed).count(), 0);
}

#[test]
fn a_directed_edge_leaves_its_target_unable_to_traverse_it() {
    let graph = graph_of(&compile(&pair(json!(true)), &[], &[]).unwrap());

    let edge = graph.edges.iter().position(|edge| edge.directed).unwrap() as u32;
    let entries = |node: u32| {
        let start = graph.csr_offsets[node as usize] as usize;
        let end = graph.csr_offsets[node as usize + 1] as usize;
        graph.adjacency[start..end].iter().filter(|entry| entry.edge_index == edge).count()
    };
    let record = &graph.edges[edge as usize];
    assert_eq!(entries(record.source), 1, "the source can traverse it");
    assert_eq!(entries(record.target), 0, "the target cannot");
}

#[test]
fn an_absent_directed_key_means_undirected() {
    let graph = graph_of(&compile(&pair(json!(null)), &[], &[]).unwrap());

    assert!(graph.edges.iter().all(|edge| !edge.directed));
}

#[test]
fn a_non_boolean_directed_value_is_rejected_rather_than_read_as_false() {
    // OSM spells it `yes`, and compiling that as two-way would be an invisible wrong answer.
    let outcome = compile(&pair(json!("yes")), &[], &[]);

    assert!(outcome.unwrap_err().contains("_trama_directed must be a boolean"));
}

#[test]
fn the_directed_key_does_not_become_a_property_column() {
    let with_key = compile(&pair(json!(true)), &[], &[]).unwrap();
    let without = compile(&pair(json!(null)), &[], &[]).unwrap();

    assert_eq!(columns(&with_key), columns(&without), "a reserved key must not grow a PROP column");
}

#[test]
fn a_directed_edge_survives_a_round_trip_through_geojson() {
    let container = compile(&pair(json!(true)), &[], &[]).unwrap();

    let exported = trama_format::export(&container).unwrap();
    let features = exported.edges["features"].as_array().unwrap();
    let flagged: Vec<&Value> = features.iter().filter(|f| f["properties"]["_trama_directed"] == json!(true)).collect();
    assert_eq!(flagged.len(), 1, "exactly the directed edge carries the key");

    let recompiled = compile(&features.to_vec(), &[], &[]).unwrap();
    let graph = graph_of(&recompiled);
    assert_eq!(graph.edges.iter().filter(|edge| edge.directed).count(), 1);
}

#[test]
fn a_file_with_no_directed_edge_exports_without_the_key() {
    let container = compile(&pair(json!(false)), &[], &[]).unwrap();

    let exported = trama_format::export(&container).unwrap();
    for feature in exported.edges["features"].as_array().unwrap() {
        assert!(feature["properties"].get("_trama_directed").is_none());
    }
}
