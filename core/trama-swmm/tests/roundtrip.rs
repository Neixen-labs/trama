// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! A SWMM network in, a container, and the same network back out.
//!
//! Without an engine there is no verification by simulation yet, so the round trip is checked
//! at the level the importer defines: the re-imported file must produce the same entities with
//! the same properties, and the sections this crate does not read must survive verbatim.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use trama_format::Importer;
use trama_swmm::exporter::export_inp;
use trama_swmm::importer::{SwmmImporter, import};

const CRS: &str = "EPSG:3857";

fn network() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/networks/model_full_features.inp");
    std::fs::read_to_string(path).unwrap()
}

/// Entity name → (kind, properties), which is identity as the importer defines it.
fn entities(features: &[Value]) -> BTreeMap<String, (String, Value)> {
    features
        .iter()
        .map(|feature| {
            let properties = &feature["properties"];
            (
                properties["swmm:name"].as_str().unwrap().to_string(),
                (properties["swmm:kind"].as_str().unwrap().to_string(), properties.clone()),
            )
        })
        .collect()
}

#[test]
fn the_drainage_network_becomes_the_graph_and_the_hydrology_becomes_extras() {
    let imported = import(&network(), CRS).unwrap();

    let named = entities(&imported.features);
    assert_eq!(named.len(), 7, "four nodes and three links");
    assert_eq!(named["J1"].0, "junction");
    assert_eq!(named["J2"].0, "storage");
    assert_eq!(named["J4"].0, "outfall");
    assert_eq!(named["C1:C2"].0, "conduit");
    assert_eq!(named["C2"].0, "pump");
    assert_eq!(named["C3"].0, "weir");

    // Typed where the column is fixed, one parameters string where it is not.
    assert_eq!(named["J1"].1["swmm:invert"], 20.728);
    assert_eq!(named["C1:C2"].1["swmm:length"], 244.63);
    assert_eq!(named["C1:C2"].1["swmm:roughness"], 0.01);
    assert_eq!(named["C2"].1["swmm:parameters"], "* ON 0 0");

    // The cross-section folded into its link.
    assert_eq!(named["C1:C2"].1["swmm:shape"], "CIRCULAR");
    assert_eq!(named["C1:C2"].1["swmm:geom1"], 1.0);
    assert_eq!(named["C3"].1["swmm:shape"], "RECT_OPEN");

    // Hydrology travels unread: one extra owned by swmm, carrying the subcatchments.
    assert_eq!(imported.extras.len(), 1);
    assert_eq!(imported.extras[0].owner, "swmm");
    let remainder = String::from_utf8_lossy(&imported.extras[0].payload).into_owned();
    assert!(remainder.contains("[SUBCATCHMENTS]"));
    assert!(remainder.contains("[TIMESERIES]"));
    assert!(!remainder.contains("[JUNCTIONS]"), "expressed sections must not travel twice");

    // CFS is a US unit, so depth declares feet.
    let channels: Vec<(String, String)> = imported
        .channels
        .iter()
        .map(|channel| (channel["name"].as_str().unwrap().to_string(), channel["unit"].as_str().unwrap().to_string()))
        .collect();
    assert!(channels.contains(&("depth".into(), "ft".into())));
    assert!(channels.contains(&("flow".into(), "cfs".into())));
    assert!(channels.iter().any(|(name, _unit)| name == "critical"));
}

#[test]
fn a_container_full_round_trip_preserves_every_entity_and_the_hydrology() {
    let imported = import(&network(), CRS).unwrap();
    let container = trama_format::compile(&imported.features, &imported.channels, &imported.extras).unwrap();
    let written = export_inp(&container, CRS).unwrap();
    let again = import(&written, CRS).unwrap();

    let before = entities(&imported.features);
    let after = entities(&again.features);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "an entity was lost or invented on the way through"
    );
    for (name, (kind, properties)) in &before {
        assert_eq!(*kind, after[name].0, "{name} changed kind");
        assert_eq!(*properties, after[name].1, "{name} changed properties");
    }

    // The hydrology the exporter spliced back is the hydrology the importer removed.
    assert_eq!(imported.extras[0].payload, again.extras[0].payload, "the unread sections must survive verbatim");
}

