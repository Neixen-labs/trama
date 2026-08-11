// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! The safety net for the migration: the same input through both implementations, compared
//! as bytes. The format is deterministic by design, so this is checkable rather than argued.

use std::{fs, path::PathBuf};

use serde_json::Value;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn features(name: &str) -> Vec<Value> {
    let source: Value =
        serde_json::from_str(&fs::read_to_string(repository().join("fixtures").join(name)).unwrap()).unwrap();
    source["features"].as_array().cloned().unwrap()
}

#[test]
fn compiles_the_shared_fixture_byte_for_byte() {
    let produced = trama_format::compile(&features("network.geojson"), &[], &[]).unwrap();

    let python = fs::read(repository().join("fixtures").join("network.trama")).unwrap();
    assert_eq!(produced, python, "the Rust and Python compilers disagree on network.geojson");
}

#[test]
fn reads_back_what_it_wrote() {
    let produced = trama_format::compile(&features("network.geojson"), &[], &[]).unwrap();

    let sections = trama_format::read_sections(&produced).unwrap();
    let kinds: Vec<String> = sections.iter().map(|s| String::from_utf8_lossy(&s.kind).into_owned()).collect();
    assert_eq!(kinds, ["GEOM", "GEOM", "GRPH", "PROP", "STCH"]);
    let graph = sections.iter().find(|s| &s.kind == b"GRPH").unwrap();
    let parsed = trama_format::parse_graph(&graph.payload).unwrap();
    assert_eq!(parsed.nodes.len(), 4);
    assert_eq!(parsed.edges.len(), 3);
    let mut identities: Vec<u64> = parsed.nodes.iter().map(|node| node.id).collect();
    let sorted = {
        let mut copy = identities.clone();
        copy.sort_unstable();
        copy
    };
    assert_eq!(identities, sorted, "SPEC 4 requires the node array sorted by ascending id");
    identities.dedup();
    assert_eq!(identities.len(), 4);
}

#[test]
fn reads_the_containers_the_python_compiler_wrote() {
    for name in ["network.trama", "demo-grid.trama", "net3.trama"] {
        let data = fs::read(repository().join("fixtures").join(name)).unwrap();
        let sections = trama_format::read_sections(&data).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(sections.iter().any(|s| &s.kind == b"GRPH"), "{name} has no graph");
    }
}

#[test]
fn rejects_a_corrupted_section() {
    let mut produced = trama_format::compile(&features("network.geojson"), &[], &[]).unwrap();
    let last = produced.len() - 1;
    produced[last] ^= 0xFF;

    assert!(trama_format::read_sections(&produced).is_err());
}

#[test]
fn an_extra_is_additive_and_nothing_else_moves() {
    let plain = trama_format::compile(&features("network.geojson"), &[], &[]).unwrap();
    let carrying = trama_format::compile(
        &features("network.geojson"),
        &[],
        &[trama_format::Extra {
            owner: "epanet".into(),
            media_type: "text/plain".into(),
            payload: b"[PATTERNS]".to_vec(),
        }],
    )
    .unwrap();

    let of = |data: &[u8]| -> Vec<(String, Vec<u8>)> {
        trama_format::read_sections(data)
            .unwrap()
            .into_iter()
            .filter(|s| &s.kind != b"XTRA")
            .map(|s| (String::from_utf8_lossy(&s.kind).into_owned(), s.payload))
            .collect()
    };
    assert_eq!(of(&carrying), of(&plain));
}

#[test]
fn exports_every_entity_the_graph_holds() {
    let data = fs::read(repository().join("fixtures").join("net3.trama")).unwrap();

    let exported = trama_format::export(&data).unwrap();

    let nodes = exported.nodes["features"].as_array().unwrap();
    let edges = exported.edges["features"].as_array().unwrap();
    assert_eq!((nodes.len(), edges.len()), (97, 119));
    // The EPANET importer put these there; the core carries them without knowing what they are.
    assert!(nodes[0]["properties"].get("epanet:name").is_some());
    assert!(nodes.iter().all(|node| node["properties"]["_trama_id"].is_string()));
}

#[test]
fn an_exported_network_recompiles_to_the_same_identities() {
    let data = fs::read(repository().join("fixtures").join("network.trama")).unwrap();
    let exported = trama_format::export(&data).unwrap();

    let mut features = exported.edges["features"].as_array().unwrap().clone();
    features.extend(exported.nodes["features"].as_array().unwrap().clone());
    let recompiled = trama_format::compile(&features, &[], &[]).unwrap();

    let before = trama_format::parse_graph(&trama_format::read_sections(&data).unwrap()[2].payload).unwrap();
    let after = trama_format::parse_graph(&trama_format::read_sections(&recompiled).unwrap()[2].payload).unwrap();
    assert_eq!(
        before.edges.iter().map(|e| e.id).collect::<Vec<u64>>(),
        after.edges.iter().map(|e| e.id).collect::<Vec<u64>>()
    );
    assert_eq!(
        before.nodes.iter().map(|n| n.id).collect::<Vec<u64>>(),
        after.nodes.iter().map(|n| n.id).collect::<Vec<u64>>()
    );
}
