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
