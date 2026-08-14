// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! What a pandapower network must survive being compiled: its topology, its electrical
//! parameters, and everything a reader needs to put it back together.

use std::collections::BTreeMap;
use std::path::PathBuf;

use trama_format::Importer;
use trama_power::PowerImporter;

fn network() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("networks").join("oberrhein.json")
}

fn imported() -> trama_format::Import {
    PowerImporter.load(&network(), &BTreeMap::new()).unwrap()
}

fn compiled() -> Vec<u8> {
    let import = imported();
    trama_format::compile(&import.features, &import.channels, &import.extras).unwrap()
}

#[test]
fn buses_become_nodes_and_lines_and_transformers_become_edges() {
    let import = imported();
    let kinds: BTreeMap<&str, usize> = import.features.iter().fold(BTreeMap::new(), |mut counted, feature| {
        *counted.entry(feature["properties"]["power:kind"].as_str().unwrap()).or_default() += 1;
        counted
    });

    // The fixture is pandapower's own mv_oberrhein: 179 buses, 181 lines, 2 transformers.
    assert_eq!(kinds["bus"], 179);
    assert_eq!(kinds["line"], 181);
    assert_eq!(kinds["trafo"], 2);
}

#[test]
fn an_edge_starts_and_ends_exactly_on_the_buses_it_connects() {
    let import = imported();
    let position = |index: i64| -> Vec<f64> {
        let bus = import
            .features
            .iter()
            .find(|feature| {
                feature["properties"]["power:kind"] == "bus" && feature["properties"]["power:index"] == index
            })
            .expect("the edge names a bus this network has");
        bus["geometry"]["coordinates"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect()
    };

    // Every edge, not a sample: SPEC 4.2 takes a node from each end of a LineString, so an
    // endpoint that misses its bus by a metre is a graph that silently comes apart.
    for feature in import.features.iter().filter(|feature| feature["properties"]["power:kind"] != "bus") {
        let path = feature["geometry"]["coordinates"].as_array().unwrap();
        let ends =
            |at: usize| -> Vec<f64> { path[at].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect() };
        let kind = feature["properties"]["power:kind"].as_str().unwrap();
        let (from, to) = if kind == "line" { ("from_bus", "to_bus") } else { ("hv_bus", "lv_bus") };
        assert_eq!(ends(0), position(feature["properties"][format!("power:{from}")].as_i64().unwrap()));
        assert_eq!(ends(path.len() - 1), position(feature["properties"][format!("power:{to}")].as_i64().unwrap()));
    }
}

#[test]
fn a_line_keeps_the_bends_its_own_geometry_drew() {
    let import = imported();
    let bendy = import
        .features
        .iter()
        .filter(|feature| feature["properties"]["power:kind"] == "line")
        .max_by_key(|feature| feature["geometry"]["coordinates"].as_array().unwrap().len())
        .unwrap();

    // A line is more than its endpoints, or the map draws a spider web over the countryside.
    assert!(bendy["geometry"]["coordinates"].as_array().unwrap().len() > 2);
}

#[test]
fn the_electrical_parameters_travel_as_typed_properties() {
    let import = imported();
    let line = import.features.iter().find(|feature| feature["properties"]["power:kind"] == "line").unwrap();

    // What a load flow needs to solve this span, in the units pandapower wrote them in.
    for key in ["power:length_km", "power:r_ohm_per_km", "power:x_ohm_per_km", "power:max_i_ka"] {
        assert!(line["properties"][key].is_number(), "{key} is absent or not a number");
    }
    // And the geometry is not stored twice: it became the feature's own shape.
    assert!(line["properties"]["power:geo"].is_null());
}

#[test]
fn the_opaque_record_carries_the_rest_of_the_network_and_the_schema_of_what_was_expressed() {
    let import = imported();
    let payload = String::from_utf8(import.extras[0].payload.clone()).unwrap();
    let document: serde_json::Value = serde_json::from_str(&payload).unwrap();
    let table = |name: &str| -> serde_json::Value {
        serde_json::from_str(document["_object"][name]["_object"].as_str().unwrap()).unwrap()
    };

    // What the core cannot type went in whole: the loads and the external grid decide the flows.
    assert!(!table("load")["data"].as_array().unwrap().is_empty());
    assert!(!table("ext_grid")["data"].as_array().unwrap().is_empty());
    // What it can type kept only its column order, which is what a reader needs to rebuild it.
    assert!(table("bus")["data"].as_array().unwrap().is_empty());
    assert!(table("line")["data"].as_array().unwrap().is_empty());
    assert_eq!(table("bus")["columns"][0], "name");
}

#[test]
fn the_container_declares_what_an_electrical_network_can_be_solved_for() {
    let container = compiled();

    let channels = trama_solver::channels(&container).unwrap();
    let named: BTreeMap<&str, &trama_solver::Channel> =
        channels.iter().map(|channel| (channel.name.as_str(), channel)).collect();

    assert_eq!((named["voltage"].entity_kind, named["voltage"].unit.as_str()), (1, "p.u."));
    assert_eq!((named["loading"].entity_kind, named["loading"].unit.as_str()), (2, "%"));
    // Neither declares a range: an undervoltage and an overloaded line are the answers the
    // study exists to find, and a declared range would have the host reject them as invalid.
    assert!(channels.iter().all(|channel| !channel.range_present));
}

#[test]
fn the_graph_joins_where_the_network_does() {
    let container = compiled();
    let exported = trama_format::export(&container).unwrap();

    let nodes = exported.nodes["features"].as_array().unwrap().len();
    let edges = exported.edges["features"].as_array().unwrap().len();
    // 179 buses in, 179 nodes out: every line endpoint landed on a bus rather than creating one.
    assert_eq!(nodes, 179);
    assert_eq!(edges, 183);
}

#[test]
fn the_two_sides_of_a_transformer_stay_two_nodes() {
    let import = imported();
    let position = |index: i64| -> (f64, f64) {
        let bus = import
            .features
            .iter()
            .find(|f| f["properties"]["power:kind"] == "bus" && f["properties"]["power:index"] == index)
            .unwrap();
        let pair = bus["geometry"]["coordinates"].as_array().unwrap();
        (pair[0].as_f64().unwrap(), pair[1].as_f64().unwrap())
    };

    // Buses 39 and 58 are the 20 kV and 110 kV sides of the fixture's transformer 114, drawn at
    // one coordinate because that is where the substation is. Left there they would be one node,
    // which shorts the transformer out and leaves a network with a single voltage level.
    let (low, high) = (position(39), position(58));
    assert_ne!(low, high, "the two sides of a transformer collapsed into one node");
    assert_eq!(low.1, high.1, "separation is eastward, so the latitude is untouched");
    // Ten metres: below what a network-scale view resolves, above the quantization floor.
    let apart = (high.0 - low.0).abs() * 111_319.49 * low.1.to_radians().cos();
    assert!((apart - 10.0).abs() < 0.1, "the two sides ended up {apart} m apart");
}

#[test]
fn a_document_that_is_not_a_pandapower_network_is_refused() {
    let error = trama_power::import("{\"type\": \"FeatureCollection\", \"features\": []}").err().unwrap();

    assert!(error.contains("pandapower.to_json"), "{error}");
}