#[test]
fn each_epa_importer_redirects_the_other_ones_file() {
    let swmm_error = trama_epanet::importer::import(&network(), CRS).err().unwrap();
    assert!(swmm_error.contains("--importer swmm"), "{swmm_error}");

    let epanet_text = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../trama-epanet/tests/networks/Net1.inp"),
    )
    .unwrap();
    let epanet_error = import(&epanet_text, CRS).err().unwrap();
    assert!(epanet_error.contains("EPANET"), "{epanet_error}");
}

#[test]
fn the_importer_is_reached_by_name_and_requires_a_crs() {
    assert_eq!(SwmmImporter.id(), "swmm");
    assert!(SwmmImporter.suffixes().is_empty(), ".inp belongs to the EPANET importer");

    let missing = SwmmImporter.load(std::path::Path::new("whatever.inp"), &BTreeMap::new()).err().unwrap();
    assert!(missing.contains("source-crs"), "{missing}");
}

#[test]
fn the_round_trip_is_verified_by_simulation() {
    // The standard the EPANET round trip is held to: the original file and the exported one,
    // run through the same engine, must agree — not byte-compare, simulate.
    let imported = import(&network(), CRS).unwrap();
    let container = trama_format::compile(&imported.features, &imported.channels, &imported.extras).unwrap();

    let deltas = trama_swmm::solver::solve(&container, "depth", "flow", 0.0, f32::MAX).unwrap();
    assert!(!deltas.is_empty(), "the simulation reported nothing");
    assert_eq!(deltas.len() % 18, 0, "deltas are 18-byte records");

    // Sanity of shape: every entity of the network appears, and time advances.
    let mut seen = std::collections::BTreeSet::new();
    let mut latest = 0.0f32;
    for record in deltas.chunks(18) {
        seen.insert(u64::from_le_bytes(record[0..8].try_into().unwrap()));
        latest = latest.max(f32::from_le_bytes(record[10..14].try_into().unwrap()));
    }
    assert_eq!(seen.len(), 7, "four nodes and three links should all report");
    // The fixture runs 11/01 14:00 to 11/04 00:00: 58 hours.
    assert!(latest >= 57.0 * 3600.0, "the fixture simulates 58 hours; the run stopped at {latest}s");

    // The original network through the same engine: agreement per entity per timestep, within
    // the tolerance of writing coordinates back through quantized geometry — which the routing
    // never reads, so the answers should match to float precision.
    let direct = {
        let direct_import = import(&network(), CRS).unwrap();
        let direct_container =
            trama_format::compile(&direct_import.features, &direct_import.channels, &direct_import.extras).unwrap();
        trama_swmm::solver::solve(&direct_container, "depth", "flow", 0.0, f32::MAX).unwrap()
    };
    assert_eq!(deltas, direct, "the same container must simulate identically twice");

    let exported = trama_swmm::exporter::export_inp(&container, CRS).unwrap();
    let re_imported = import(&exported, CRS).unwrap();
    let re_container =
        trama_format::compile(&re_imported.features, &re_imported.channels, &re_imported.extras).unwrap();
    let re_deltas = trama_swmm::solver::solve(&re_container, "depth", "flow", 0.0, f32::MAX).unwrap();
    assert_eq!(deltas.len(), re_deltas.len(), "the exported network must report the same schedule");
    for (a, b) in deltas.chunks(18).zip(re_deltas.chunks(18)) {
        assert_eq!(a[0..14], b[0..14], "same entity, same channel, same time");
        let (va, vb) =
            (f32::from_le_bytes(a[14..18].try_into().unwrap()), f32::from_le_bytes(b[14..18].try_into().unwrap()));
        assert!((va - vb).abs() <= 1e-4 * va.abs().max(1.0), "values diverged: {va} vs {vb}");
    }
}
