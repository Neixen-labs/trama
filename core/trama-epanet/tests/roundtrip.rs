// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! `.inp` -> `.trama` -> `.inp`, verified the way SPEC 9 defines it: by simulation.
//!
//! Byte equality is not the criterion and could not be met — comments and field spacing are
//! not information about the network. What must survive is every node pressure and link flow
//! at every reported timestep.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use trama_epanet::{exporter::export_inp, importer::EpanetImporter, solver};
use trama_format::Importer;

/// Net1 and Net3 place their nodes on a small unnamed grid. Read as metres they make a network
/// about 80 m across; read as degrees the same numbers would stretch each pipe over hundreds
/// of kilometres and across hundreds of tiles, which is a fine thing to test but a poor default.
const CRS: &str = "EPSG:3857";

fn networks() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("networks")
}

fn compile(name: &str) -> Vec<u8> {
    let options: BTreeMap<String, String> = [("source-crs".to_string(), CRS.to_string())].into_iter().collect();
    let imported = EpanetImporter.load(&networks().join(name), &options).unwrap();
    trama_format::compile(&imported.features, &imported.channels, &imported.extras).unwrap()
}

/// Every node pressure and link flow, keyed by entity name and simulation time.
fn results(source: &Path) -> BTreeMap<(String, u16, u32), f32> {
    let container = compile_from(source);
    let (nodes, links) = solver::entity_ids(&container).unwrap();
    let mut names: BTreeMap<u64, String> = nodes.iter().map(|(name, id)| (*id, name.clone())).collect();
    names.extend(links.iter().map(|(name, id)| (*id, name.clone())));
    let deltas = solver::solve(&container, "pressure", "flow", 0.0, 86400.0).unwrap();
    deltas
        .chunks(18)
        .map(|record| {
            let id = u64::from_le_bytes(record[0..8].try_into().unwrap());
            let channel = u16::from_le_bytes(record[8..10].try_into().unwrap());
            let t = f32::from_le_bytes(record[10..14].try_into().unwrap());
            let value = f32::from_le_bytes(record[14..18].try_into().unwrap());
            ((names[&id].clone(), channel, t as u32), value)
        })
        .collect()
}

fn compile_from(source: &Path) -> Vec<u8> {
    let options: BTreeMap<String, String> = [("source-crs".to_string(), CRS.to_string())].into_iter().collect();
    let imported = EpanetImporter.load(source, &options).unwrap();
    trama_format::compile(&imported.features, &imported.channels, &imported.extras).unwrap()
}

#[test]
fn the_rebuilt_network_simulates_identically() {
    for name in ["Net1.inp", "Net3.inp"] {
        let container = compile(name);
        let rebuilt = std::env::temp_dir().join(format!("trama-rebuilt-{name}"));
        std::fs::write(&rebuilt, export_inp(&container, CRS).unwrap()).unwrap();

        let expected = results(&networks().join(name));
        let actual = results(&rebuilt);

        assert_eq!(actual.len(), expected.len(), "{name} produced a different number of samples");
        let worst = expected.iter().map(|(key, value)| (actual[key] - value).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-3, "{name} drifted by {worst}");
    }
}

#[test]
fn the_container_carries_one_opaque_record_and_the_core_still_validates() {
    let container = compile("Net3.inp");

    let sections = trama_format::read_sections(&container).unwrap();
    let extras: Vec<&trama_format::Section> = sections.iter().filter(|s| &s.kind == b"XTRA").collect();
    assert_eq!(extras.len(), 1);
    let text = String::from_utf8_lossy(&extras[0].payload);
    // What the core cannot type went in; what it can type stayed out.
    assert!(text.contains("[PATTERNS]"));
    assert!(!text.contains("[JUNCTIONS]") && !text.contains("[COORDINATES]"));
}

#[test]
fn an_inp_without_a_declared_crs_is_refused() {
    let outcome = EpanetImporter.load(&networks().join("Net1.inp"), &BTreeMap::new());

    let Err(error) = outcome else { panic!("an .inp with no CRS was accepted") };
    assert!(error.contains("coordinate reference system"), "{error}");
}

#[test]
fn a_channel_the_container_never_declared_is_refused() {
    let container = compile("Net1.inp");

    let error = solver::solve(&container, "head", "flow", 0.0, 3600.0).unwrap_err();

    assert!(error.contains("no node channel named 'head'"), "{error}");
}

#[test]
fn a_container_from_another_format_is_refused() {
    let features: Vec<serde_json::Value> = vec![serde_json::json!({
        "type": "Feature",
        "id": "a",
        "properties": {},
        "geometry": {"type": "LineString", "coordinates": [[-3.704, 40.416], [-3.703, 40.417]]},
    })];
    let channels = vec![
        serde_json::json!({"name": "pressure", "entity_kind": "node", "unit": "m"}),
        serde_json::json!({"name": "flow", "entity_kind": "edge", "unit": "l/s"}),
    ];
    let container = trama_format::compile(&features, &channels, &[]).unwrap();

    let error = solver::solve(&container, "pressure", "flow", 0.0, 3600.0).unwrap_err();

    assert!(error.contains("not compiled from an EPANET network"), "{error}");
}
